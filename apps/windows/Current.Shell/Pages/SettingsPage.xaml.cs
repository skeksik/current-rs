using System.Diagnostics;
using Current.Shell.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Pages;

public sealed partial class SettingsPage : Page
{
    public SettingsPage()
    {
        ViewModel = new SettingsViewModel(App.Core, new DispatcherQueueUi(DispatcherQueue));
        InitializeComponent();
        // The subscription lives while the page is shown: started on Loaded, cancelled on Unloaded (ADR 0004).
        Loaded += (_, _) => ViewModel.Start();
        Unloaded += (_, _) => ViewModel.Stop();
    }

    public SettingsViewModel ViewModel { get; }

    // async void, as event handlers are: it catches everything itself, because an exception escaping it would end the process.
    private async void OnCopyDiagnostics(object sender, RoutedEventArgs e)
    {
        bool copied;
        try
        {
            copied = await ClipboardText.TryCopyAsync(ViewModel.DiagnosticsText());
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: copying diagnostics failed: {error}");
            copied = false;
        }
        CopiedInfo.Severity = copied ? InfoBarSeverity.Success : InfoBarSeverity.Error;
        CopiedInfo.Message = copied
            ? "Diagnostics copied to the clipboard."
            : "The clipboard is in use by another app. Try again.";
        CopiedInfo.IsOpen = true;
    }
}
