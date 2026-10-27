using System.ComponentModel;
using System.Diagnostics;
using Current.Bindings;
using Current.Shell.Gateway;
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
/// with (<see cref="SignInRequest"/>) and disposes it when left; MainWindow keeps it out of the back and forward stacks, so it is
/// never shown again.
/// </summary>
public sealed partial class SignInPage : Page
{
    private SignInRequest? _request;
    private ValueSubscription<SignInState>? _state;
    private SignInState? _shown;
    private bool? _quickConnect;
    private CancellationTokenSource? _attempt;
    private readonly CancellationTokenSource _leave = new();
    private bool _leaving;
    private bool _gatewayRunning;

    public SignInPage()
    {
        InitializeComponent();
        Loaded += OnLoaded;
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        // Only a new navigation with an unused request: going back or forward to a sign-in would find its flow disposed.
        if (e.NavigationMode is NavigationMode.Back or NavigationMode.Forward
            || e.Parameter is not SignInRequest { Taken: false } request)
        {
            return;
        }
        request.Taken = true;
        _request = request;
        Heading.Text = $"Sign in to {request.ServerName}";
        TlsWarning.IsOpen = request.HttpsFailedTls;
        UserNameBox.Text = request.UserName ?? "";
        _state = request.Flow.ObserveState(new DispatcherQueueUi(DispatcherQueue));
        _state.PropertyChanged += OnStateChanged;
        Render(_state.Value);
        _ = CheckQuickConnectAsync(request.Flow, _leave.Token);
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        _leaving = true;
        PasswordInput.Password = "";
        _leave.Cancel();
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

    /// <summary>Focus once the page is in the tree: the user name, or the password when signing in again.</summary>
    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        if (_request is not null)
        {
            FocusForm();
        }
    }

    private void FocusForm()
    {
        if (UserNameBox.Text.Length > 0)
        {
            PasswordInput.Focus(FocusState.Programmatic);
        }
        else
        {
            UserNameBox.Focus(FocusState.Programmatic);
        }
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
        var before = _shown;
        _shown = state;
        var signingIn = state is SignInState.SigningIn;
        var waiting = state is SignInState.QuickConnectWaiting;

        PasswordForm.Visibility = waiting ? Visibility.Collapsed : Visibility.Visible;
        UserNameBox.IsReadOnly = signingIn;
        // PasswordBox has no read-only mode, so it is disabled; focus moves to Cancel meanwhile.
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
                _quickConnect = ready.QuickConnect ?? _quickConnect;
                break;
            case SignInState.SigningIn:
                HideError();
                if (before is not SignInState.SigningIn)
                {
                    FocusLater(CancelButton);
                    Announce.Shown(SigningInText);
                }
                break;
            case SignInState.QuickConnectWaiting code:
                HideError();
                var newCode = before is not SignInState.QuickConnectWaiting { Code: var shownCode } || shownCode != code.Code;
                CodeText.Text = code.Code;
                // Narrator reads the digits one by one, when the code is announced and when the user reaches it.
                var spoken = $"Quick Connect code {string.Join(" ", code.Code.Select(digit => digit.ToString()))}";
                AutomationProperties.SetName(CodeView, spoken);
                if (before is not SignInState.QuickConnectWaiting)
                {
                    FocusLater(QuickConnectCancel);
                }
                if (newCode)
                {
                    CodeCopiedInfo.IsOpen = false;
                    // The code itself is what the user needs to hear, when it appears and when a new one replaces it (owner's run,
                    // 2026-10-23: only "Waiting for approval…" was read). One announcement, so the two do not cut each other off. A
                    // notification, not a live region: Narrator read the live region's text, the code as one number (owner's run,
                    // 2026-10-23), not its digit-by-digit name.
                    Announce.Say(CodeView, spoken, "QuickConnectCode");
                }
                break;
            case SignInState.Failed { Problem: SignInProblem.Gateway gateway }
                when gateway.Answer != GatewayProblem.Unsupported && _request?.Saved is not null:
                ShowError(App.Core?.ProblemText(gateway) ?? ProblemTexts.Unexpected);
                OfferGatewaySignIn(gateway.Answer == GatewayProblem.Refused);
                break;
            case SignInState.Failed failed:
                ShowError(App.Core?.ProblemText(failed.Problem) ?? ProblemTexts.Unexpected);
                break;
            case SignInState.SignedIn signedIn:
                DispatcherQueue.TryEnqueue(() => Finish(signedIn));
                break;
        }

        // Offered once the server said it has Quick Connect; shown but unavailable while that is not known yet.
        QuickConnectOffer.Visibility = _quickConnect == false ? Visibility.Collapsed : Visibility.Visible;
        QuickConnectButton.IsEnabled = _quickConnect == true && !signingIn;

