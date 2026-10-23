using System.ComponentModel;
using System.Diagnostics;
using Current.Bindings;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Navigation;
using Windows.System;

namespace Current.Shell.Pages;

/// <summary>
/// Signing in to one server with a password or Quick Connect (ADR 0015). It renders the SignInFlow's state and sends the user's
/// actions. The password goes from the PasswordBox straight into the call; no field holds it. The page owns the flow it is navigated
/// with (<see cref="SignInRequest"/>) and disposes it when left.
/// </summary>
public sealed partial class SignInPage : Page
{
    private SignInRequest? _request;
    private ValueSubscription<SignInState>? _state;
    private CancellationTokenSource? _attempt;
    private bool _leaving;

    public SignInPage()
    {
        InitializeComponent();
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        if (e.Parameter is not SignInRequest request)
        {
            return;
        }
        _request = request;
        Heading.Text = $"Sign in to {request.ServerName}";
        TlsWarning.IsOpen = request.HttpsFailedTls;
        UserNameBox.Text = request.UserName ?? "";
        _state = request.Flow.ObserveState(new DispatcherQueueUi(DispatcherQueue));
        _state.PropertyChanged += OnStateChanged;
        Render(_state.Value);
        _ = CheckQuickConnectAsync(request.Flow);
        // The user name is known when signing in again: start in the password box.
        if (UserNameBox.Text.Length > 0)
        {
            PasswordInput.Focus(FocusState.Programmatic);
        }
        else
        {
            UserNameBox.Focus(FocusState.Programmatic);
        }
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        _leaving = true;
        PasswordInput.Password = "";
        // A sign-in the server already accepted is saved by the core even when cancelled (ADR 0015).
        _attempt?.Cancel();
        if (_state is not null)
        {
            _state.PropertyChanged -= OnStateChanged;
            _state.Dispose();
            _state = null;
        }
        _request?.Flow.Dispose();
        _request = null;
    }

    private void OnStateChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (_state is not null)
        {
            Render(_state.Value);
        }
    }

    private void Render(SignInState state)
    {
        var signingIn = state is SignInState.SigningIn;
        var waiting = state is SignInState.QuickConnectWaiting;

        PasswordForm.Visibility = waiting ? Visibility.Collapsed : Visibility.Visible;
        UserNameBox.IsReadOnly = signingIn;
        PasswordInput.IsEnabled = !signingIn;
        SignInButton.IsEnabled = !signingIn;
        CancelButton.Visibility = signingIn ? Visibility.Visible : Visibility.Collapsed;
        SigningInPanel.Visibility = signingIn ? Visibility.Visible : Visibility.Collapsed;
        SigningInRing.IsActive = signingIn;

        QuickConnectPanel.Visibility = waiting ? Visibility.Visible : Visibility.Collapsed;
        WaitingRing.IsActive = waiting;

        switch (state)
        {
            case SignInState.Ready ready:
                HideError();
                // Offered once the server said it has Quick Connect; shown but unavailable while that is not known yet.
                QuickConnectOffer.Visibility = ready.QuickConnect == false ? Visibility.Collapsed : Visibility.Visible;
                QuickConnectButton.IsEnabled = ready.QuickConnect == true;
                break;
            case SignInState.SigningIn:
                HideError();
                QuickConnectButton.IsEnabled = false;
                break;
            case SignInState.QuickConnectWaiting code:
                HideError();
                CodeText.Text = code.Code;
                // Narrator reads the digits one by one.
                AutomationProperties.SetName(CodeText, $"Quick Connect code {string.Join(" ", code.Code.Select(digit => digit.ToString()))}");
                QuickConnectCancel.Focus(FocusState.Programmatic);
                break;
            case SignInState.Failed failed:
                ShowError(ProblemText.SignIn(failed.Problem));
                QuickConnectButton.IsEnabled = true;
                break;
            case SignInState.SignedIn signedIn:
                DispatcherQueue.TryEnqueue(() => Finish(signedIn));
                break;
        }
    }

    private void ShowError(string message)
    {
        // Close and open again, so Narrator reads a changed message (InfoBar page, learn.microsoft.com, read 2026-10-23).
        SignInError.IsOpen = false;
        SignInError.Message = message;
        SignInError.IsOpen = true;
    }

    private void HideError() => SignInError.IsOpen = false;

    private async Task CheckQuickConnectAsync(SignInFlow flow)
    {
        try
        {
            await flow.CheckQuickConnectAsync();
        }
        catch (Exception error)
        {
            // A failure leaves Quick Connect unknown; password sign-in still works.
            Trace.TraceError($"Current: checking Quick Connect failed: {error.GetType().Name}");
        }
    }

    private void OnFormKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Enter)
        {
            e.Handled = true;
            SignInWithPassword();
        }
    }

    private void OnSignIn(object sender, RoutedEventArgs e) => SignInWithPassword();

    private void SignInWithPassword()
    {
        if (_request is null || _attempt is not null)
        {
            return;
        }
        if (UserNameBox.Text.Trim().Length == 0)
        {
            ShowError("Enter your user name.");
            UserNameBox.Focus(FocusState.Programmatic);
            return;
        }
        var flow = _request.Flow;
        // The password is read here and handed to the call; nothing keeps it (ADR 0015).
        Run(token => flow.WithPasswordAsync(UserNameBox.Text.Trim(), PasswordInput.Password, token));
    }

    private void OnQuickConnect(object sender, RoutedEventArgs e)
    {
        if (_request is null || _attempt is not null)
        {
            return;
        }
        var flow = _request.Flow;
        Run(flow.WithQuickConnectAsync);
    }

    // async void, as event handlers are: it catches everything itself.
    private async void Run(Func<CancellationToken, Task> attempt)
    {
        using var cancel = new CancellationTokenSource();
        _attempt = cancel;
        try
        {
            await attempt(cancel.Token);
        }
        catch (OperationCanceledException)
        {
            // Cancelled: the flow is back to Ready.
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: signing in failed: {error.GetType().Name}");
            if (!_leaving)
            {
                ShowError(ProblemText.Unexpected);
            }
        }
        finally
        {
            _attempt = null;
        }
    }

    private void OnCancel(object sender, RoutedEventArgs e) => Cancel();

    private void OnEscape(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args) => args.Handled = Cancel();

    private bool Cancel()
    {
        if (_attempt is null)
        {
            return false;
        }
        _attempt.Cancel();
        _request?.Flow.Cancel();
        PasswordInput.Focus(FocusState.Keyboard);
        return true;
    }

    /// <summary>
    /// Signed in and saved: back to Home, and the setup pages leave the back stack, so Back does not return into a finished sign-in
    /// (manager, 2026-10-23).
    /// </summary>
    private void Finish(SignInState.SignedIn signedIn)
    {
        if (_leaving)
        {
            return;
        }
        if (!signedIn.Remembered)
        {
            App.ShowNotice(
                InfoBarSeverity.Warning,
                "Signed in for this session only.",
                "Windows Credential Manager isn't available, so you'll need to sign in again next time.");
        }
        Frame.Navigate(typeof(HomePage));
        Frame.BackStack.Clear();
    }
}
