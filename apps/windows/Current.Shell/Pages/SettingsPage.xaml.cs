using Current.Shell.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.ApplicationModel.DataTransfer;

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

    private void OnCopyDiagnostics(object sender, RoutedEventArgs e)
    {
        var package = new DataPackage();
        package.SetText(ViewModel.DiagnosticsText());
        Clipboard.SetContent(package);
        // Keeps the text on the clipboard after the app exits.
        Clipboard.Flush();
        CopiedInfo.IsOpen = true;
    }
}
