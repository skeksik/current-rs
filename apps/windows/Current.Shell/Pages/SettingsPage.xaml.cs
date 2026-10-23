using System.Diagnostics;
using Current.Bindings;
using Current.Shell.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Pages;

/// <summary>
/// Settings: the saved servers under Accounts (sign out, sign in again, remove), diagnostics, and About. It waits for the core to open
/// (App.CoreReady); the server subscriptions live from Loaded to Unloaded (ADR 0004).
/// </summary>
public sealed partial class SettingsPage : Page
{
    private bool _loaded;

    /// <summary>A Remove confirmation is open: a second one is not started.</summary>
    private bool _confirmingRemoval;

    public SettingsPage()
    {
        InitializeComponent();
        Loaded += OnLoaded;
        Unloaded += OnUnloaded;
    }

    public ServersViewModel? Servers { get; private set; }

    public string VersionText { get; private set; } = "";

    // async void, as event handlers are: it catches everything itself.
    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        _loaded = true;
        try
        {
            var core = await App.CoreReady;
            if (!_loaded || core is null)
            {
                return;
            }
            VersionText = $"Version {core.AppInfo.Version}";
            Servers ??= new ServersViewModel(core, new DispatcherQueueUi(DispatcherQueue));
            Servers.Start();
            Bindings.Update();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the settings page failed to load: {error.GetType().Name}");
        }
    }

    private void OnUnloaded(object sender, RoutedEventArgs e)
    {
        _loaded = false;
        Servers?.Stop();
    }

    private void OnAddServer(object sender, RoutedEventArgs e) => Frame.Navigate(typeof(AddServerPage));

    private void OnReload(object sender, RoutedEventArgs e) => Servers?.Reload();

    private void OnSignInAgain(object sender, RoutedEventArgs e)
    {
        if (App.Core is { } core && (sender as FrameworkElement)?.DataContext is ServerRow row)
        {
            SignInRequest.SignInAgain(Frame, core, row);
        }
    }

    /// <summary>
    /// Signs out at once, without asking (manager, 2026-10-23): signing in again undoes it. This PC forgets the session either way;
    /// the notice says when the server could not be told (ADR 0015, "Sign out"). async void, as event handlers are: it catches
    /// everything itself.
    /// </summary>
    private async void OnSignOut(object sender, RoutedEventArgs e)
    {
        if (App.Core is not { } core || sender is not Button { DataContext: ServerRow row } button)
        {
            return;
        }
        button.IsEnabled = false;
        AccountsInfo.IsOpen = false;
        try
        {
            var result = await core.SignOutAsync(row.Server);
            if (!result.ServerTold)
            {
                ShowAccountsInfo(
                    InfoBarSeverity.Informational,
                    $"Signed out of {row.Name} on this PC.",
                    "The server couldn't be told, so the session may still show in its dashboard.");
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: signing out failed: {error.GetType().Name}");
            ShowAccountsInfo(InfoBarSeverity.Error, $"Couldn't sign out of {row.Name}.", ProblemText.Unexpected);
        }
        finally
        {
            button.IsEnabled = true;
        }
    }

    /// <summary>
    /// Removes a server after a confirmation (Dialogs page, learn.microsoft.com, ms.date 2026-07-15, read 2026-10-23): the close
    /// button is the safe action, the default, and answers Esc, as the plain-http dialog. Unlike sign-out, removing cannot be undone
    /// by signing in again: the server would have to be added again. Home and this list update from the saved-server list. async
    /// void, as event handlers are: it catches everything itself.
    /// </summary>
    private async void OnRemove(object sender, RoutedEventArgs e)
    {
        if (App.Core is not { } core || sender is not Button { Tag: ServerRow row } button || row.Removing || _confirmingRemoval
            || XamlRoot is null)
        {
            return;
        }
        AccountsInfo.IsOpen = false;
        // Only one dialog at a time, and nothing is disabled while it is open: a disabled Remove button would lose focus, and
        // Cancel would leave it elsewhere (the owner's run, 2026-10-23).
        _confirmingRemoval = true;
        ContentDialogResult answer;
        try
        {
            var dialog = new ContentDialog
            {
                XamlRoot = XamlRoot,
                Title = $"Remove {row.Name}?",
                Content = "This signs out every user on this PC and forgets the server's addresses. You can add it again later.",
                PrimaryButtonText = "Remove",
                CloseButtonText = "Cancel",
                DefaultButton = ContentDialogButton.Close,
            };
            answer = await dialog.ShowAsync();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the remove confirmation failed: {error.GetType().Name}");
            ShowAccountsInfo(InfoBarSeverity.Error, $"Couldn't remove {row.Name}.", ProblemText.Unexpected);
            return;
        }
        finally
        {
            _confirmingRemoval = false;
        }
        if (answer != ContentDialogResult.Primary)
        {
            // Whether ContentDialog itself gives focus back to the element that opened it is not verified (no Microsoft source
            // found, 2026-10-23), so it is given back here.
            button.Focus(FocusState.Programmatic);
            return;
        }
        row.Removing = true;
        try
        {
            var result = await core.RemoveServerAsync(row.Server);
            FocusAfterRemoval(row);
            if (result.CredentialsLeft)
            {
                ShowAccountsInfo(
                    InfoBarSeverity.Warning,
                    $"Removed {row.Name}, but its saved sign-in may still be in Credential Manager.",
                    "Current no longer uses it. You can delete the entries ending in \".Current\" under Windows Credentials.");
            }
        }
        catch (CoreStoreException error)
        {
            ShowAccountsInfo(InfoBarSeverity.Error, $"Couldn't remove {row.Name}.", ProblemText.Store(error.Problem));
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: removing a server failed: {error.GetType().Name}");
            ShowAccountsInfo(InfoBarSeverity.Error, $"Couldn't remove {row.Name}.", ProblemText.Unexpected);
        }
        finally
        {
            // A row that stays (the removal failed) works again; a removed one is gone with its buttons.
            row.Removing = false;
        }
    }

    /// <summary>
    /// The removed row took the focused button with it: once the row has left the list, focus goes to "Add another server", or to
    /// "Add a server" when no server is left. Queued, so the list's visibility has changed first.
    /// </summary>
    private void FocusAfterRemoval(ServerRow row)
    {
        if (Servers is not { } servers)
        {
            return;
        }
        void MoveFocus() => DispatcherQueue.TryEnqueue(() =>
            (servers.HasServers ? AddAnotherServerButton : AddFirstServerButton).Focus(FocusState.Programmatic));
        if (!servers.Rows.Contains(row))
        {
            MoveFocus();
            return;
        }
        void OnRowsChanged(object? sender, System.Collections.Specialized.NotifyCollectionChangedEventArgs e)
        {
            if (!servers.Rows.Contains(row))
            {
                servers.Rows.CollectionChanged -= OnRowsChanged;
                MoveFocus();
            }
        }
        servers.Rows.CollectionChanged += OnRowsChanged;
    }

    private void ShowAccountsInfo(InfoBarSeverity severity, string title, string message)
    {
        AccountsInfo.Severity = severity;
        AccountsInfo.Title = title;
        AccountsInfo.Message = message;
        AccountsInfo.IsOpen = true;
    }

    // async void, as event handlers are: it catches everything itself, because an exception escaping it would end the process.
    private async void OnCopyDiagnostics(object sender, RoutedEventArgs e)
    {
        bool copied;
        try
        {
            var text = App.Core is { } core ? await core.ExportDiagnosticsAsync() : "";
            copied = await ClipboardText.TryCopyAsync(text);
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: copying diagnostics failed: {error.GetType().Name}");
            copied = false;
        }
        CopiedInfo.IsOpen = false;
        CopiedInfo.Severity = copied ? InfoBarSeverity.Success : InfoBarSeverity.Error;
        CopiedInfo.Message = copied
            ? "Diagnostics copied to the clipboard."
            : "The clipboard is in use by another app. Try again.";
        CopiedInfo.IsOpen = true;
    }
}
