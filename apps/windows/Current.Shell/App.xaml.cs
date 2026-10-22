using Current.Bindings;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;

namespace Current.Shell;

public partial class App : Application
{
    private readonly bool _smoke;
    private static DispatcherQueue? _ui;
    private static MainWindow? _window;

    public App(bool smoke)
    {
        _smoke = smoke;
        InitializeComponent();
    }

    /// <summary>The core, one per process. Disposed when the main window closes.</summary>
    internal static CoreHost? Core { get; private set; }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _ui = DispatcherQueue.GetForCurrentThread();
        if (_smoke)
        {
            _ = SmokeRun.RunAndExitAsync(_ui);
            return;
        }

        Core = CoreHost.Create();
        _window = new MainWindow();
        _window.Closed += (_, _) =>
        {
            Core?.Dispose();
            Core = null;
        };
        _window.Activate();
    }

    /// <summary>A later launch was redirected here: show the main window. Called on any thread.</summary>
    internal static void BringToFront() => _ui?.TryEnqueue(() =>
    {
        if (_window is null)
        {
            return;
        }
        if (_window.AppWindow.Presenter is OverlappedPresenter { State: OverlappedPresenterState.Minimized } presenter)
        {
            presenter.Restore();
        }
        _window.Activate();
    });
}
