using System.Diagnostics;
using System.Globalization;
using Current.Bindings;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell;

public partial class App : Application
{
    private readonly bool _smoke;
    private static DispatcherQueue? _ui;
    private static MainWindow? _window;
    private static NetworkObserver? _network;
    private static bool _closed;
    private static readonly TaskCompletionSource<CoreHost?> CoreOpened = new(TaskCreationOptions.RunContinuationsAsynchronously);

    /// <summary>
    /// How long the first showing of the window waits for the core, so the saved app theme is applied before anything is drawn rather
    /// than switching after. Opening reads one small file; a slower open shows the window in the system's theme and switches once it
    /// is open.
    /// </summary>
    private static readonly TimeSpan ThemeWait = TimeSpan.FromMilliseconds(300);

    public App(bool smoke)
    {
        _smoke = smoke;
        InitializeComponent();
        // An exception escaping a UI event handler would end the process. Each handler catches what it expects; this is the backstop,
        // which logs and keeps the app running (Application.UnhandledException, Handled).
        UnhandledException += (_, args) =>
        {
            // The type only, as everywhere: a message could carry a server's answer.
            Trace.TraceError($"Current: unhandled exception on the UI thread: {args.Exception.GetType().Name}");
            args.Handled = true;
        };
    }

    /// <summary>
    /// The core, one per process, once it is open; null before and after. Disposed when the main window closes. The smoke run sets it to
    /// its own core for its browse check.
    /// </summary>
    internal static CoreHost? Core { get; set; }

    /// <summary>The main window's browse sessions (ADR 0018), once the core is open; null before, after, and in the smoke run.</summary>
    internal static Browsing.BrowseHub? Browsing { get; set; }

    /// <summary>Completes on the UI thread when the core is open, with null if it could not start at all.</summary>
    internal static Task<CoreHost?> CoreReady => CoreOpened.Task;

    // async void, as event handlers are: it catches everything itself.
    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        _ui = DispatcherQueue.GetForCurrentThread();
        if (_smoke)
        {
            _ = SmokeRun.RunAndExitAsync(_ui);
            return;
        }