        // Back to the form after a cancel or a failure: the control that had focus is now disabled or hidden.
        if (state is SignInState.Ready or SignInState.Failed
            && before is SignInState.SigningIn or SignInState.QuickConnectWaiting)
        {
            DispatcherQueue.TryEnqueue(FocusForm);
        }
    }

    /// <summary>Focus after this layout pass, so a control just made visible or enabled can take it.</summary>
    private void FocusLater(Control control) => DispatcherQueue.TryEnqueue(() => control.Focus(FocusState.Programmatic));

    /// <summary>
    /// The gateway in front of a saved server answered the sign-in (ADR 0017): the error offers to sign in to it, with the same
    /// password dialog, sign-in window or extra headers as Settings; then the user signs in to the server as before.
    /// </summary>
    private void OfferGatewaySignIn(bool headers)
    {
        var button = new Button { Content = "Sign in to the gateway" };
        button.Click += async (_, _) =>
        {
            if (App.Core is not { } core || _request?.Saved is not { } server || XamlRoot is null || _gatewayRunning)
            {
                return;
            }
            _gatewayRunning = true;
            button.IsEnabled = false;
            try
            {
                var outcome = await new GatewaySignInRunner(XamlRoot, new DispatcherQueueUi(DispatcherQueue))
                    .RunAsync(core, server, headers);
                if (_leaving)
                {
                    return;
                }
                if (outcome.Severity is InfoBarSeverity.Success or InfoBarSeverity.Warning)
                {
                    HideError();
                    App.Announce("Signed in to the gateway. Now sign in to the server.", "GatewaySignedIn");
                    FocusForm();
                }
                else if (outcome.Message is { } message)
                {
                    ShowError(message);
                }
            }
            catch (Exception error)
            {
                Trace.TraceError($"Current: signing in to the gateway failed: {error.GetType().Name}");
                if (!_leaving)
                {
                    ShowError(ProblemTexts.Unexpected);
                }
            }
            finally
            {
                _gatewayRunning = false;
                button.IsEnabled = true;
            }
        };
        SignInError.ActionButton = button;
    }

    private void ShowError(string message)
    {
        // Close and open again, so Narrator reads a changed message (InfoBar page, learn.microsoft.com, read 2026-10-23).
        SignInError.IsOpen = false;
        SignInError.ActionButton = null;
        SignInError.Message = message;
        SignInError.IsOpen = true;
    }

    private void HideError() => SignInError.IsOpen = false;

    private static async Task CheckQuickConnectAsync(SignInFlow flow, CancellationToken leave)
    {
        try
        {
            await flow.CheckQuickConnectAsync(leave);
        }
        catch (OperationCanceledException)
        {
            // The page was left.
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
                ShowError(ProblemTexts.Unexpected);
            }
        }
        finally
        {
            _attempt = null;
        }
    }

    private void OnCancel(object sender, RoutedEventArgs e) => Cancel();

    // async void, as event handlers are: it catches everything itself, because an exception escaping it would end the process.
    private async void OnCopyCode(object sender, RoutedEventArgs e)
    {
        bool copied;
        try
        {
            copied = await ClipboardText.TryCopyAsync(CodeText.Text);
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: copying the Quick Connect code failed: {error.GetType().Name}");
            copied = false;
        }
        // As Copy diagnostics does: closed and opened again, so Narrator reads it each time.
        CodeCopiedInfo.IsOpen = false;
        CodeCopiedInfo.Severity = copied ? InfoBarSeverity.Success : InfoBarSeverity.Error;
        CodeCopiedInfo.Message = copied
            ? "Code copied to the clipboard."
            : "The clipboard is in use by another app. Try again.";
        CodeCopiedInfo.IsOpen = true;
    }

    private void OnEscape(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args) => args.Handled = Cancel();

    /// <summary>Stops the running attempt; the flow goes back to Ready, and Render puts focus back on the form.</summary>
    private bool Cancel()
    {
        if (_attempt is null)
        {
            return false;
        }
        _attempt.Cancel();
        _request?.Flow.Cancel();
        return true;
    }

    /// <summary>
    /// Signed in and saved: back to Home, with the history cleared, so Back does not return into a finished sign-in (manager,
    /// 2026-10-23).
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
        var server = _request?.ServerName ?? "the server";
        Frame.Navigate(typeof(HomePage));
        App.ClearNavigationHistory();
        // Nothing on Home is read by itself, so the result is announced (owner's run, 2026-10-24: Narrator read the title bar). From
        // the window, which outlives this page.
        App.Announce($"Signed in to {server} as {signedIn.User.Name}.", "SignedIn");
    }
}
