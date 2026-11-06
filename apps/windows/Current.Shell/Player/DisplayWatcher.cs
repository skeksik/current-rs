using System.Diagnostics;
using Current.Bindings;
using Microsoft.Graphics.Display;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;

namespace Current.Shell.Player;

/// <summary>
/// The facts of the monitor a window is on, for the engine's colour and timing options (ADR 0007, "The UI reports the display; the
/// engine decides the colour output"): HDR or SDR, peak and SDR white luminance from <c>DisplayInformation.CreateForWindowId</c> and
/// <c>GetAdvancedColorInfo</c> (Microsoft.Graphics.Display, Windows App SDK InteractiveExperiences 2.1.9, as the D2 spike read them), and
/// the refresh rate from the display path (PlayerNative.RefreshRate). <see cref="Changed"/> fires on the UI thread when they change:
/// the window moved to another monitor (<c>AppWindow.Changed</c>, ADR 0007 item 3) or the monitor's colour mode changed
/// (<c>AdvancedColorInfoChanged</c>).
/// <para>
/// One <c>DisplayInformation</c> per window, made once on the UI thread and kept until the window is destroyed (<see cref="For"/>);
/// it is never closed while the window lives. The watcher used to close its object and make a new one on every monitor change (and
/// one per player). Then a window moved across monitors while a player played, after an earlier player had closed, crashed inside
/// <c>Microsoft.Graphics.Display.dll</c> (W7 §7, docs/notes/windows-native-player-check.md) while the window handled a move: two
/// dumps with an access violation at the same offset reading a small bad address, called from <c>Microsoft.UI.Windowing.Core.dll</c>,
/// and one fail-fast in the same code. Reproduced 2 of 2 tries on the owner's PC, never with this change. Why the DLL reads a bad
/// address is not known; an object closed earlier that the window's messages still reach is inferred. The object reports the monitor
/// the window is on now ("the monitor corresponding to your window", AdvancedColorInfoChanged, learn.microsoft.com, updated
/// 2026-07-28, read 2026-10-31; seen on the owner's PC), so no new one is needed after a move.
/// </para>
/// </summary>
internal sealed class DisplayWatcher : IDisposable
{
    /// <summary>The window's DisplayInformation, by window; UI thread only.</summary>
    private static readonly Dictionary<WindowId, DisplayInformation?> Shared = [];

    private readonly Window _window;
    private readonly IntPtr _hwnd;
    private readonly DisplayInformation? _display;
    private IntPtr _monitor;
    private bool _disposed;

    public DisplayWatcher(Window window)
    {
        _window = window;
        _hwnd = WinRT.Interop.WindowNative.GetWindowHandle(window);
        _window.AppWindow.Changed += OnWindowChanged;
        _monitor = PlayerNative.MonitorOf(_hwnd);
        _display = For(window.AppWindow.Id);
        if (_display is not null)
        {
            _display.AdvancedColorInfoChanged += OnColourChanged;
        }
        Facts = Read();
    }

    public DisplayFacts Facts { get; private set; }

    public event EventHandler? Changed;

    /// <summary>
    /// The window's DisplayInformation, made on first use and closed when the window is destroyed; null when it cannot be made (tried
    /// once per window). On the UI thread: <c>CreateForWindowId</c> needs a thread with a running DispatcherQueue (DisplayInformation
    /// class, learn.microsoft.com, updated 2026-07-28, read 2026-10-31).
    /// </summary>
    private static DisplayInformation? For(WindowId window)
    {
        if (Shared.TryGetValue(window, out var known))
        {
            return known;
        }
        DisplayInformation? display = null;
        try
        {
            display = DisplayInformation.CreateForWindowId(window);
            if (AppWindow.GetFromWindowId(window) is { } appWindow)
            {
                appWindow.Destroying += (_, _) =>
                {
                    Shared.Remove(window);
                    display.Dispose();
                };
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: reading the display failed: {error.GetType().Name}");
        }
        Shared[window] = display;
        return display;
    }

    private DisplayFacts Read() => Read(_display, _monitor);

    /// <summary>
    /// The facts of the monitor the window <paramref name="window"/> names is on, read once (Settings' "Supported formats", ADR 0021);
    /// null when they cannot be read.
    /// </summary>
    public static DisplayFacts? ReadOnce(WindowId window)
    {
        try
        {
            return Read(For(window), PlayerNative.MonitorOf(Win32Interop.GetWindowFromWindowId(window)));
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: reading the display failed: {error.GetType().Name}");
            return null;
        }
    }

    private static DisplayFacts Read(DisplayInformation? display, IntPtr monitor)
    {
        var hdr = false;
        float peak = 0, white = 0;
        try
        {
            if (display?.GetAdvancedColorInfo() is { } colour)
            {
                hdr = colour.CurrentAdvancedColorKind == DisplayAdvancedColorKind.HighDynamicRange;
                peak = (float)colour.MaxLuminanceInNits;
                white = (float)colour.SdrWhiteLevelInNits;
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: reading the display's colour failed: {error.GetType().Name}");
        }
        return new DisplayFacts(hdr, peak, white, PlayerNative.RefreshRate(monitor));
    }

    private void Refresh()
    {
        if (_disposed)
        {
            return;
        }
        var facts = Read();
        if (facts != Facts)
        {
            Facts = facts;
            Changed?.Invoke(this, EventArgs.Empty);
        }
    }

    private void OnWindowChanged(AppWindow sender, AppWindowChangedEventArgs args)
    {
        if ((args.DidPositionChange || args.DidSizeChange || args.DidPresenterChange) && PlayerNative.MonitorOf(_hwnd) != _monitor)
        {
            // The same DisplayInformation follows the window; its colour change, if it comes later, refreshes again.
            _monitor = PlayerNative.MonitorOf(_hwnd);
            Refresh();
        }
    }

    // Raised on the thread the object was made on, the UI thread (not verified): queued there either way.
    private void OnColourChanged(DisplayInformation sender, object args) => _window.DispatcherQueue.TryEnqueue(Refresh);

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }
        _disposed = true;
        _window.AppWindow.Changed -= OnWindowChanged;
        // The handler only: the window keeps its DisplayInformation (the class summary).
        if (_display is not null)
        {
            _display.AdvancedColorInfoChanged -= OnColourChanged;
        }
    }
}