        // The window shows as soon as the core is open (its app theme applied) or after ThemeWait; its pages wait for CoreReady.
        _window = new MainWindow();
        _window.Closed += (_, _) =>
        {
            _closed = true;
            // A sign-in window talks to the core, which goes with this window; open, it would also keep the app running.
            Gateway.SignInWindow.CloseAll();
            Browsing?.Dispose();
            Browsing = null;
            _network?.Dispose();
            _network = null;
            Core?.Dispose();
            Core = null;
        };
        _ = OpenCoreAsync(_ui);
        // Sign-in profiles an earlier run could not delete (it ended with a sign-in window open).
        _ = Task.Run(Gateway.SignInProfiles.DeleteLeftovers);
        try
        {
            await Task.WhenAny(CoreReady, Task.Delay(ThemeWait));
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: waiting for the core failed: {error.GetType().Name}");
        }
        _window.Activate();
    }

    /// <summary>A notice above every page: closed by the user. Called on the UI thread; dropped once the window has closed.</summary>
    internal static void ShowNotice(InfoBarSeverity severity, string title, string message)
    {
        if (!_closed)
        {
            _window?.ShowNotice(severity, title, message);
        }
    }

    /// <summary>Has Narrator speak <paramref name="text"/> from the window, for a result that outlives the page (Announce.Say).</summary>
    internal static void Announce(string text, string activityId)
    {
        if (!_closed)
        {
            _window?.Announce(text, activityId);
        }
    }

    /// <summary>Plays what a Play button names, in the main window (ADR 0019 task 5). Called on the UI thread.</summary>
    internal static void Play(PlayTarget target, bool fromStart)
    {
        if (!_closed)
        {
            _window?.Play(target, fromStart);
        }
    }

    /// <summary>Drops the back and forward history (after a finished sign-in). Called on the UI thread.</summary>
    internal static void ClearNavigationHistory() => _window?.ClearHistory();

    /// <summary>
    /// Opens the core on the data and cache folders (ADR 0013), off the UI thread inside the adapter, then reports the network and restores the
    /// saved sessions in the background (ADR 0015, "Restore").
    /// </summary>
    private static async Task OpenCoreAsync(DispatcherQueue ui)
    {
        CoreHost? core = null;
        try
        {
            core = await CoreHost.OpenAsync(AppIdentity.DataFolder, AppIdentity.CacheFolder, DeviceName.Get(), SecretStore.Keyring);
        }
        catch (CoreStoreException error)
        {
            // Keep the app usable: a core that stores nothing, and say so (manager, 2026-10-23).
            ShowNotice(
                InfoBarSeverity.Error,
                "Current couldn't open its settings folder.",
                $"Sign-ins won't be saved this time. {ProblemText.Store(error.Problem)}");
            core = TryCreateInMemory();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: opening the core failed: {error.GetType().Name}");
            ShowNotice(InfoBarSeverity.Error, "Current couldn't open its settings folder.", "Sign-ins won't be saved this time.");
            core = TryCreateInMemory();
        }

        if (core is not null && !_closed)
        {
            try
            {
                ShowLoadReport(core.LoadReport);
            }
            catch (Exception error)
            {
                Trace.TraceError($"Current: reading the load report failed: {error.GetType().Name}");
            }
        }
        if (core is not null && _closed)
        {
            // The window closed while the core was opening.
            core.Dispose();
            core = null;
        }
        // The region's decimal separator for the core's labels with decimals ("26,1 Mbit/s"), as the app wrote numbers before: in
        // the current culture.
        core?.SetDecimalSeparator(CultureInfo.CurrentCulture.NumberFormat.NumberDecimalSeparator);
        Core = core;
        if (core is not null)
        {
            try
            {
                _window?.FollowAppTheme(core);
            }
            catch (Exception error)
            {
                Trace.TraceError($"Current: following the app theme failed: {error.GetType().Name}");
            }
            // Before CoreReady completes, so pages waiting for the core find their browse sessions.
            try
            {
                Browsing = new Browsing.BrowseHub(core, new DispatcherQueueUi(ui));
                _window?.FollowBrowsing(Browsing);
            }
            catch (Exception error)
            {
                Trace.TraceError($"Current: starting browsing failed: {error.GetType().Name}");
            }
        }
        CoreOpened.TrySetResult(core);
        if (core is null)
        {
            return;
        }

        _network = new NetworkObserver(core, ui);
        _network.Start();
        try
        {
            await core.RestoreSessionsAsync();
        }
        catch (Exception error)
        {
            // The capabilities state shows each server's outcome; a failure of the call itself is only traced.
            Trace.TraceError($"Current: restoring sessions failed: {error.GetType().Name}");
        }
    }

    private static CoreHost? TryCreateInMemory()
    {
        try
        {
            return CoreHost.Create();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the core could not start: {error.GetType().Name}");
            return null;
        }
    }

    /// <summary>Only what the user should know: settings that were reset or that belong to a newer version.</summary>
    private static void ShowLoadReport(LoadReport? report)
    {
        switch (report)
        {
            case LoadReport.ResetCorrupt reset:
                ShowNotice(
                    InfoBarSeverity.Warning,
                    "Your settings couldn't be read and were reset.",
                    $"The old file was kept as {reset.BackupFile} in Current's data folder.");
                break;
            case LoadReport.NewerVersion:
                ShowNotice(
                    InfoBarSeverity.Warning,
                    "These settings are from a newer version of Current.",
                    "Update Current to sign in or save changes.");
                break;
        }
    }

    /// <summary>The main window came back to the foreground: unreachable servers try again at once (ADR 0014).</summary>
    internal static void ReturnedToForeground()
    {
        try
        {
            Core?.ReturnedToForeground();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: returned-to-foreground failed: {error.GetType().Name}");
        }
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
