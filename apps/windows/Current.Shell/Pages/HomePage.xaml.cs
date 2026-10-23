using System.Diagnostics;
using Current.Shell.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Pages;

/// <summary>
/// Home: an invitation to add a server before there is one, else the saved servers with their user (browse is M2). It waits for the
/// core to open (App.CoreReady); its subscriptions live from Loaded to Unloaded (ADR 0004).
/// </summary>
public sealed partial class HomePage : Page
{
    private static readonly TimeSpan OpeningDelay = TimeSpan.FromMilliseconds(500);
    private bool _loaded;

    public HomePage()
    {
        InitializeComponent();
        Loaded += OnLoaded;
        Unloaded += OnUnloaded;
    }

    public ServersViewModel? ViewModel { get; private set; }

    // async void, as event handlers are: it catches everything itself.
    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        _loaded = true;
        try
        {
            var ready = App.CoreReady;
            if (!ready.IsCompleted)
            {
                // A ring only if opening takes a moment, so a fast start does not flash one.
                if (await Task.WhenAny(ready, Task.Delay(OpeningDelay)) != ready)
                {
                    Opening.IsActive = true;
                }
            }
            var core = await ready;
            Opening.IsActive = false;
            if (!_loaded)
            {
                return;
            }
            if (core is null)
            {
                CoreFailed.Visibility = Visibility.Visible;
                return;
            }
            ViewModel ??= new ServersViewModel(core, new DispatcherQueueUi(DispatcherQueue));
            ViewModel.Start();
            Bindings.Update();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the home page failed to load: {error.GetType().Name}");
        }
    }

    private void OnUnloaded(object sender, RoutedEventArgs e)
    {
        _loaded = false;
        ViewModel?.Stop();
    }

    private void OnAddServer(object sender, RoutedEventArgs e) => Frame.Navigate(typeof(AddServerPage));

    private void OnReload(object sender, RoutedEventArgs e) => ViewModel?.Reload();

    private void OnSignInAgain(object sender, RoutedEventArgs e)
    {
        if (App.Core is { } core && (sender as FrameworkElement)?.DataContext is ServerRow row)
        {
            SignInRequest.SignInAgain(Frame, core, row);
        }
    }
}
