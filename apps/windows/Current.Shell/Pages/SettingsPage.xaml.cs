using System.Diagnostics;
using Current.Bindings;
using Current.Shell.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Pages;

/// <summary>
/// Settings: the saved servers under Accounts (sign out, sign in again), diagnostics, and About. It waits for the core to open
/// (App.CoreReady); the server subscriptions live from Loaded to Unloaded (ADR 0004).
/// </summary>
public sealed partial class SettingsPage : Page
{
    private bool _loaded;

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
            Trace.TraceError($"Current: the settings page failed to load: {error}");
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
            Trace.TraceError($"Current: copying diagnostics failed: {error}");
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
