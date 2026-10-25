using System.Diagnostics;
using Current.Bindings;
using Microsoft.Graphics.Display;
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
/// </summary>
internal sealed class DisplayWatcher : IDisposable
{
    private readonly Window _window;
    private readonly IntPtr _hwnd;
    private DisplayInformation? _display;
    private IntPtr _monitor;
    private bool _disposed;

    public DisplayWatcher(Window window)
    {
        _window = window;
        _hwnd = WinRT.Interop.WindowNative.GetWindowHandle(window);
        _window.AppWindow.Changed += OnWindowChanged;
        Attach();
        Facts = Read();
    }

    public DisplayFacts Facts { get; private set; }

    public event EventHandler? Changed;

    /// <summary>
    /// A DisplayInformation for the monitor the window is on now. Created again when the window changes monitor, rather than relying on
    /// one object following the window (whether it does was not verified).
    /// </summary>
    private void Attach()
    {
        Detach();
        _monitor = PlayerNative.MonitorOf(_hwnd);
        try
        {
            _display = DisplayInformation.CreateForWindowId(_window.AppWindow.Id);
            _display.AdvancedColorInfoChanged += OnColourChanged;
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: reading the display failed: {error.GetType().Name}");
            _display = null;
        }
    }

    private void Detach()
    {
        if (_display is not null)
        {
            _display.AdvancedColorInfoChanged -= OnColourChanged;
            _display.Dispose();
            _display = null;
        }
    }

    private DisplayFacts Read()
    {
        var hdr = false;
        float peak = 0, white = 0;
        try
        {
            if (_display?.GetAdvancedColorInfo() is { } colour)
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
        return new DisplayFacts(hdr, peak, white, PlayerNative.RefreshRate(_monitor));
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
            Attach();
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
        Detach();
    }
}
