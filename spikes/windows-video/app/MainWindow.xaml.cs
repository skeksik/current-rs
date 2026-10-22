using System.Diagnostics;
using System.Text;
using System.Text.Json;
using Microsoft.Graphics.Display;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.Media;
using Windows.Storage;
using Windows.Storage.Streams;
using VirtualKey = Windows.System.VirtualKey;

namespace VideoSpike;

public sealed partial class MainWindow : Window
{
    readonly Options _opts;
    readonly IntPtr _hwnd;
    readonly DispatcherQueue _ui;
    Mpv? _mpv;
    StreamWriter? _log;

    // Swap chain from mpv's display-swapchain property (mpv owns it; we hold one reference while the panel shows it).
    IntPtr _swapChain;
    (int W, int H) _compositionSize;
    float _scaleX = 1, _scaleY = 1;

    DisplayInformation? _display;
    string _colourMode;
    string _displaySummary = "";
    IntPtr _monitor;

    double _duration, _position;
    bool _paused = true, _updatingSeekBar, _pointerOverControls, _fullScreen;
    string _hwdec = "", _videoParams = "", _targetParams = "", _title = "";
    readonly DispatcherQueueTimer _hideTimer, _infoTimer, _swapChainPoll;
    long _lastHideRequest;

    SystemMediaTransportControls? _smtc;
    DateTime _lastTimelineUpdate;

    public MainWindow(Options opts)
    {
        _opts = opts;
        _colourMode = opts.Colour;
        InitializeComponent();
        _ui = DispatcherQueue;
        _hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Tall;
        AppWindow.SetIcon(null as string);
        if (opts.Size is { } size && size.Split('x') is [var w, var h])
            AppWindow.Resize(new Windows.Graphics.SizeInt32(int.Parse(w), int.Parse(h)));
        if (opts.Pos is { } pos && pos.Split(',') is [var x, var y])
            AppWindow.Move(new Windows.Graphics.PointInt32(int.Parse(x), int.Parse(y)));
        AppWindow.Changed += AppWindow_Changed;
        Closed += MainWindow_Closed;

        Directory.CreateDirectory(_opts.DataDir);
        _log = new StreamWriter(Path.Combine(_opts.DataDir, $"mpv-{DateTime.Now:yyyyMMdd-HHmmss}.log"), false, Encoding.UTF8)
            { AutoFlush = true };

        _hideTimer = _ui.CreateTimer();
        _hideTimer.Interval = TimeSpan.FromSeconds(3);
        _hideTimer.IsRepeating = false;
        _hideTimer.Tick += (_, _) => HideControlsIfIdle();
        _infoTimer = _ui.CreateTimer();
        _infoTimer.Interval = TimeSpan.FromMilliseconds(500);
        _infoTimer.Tick += (_, _) => UpdateInfo();
        _swapChainPoll = _ui.CreateTimer();
        _swapChainPoll.Interval = TimeSpan.FromMilliseconds(50);
        _swapChainPoll.Tick += (_, _) => AttachSwapChain();

        VideoPanel.Loaded += (_, _) => StartPlayer();
    }

    void Log(string line) { lock (this) _log?.WriteLine($"{DateTime.Now:HH:mm:ss.fff} {line}"); }

    // ------------------------------------------------------------------ engine start
    void StartPlayer()
    {
        _scaleX = VideoPanel.CompositionScaleX;
        _scaleY = VideoPanel.CompositionScaleY;
        _compositionSize = PixelSize();
        _display = DisplayInformation.CreateForWindowId(AppWindow.Id);
        _display.AdvancedColorInfoChanged += (_, _) => _ui.TryEnqueue(UpdateDisplay);

        _mpv = new Mpv();
        Log($"libmpv client API {Mpv.ApiVersion}");
        // Engine options the Windows UI must set (bridge record in docs/notes/windows-video.md).
        foreach (var (k, v) in new[]
        {
            ("vo", "gpu-next"), ("gpu-api", "d3d11"), ("gpu-context", "d3d11"),
            ("d3d11-output-mode", "composition"),
            ("d3d11-composition-size", $"{_compositionSize.W}x{_compositionSize.H}"),
            ("hwdec", "auto-safe"), ("force-window", "yes"), ("idle", "yes"), ("keep-open", "yes"),
            ("osc", "no"), ("input-default-bindings", "no"), ("input-vo-keyboard", "no"), ("ytdl", "no"),
            ("terminal", "no"), ("sub-auto", "no"), ("audio-file-auto", "no"), ("screenshot-directory", _opts.DataDir),
        })
        {
            var r = _mpv.SetOption(k, v);
            if (r < 0) Log($"option {k}={v}: {Mpv.Error(r)}");
        }
        if (_opts.Loop) _mpv.SetOption("loop-file", "inf");
        _mpv.Log += (level, prefix, text) => Log($"[{level}] {prefix}: {text.TrimEnd()}");
        _mpv.PropertyChanged += (name, value) => _ui.TryEnqueue(() => OnProperty(name, value));
        _mpv.Event += (id, code) => _ui.TryEnqueue(() => OnEvent(id, code));
        _mpv.RequestLog("v");
        _mpv.Initialize();

        _mpv.Observe("pause", Mpv.Format.Flag);
        _mpv.Observe("time-pos", Mpv.Format.Double);
        _mpv.Observe("duration", Mpv.Format.Double);
        _mpv.Observe("track-list", Mpv.Format.String); // node printed as JSON
        _mpv.Observe("hwdec-current", Mpv.Format.String);
        _mpv.Observe("video-params", Mpv.Format.String);
        _mpv.Observe("video-target-params", Mpv.Format.String);
        _mpv.Observe("media-title", Mpv.Format.String);

        UpdateDisplay();
        SetupSmtc();
        _swapChainPoll.Start();
        _infoTimer.Start();
        ShowControls();

        if (_opts.FullScreen) SetFullScreen(true);
        if (_opts.File is { } file) Load(file);
    }

    (int W, int H) PixelSize() =>
        (Math.Max(1, (int)Math.Round(VideoPanel.ActualWidth * VideoPanel.CompositionScaleX)),
         Math.Max(1, (int)Math.Round(VideoPanel.ActualHeight * VideoPanel.CompositionScaleY)));

    // ------------------------------------------------------------------ swap chain
    void AttachSwapChain()
    {
        if (_mpv == null) return;
        var ptr = (IntPtr)(_mpv.GetInt64("display-swapchain") ?? 0);
        if (ptr == _swapChain) { if (ptr != IntPtr.Zero) _swapChainPoll.Stop(); return; }
        if (_swapChain != IntPtr.Zero) System.Runtime.InteropServices.Marshal.Release(_swapChain);
        _swapChain = ptr;
        if (ptr == IntPtr.Zero) { Native.SetSwapChain(VideoPanel, IntPtr.Zero); return; }
        // mpv's property is a borrowed pointer: take our own reference before mpv can drop its VO.
        System.Runtime.InteropServices.Marshal.AddRef(ptr);
        Native.SetSwapChain(VideoPanel, ptr);
        Native.SetMatrixTransform(ptr, _scaleX, _scaleY);
        Log($"attached swap chain 0x{ptr:X}: {Native.GetDesc1(ptr)}");
        _swapChainPoll.Stop();
    }

    void VideoPanel_SizeChanged(object sender, SizeChangedEventArgs e) => UpdateCompositionSize();

    void VideoPanel_CompositionScaleChanged(SwapChainPanel sender, object args)
    {
        _scaleX = sender.CompositionScaleX;
        _scaleY = sender.CompositionScaleY;
        Log($"composition scale {_scaleX}x{_scaleY}");
        if (_swapChain != IntPtr.Zero) Native.SetMatrixTransform(_swapChain, _scaleX, _scaleY);
        UpdateCompositionSize();
    }

    void UpdateCompositionSize()
    {
        var size = PixelSize();
        if (size == _compositionSize || _mpv == null) return;
        _compositionSize = size;
        // Asynchronous: mpv resizes the buffers on its VO thread (VOCTRL_EXTERNAL_RESIZE).
        _mpv.SetPropertyAsync("d3d11-composition-size", $"{size.W}x{size.H}");
    }

    // ------------------------------------------------------------------ display and colour
    void AppWindow_Changed(AppWindow sender, AppWindowChangedEventArgs args)
    {
        if (args.DidPositionChange || args.DidSizeChange)
        {
            var m = Native.MonitorFromWindow(_hwnd, 2);
            if (m != _monitor) UpdateDisplay();
        }
        if (args.DidPresenterChange)
        {
            _fullScreen = AppWindow.Presenter.Kind == AppWindowPresenterKind.FullScreen;
            AppTitleBar.Visibility = Toolbar.Visibility = _fullScreen ? Visibility.Collapsed : Visibility.Visible;
            FullScreenIcon.Glyph = _fullScreen ? "" : "";
            AutomationName(FullScreenButton, _fullScreen ? "Exit full screen" : "Full screen");
        }
    }

    void UpdateDisplay()
    {
        if (_display == null || _mpv == null) return;
        _monitor = Native.MonitorFromWindow(_hwnd, 2);
        var info = _display.GetAdvancedColorInfo();
        var dxgi = Native.OutputForMonitor(_monitor);
        _displaySummary = $"display: {info.CurrentAdvancedColorKind} max {info.MaxLuminanceInNits:0} nit, SDR white {info.SdrWhiteLevelInNits:0} nit";
        if (dxgi is { } o)
        {
            var hz = Native.RefreshRate(o.Desc.DeviceName);
            _displaySummary += $"\n  dxgi: {o.Desc.DeviceName} on {o.Adapter}, {Native.ColorSpaceName(o.Desc.ColorSpace)}, " +
                               $"{o.Desc.BitsPerColor} bpc, max {o.Desc.MaxLuminance:0} nit, {hz} Hz";
            // Composition mode has no output to query, so mpv's display-fps stays unknown unless the UI passes it in.
            if (hz > 0) _mpv.SetProperty("display-fps-override", hz.ToString());
        }
        Log(_displaySummary);
        ApplyColour(info);
    }

    // mpv cannot see the monitor in composition mode (its d3d11 target_csp returns nothing), so the UI passes it in.
    void ApplyColour(DisplayAdvancedColorInfo info)
    {
        if (_mpv == null) return;
        var hdr = info.CurrentAdvancedColorKind == DisplayAdvancedColorKind.HighDynamicRange;
        var peak = info.MaxLuminanceInNits > 0 ? $"{info.MaxLuminanceInNits:0}" : "auto";
        var white = info.SdrWhiteLevelInNits > 0 ? $"{info.SdrWhiteLevelInNits:0}" : "auto";
        var set = _colourMode switch
        {
            "auto" when hdr => new[] { ("target-colorspace-hint", "yes"), ("target-colorspace-hint-mode", "target"),
                ("target-trc", "pq"), ("target-prim", "bt.2020"), ("target-peak", peak), ("hdr-reference-white", white) },
            "source" => new[] { ("target-colorspace-hint", "yes"), ("target-colorspace-hint-mode", "source"),
                ("target-trc", "auto"), ("target-prim", "auto"), ("target-peak", "auto"), ("hdr-reference-white", white) },
            "scrgb" => new[] { ("target-colorspace-hint", "yes"), ("target-colorspace-hint-mode", "target"),
                ("target-trc", "scrgb"), ("target-prim", "bt.709"), ("target-peak", peak), ("hdr-reference-white", white) },
            _ => new[] { ("target-colorspace-hint", "no"), ("target-colorspace-hint-mode", "target"),
                ("target-trc", "auto"), ("target-prim", "auto"), ("target-peak", "auto"), ("hdr-reference-white", "auto") },
        };
        foreach (var (k, v) in set)
        {
            var r = _mpv.SetProperty(k, v);
            Log($"set {k}={v}{(r < 0 ? " -> " + Mpv.Error(r) : "")}");
        }
        ColourButton.Content = $"Colour: {_colourMode}";
    }

    void Colour_Click(object sender, RoutedEventArgs e)
    {
        _colourMode = (string)((FrameworkElement)sender).Tag;
        UpdateDisplay();
    }

    // ------------------------------------------------------------------ engine events
    void OnProperty(string name, object? value)
    {
        switch (name)
        {
            case "pause":
                _paused = value as bool? ?? true;
                PlayPauseIcon.Glyph = _paused ? "" : "";
                AutomationName(PlayPauseButton, _paused ? "Play" : "Pause");
                ToolTipService.SetToolTip(PlayPauseButton, _paused ? "Play (Space)" : "Pause (Space)");
                if (_smtc != null)
                    _smtc.PlaybackStatus = _paused ? MediaPlaybackStatus.Paused : MediaPlaybackStatus.Playing;
                UpdateTimeline(force: true);
                break;
            case "time-pos":
                _position = value as double? ?? 0;
                PositionText.Text = Time(_position);
                _updatingSeekBar = true;
                SeekBar.Value = Math.Min(_position, SeekBar.Maximum);
                _updatingSeekBar = false;
                UpdateTimeline(force: false);
                break;
            case "duration":
                _duration = value as double? ?? 0;
                SeekBar.Maximum = Math.Max(1, _duration);
                DurationText.Text = Time(_duration);
                UpdateTimeline(force: true);
                break;
            case "track-list":
                BuildTrackMenus(value as string);
                break;
            case "hwdec-current": _hwdec = value as string ?? ""; break;
            case "video-params": _videoParams = value as string ?? ""; Log($"video-params {_videoParams}"); break;
            case "video-target-params": _targetParams = value as string ?? ""; Log($"video-target-params {_targetParams}"); break;
            case "media-title":
                _title = value as string ?? "";
                UpdateSmtcDisplay();
                break;
        }
    }

    void OnEvent(Mpv.EventId id, int code)
    {
        Log($"event {id} {code}");
        switch (id)
        {
            case Mpv.EventId.VideoReconfig:
            case Mpv.EventId.FileLoaded:
                AttachSwapChain();
                if (_swapChain != IntPtr.Zero) Log($"swap chain now: {Native.GetDesc1(_swapChain)}");
                break;
            case Mpv.EventId.PlaybackRestart:
                UpdateTimeline(force: true); // after every seek, or the flyout shows a stale position
                if (_opts.MeasureSeconds > 0 && !_measureStarted) { _measureStarted = true; _ = Measure(_opts.MeasureSeconds, _opts.Exit); }
                if (_opts.Transitions && !_measureStarted) { _measureStarted = true; _ = Transitions(); }
                break;
        }
    }

    bool _measureStarted;

    // ------------------------------------------------------------------ transport
    void Load(string pathOrUrl)
    {
        _mpv?.CommandAsync("loadfile", pathOrUrl);
        _mpv?.SetPropertyAsync("pause", "no");
    }

    async void Open_Click(object sender, RoutedEventArgs e)
    {
        var picker = new Windows.Storage.Pickers.FileOpenPicker();
        WinRT.Interop.InitializeWithWindow.Initialize(picker, _hwnd);
        foreach (var ext in new[] { ".mkv", ".mp4", ".m2ts", ".ts", ".webm", ".mov" }) picker.FileTypeFilter.Add(ext);
        var file = await picker.PickSingleFileAsync();
        if (file != null) Load(file.Path);
    }

    void PlayUrl_Click(object sender, RoutedEventArgs e)
    {
        // The URL is typed at runtime and never logged or stored (it may carry a token).
        if (Uri.TryCreate(UrlBox.Text.Trim(), UriKind.Absolute, out var uri)) { Load(uri.ToString()); UrlBox.Text = ""; }
    }

    void UrlBox_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Enter) { PlayUrl_Click(sender, e); e.Handled = true; }
    }

    void TogglePause() => _mpv?.CommandAsync("cycle", "pause");
    void SeekRelative(double s) => _mpv?.CommandAsync("seek", s.ToString(System.Globalization.CultureInfo.InvariantCulture), "relative");

    void PlayPause_Click(object sender, RoutedEventArgs e) => TogglePause();
    void Back_Click(object sender, RoutedEventArgs e) => SeekRelative(-10);
    void Forward_Click(object sender, RoutedEventArgs e) => SeekRelative(30);

    void SeekBar_ValueChanged(object sender, Microsoft.UI.Xaml.Controls.Primitives.RangeBaseValueChangedEventArgs e)
    {
        if (_updatingSeekBar || _mpv == null) return;
        _mpv.CommandAsync("seek", e.NewValue.ToString(System.Globalization.CultureInfo.InvariantCulture), "absolute");
    }

    void BuildTrackMenus(string? json)
    {
        AudioMenu.Items.Clear();
        SubtitleMenu.Items.Clear();
        var off = new RadioMenuFlyoutItem { Text = "Off", GroupName = "sub", IsChecked = true };
        off.Click += (_, _) => _mpv?.SetPropertyAsync("sid", "no");
        SubtitleMenu.Items.Add(off);
        if (string.IsNullOrEmpty(json)) return;
        foreach (var t in JsonDocument.Parse(json).RootElement.EnumerateArray())
        {
            var type = t.GetProperty("type").GetString();
            if (type is not ("audio" or "sub")) continue;
            var id = t.GetProperty("id").GetInt64();
            var label = string.Join(" - ", new[]
            {
                t.TryGetProperty("title", out var ti) ? ti.GetString() : null,
                t.TryGetProperty("lang", out var la) ? la.GetString() : null,
                t.TryGetProperty("codec", out var co) ? co.GetString() : null,
            }.Where(s => !string.IsNullOrEmpty(s)));
            var item = new RadioMenuFlyoutItem
            {
                Text = label.Length > 0 ? label : $"Track {id}", GroupName = type,
                IsChecked = t.TryGetProperty("selected", out var sel) && sel.GetBoolean(),
            };
            var prop = type == "audio" ? "aid" : "sid";
            item.Click += (_, _) => _mpv?.SetPropertyAsync(prop, id.ToString());
            if (type == "audio") AudioMenu.Items.Add(item);
            else { if (item.IsChecked) off.IsChecked = false; SubtitleMenu.Items.Add(item); }
        }
    }

    // ------------------------------------------------------------------ full screen, controls, keyboard
    void SetFullScreen(bool on) =>
        AppWindow.SetPresenter(on ? AppWindowPresenterKind.FullScreen : AppWindowPresenterKind.Overlapped);

    void FullScreen_Click(object sender, RoutedEventArgs e) => SetFullScreen(!_fullScreen);
    void Player_DoubleTapped(object sender, DoubleTappedRoutedEventArgs e) => SetFullScreen(!_fullScreen);

    void Root_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        ShowControls();
        switch (e.Key)
        {
            case VirtualKey.Space: TogglePause(); break;
            case VirtualKey.Left: SeekRelative(-10); break;
            case VirtualKey.Right: SeekRelative(30); break;
            case VirtualKey.F11: case VirtualKey.F: SetFullScreen(!_fullScreen); break;
            case VirtualKey.Escape when _fullScreen: SetFullScreen(false); break;
            case VirtualKey.I: InfoToggle.IsChecked = !InfoToggle.IsChecked; InfoToggle_Click(InfoToggle, e); break;
            default: return;
        }
        e.Handled = true;
    }

    void Player_PointerMoved(object sender, PointerRoutedEventArgs e) => ShowControls();
    void Controls_PointerEntered(object sender, PointerRoutedEventArgs e) => _pointerOverControls = true;
    void Controls_PointerExited(object sender, PointerRoutedEventArgs e) => _pointerOverControls = false;

    void ShowControls()
    {
        Controls.Visibility = Visibility.Visible;
        Controls.Opacity = 1;
        _lastHideRequest = Environment.TickCount64;
        _hideTimer.Stop();
        _hideTimer.Start();
    }

    void HideControlsIfIdle()
    {
        // Keep them while the pointer is over them, a menu is open, or nothing is playing.
        var menuOpen = AudioMenu.IsOpen || SubtitleMenu.IsOpen;
        if (_pointerOverControls || menuOpen || _paused) { _hideTimer.Start(); return; }
        Controls.Visibility = Visibility.Collapsed;
        if (FocusManager.GetFocusedElement(Content.XamlRoot) is DependencyObject f && IsInside(f, Controls))
            VideoPanelFocusFallback();
    }

    void VideoPanelFocusFallback() => Root.Focus(FocusState.Programmatic);

    static bool IsInside(DependencyObject d, DependencyObject parent)
    {
        for (var p = d; p != null; p = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetParent(p)) if (p == parent) return true;
        return false;
    }

    void InfoToggle_Click(object sender, RoutedEventArgs e) =>
        InfoPanel.Visibility = InfoToggle.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;

    static void AutomationName(UIElement e, string name) =>
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(e, name);

    static string Time(double s) =>
        s >= 3600 ? TimeSpan.FromSeconds(s).ToString(@"h\:mm\:ss") : TimeSpan.FromSeconds(s).ToString(@"m\:ss");

    void UpdateInfo()
    {
        if (_mpv == null || InfoPanel.Visibility != Visibility.Visible) return;
        var sb = new StringBuilder();
        sb.AppendLine($"hwdec: {_hwdec}   composition: {_compositionSize.W}x{_compositionSize.H} @ scale {_scaleX:0.##}");
        if (_swapChain != IntPtr.Zero) sb.AppendLine($"swap chain: {Native.GetDesc1(_swapChain)}");
        sb.AppendLine(_displaySummary);
        sb.AppendLine($"colour mode: {_colourMode}");
        sb.AppendLine($"video: {Brief(_videoParams)}");
        sb.AppendLine($"target: {Brief(_targetParams)}");
        sb.Append($"drops: vo {_mpv.GetInt64("frame-drop-count")}  dec {_mpv.GetInt64("decoder-frame-drop-count")}  " +
                  $"delayed {_mpv.GetInt64("vo-delayed-frame-count")}  mistimed {_mpv.GetInt64("mistimed-frame-count")}  " +
                  $"display {_mpv.GetDouble("display-fps"):0.##} fps  est {_mpv.GetDouble("estimated-vf-fps"):0.##} fps");
        InfoText.Text = sb.ToString();
    }

    static string Brief(string json)
    {
        if (string.IsNullOrEmpty(json)) return "-";
        try
        {
            var r = JsonDocument.Parse(json).RootElement;
            string P(string k) => r.TryGetProperty(k, out var v) ? v.ToString() : "?";
            return $"{P("w")}x{P("h")} {P("hw-pixelformat")}/{P("pixelformat")} {P("primaries")} {P("gamma")} " +
                   $"peak {P("sig-peak")} max-luma {P("max-luma")} max-cll {P("max-cll")}";
        }
        catch (JsonException) { return json; }
    }

    // ------------------------------------------------------------------ measurement (case 1)
    async void Measure_Click(object sender, RoutedEventArgs e) => await Measure(60, exit: false);

    async Task Measure(int seconds, bool exit)
    {
        if (_mpv == null) return;
        MeasureButton.IsEnabled = false;
        await Task.Delay(3000); // warm-up after start or seek
        var proc = Process.GetCurrentProcess();
        string[] counters = { "frame-drop-count", "decoder-frame-drop-count", "vo-delayed-frame-count", "mistimed-frame-count" };
        var start = counters.ToDictionary(c => c, c => _mpv.GetInt64(c) ?? 0);
        var cpu0 = proc.TotalProcessorTime;
        var sw = Stopwatch.StartNew();
        var pos0 = _position;
        for (int i = seconds; i > 0; i--) { MeasureButton.Content = $"Measuring {i} s"; await Task.Delay(1000); }
        sw.Stop();
        proc.Refresh();
        var cpu = (proc.TotalProcessorTime - cpu0).TotalSeconds / sw.Elapsed.TotalSeconds / Environment.ProcessorCount * 100;
        var result = new Dictionary<string, object?>
        {
            ["file"] = Path.GetFileName(_mpv.GetString("path") ?? ""),
            ["seconds"] = Math.Round(sw.Elapsed.TotalSeconds, 2),
            ["played_seconds"] = Math.Round(_position - pos0, 2),
            ["cpu_percent_of_machine"] = Math.Round(cpu, 2),
            ["logical_cpus"] = Environment.ProcessorCount,
            ["hwdec"] = _hwdec,
            ["composition_size"] = $"{_compositionSize.W}x{_compositionSize.H}",
            ["composition_scale"] = _scaleX,
            ["fullscreen"] = _fullScreen,
            ["colour_mode"] = _colourMode,
            ["display"] = _displaySummary,
            ["swap_chain"] = _swapChain != IntPtr.Zero ? Native.GetDesc1(_swapChain).ToString() : null,
            ["display_fps"] = _mpv.GetDouble("display-fps"),
            ["estimated_vf_fps"] = _mpv.GetDouble("estimated-vf-fps"),
            ["video_params"] = _videoParams,
            ["video_target_params"] = _targetParams,
            ["pid"] = Environment.ProcessId,
        };
        foreach (var c in counters) result[c] = (_mpv.GetInt64(c) ?? 0) - start[c];
        var json = JsonSerializer.Serialize(result, new JsonSerializerOptions { WriteIndented = true });
        var file = Path.Combine(_opts.DataDir, $"measure-{DateTime.Now:yyyyMMdd-HHmmss}.json");
        await File.WriteAllTextAsync(file, json);
        Log("measurement " + json);
        MeasureButton.Content = "Measure 60 s";
        MeasureButton.IsEnabled = true;
        if (exit) Close();
    }

    // Case 4: maximize, full screen, restore and an animated resize (programmatic, not the user's sizing loop), five rounds.
    async Task Transitions()
    {
        if (_mpv == null) return;
        await Task.Delay(2000);
        string[] counters = { "frame-drop-count", "decoder-frame-drop-count", "vo-delayed-frame-count", "mistimed-frame-count" };
        var start = counters.ToDictionary(c => c, c => _mpv.GetInt64(c) ?? 0);
        var sizes = new List<string>();
        var presenter = (OverlappedPresenter)AppWindow.Presenter;
        var baseSize = AppWindow.Size;
        var sw = Stopwatch.StartNew();
        for (int round = 0; round < 5; round++)
        {
            presenter.Maximize(); await Task.Delay(1500); sizes.Add($"max {_compositionSize.W}x{_compositionSize.H}");
            SetFullScreen(true); await Task.Delay(1500); sizes.Add($"full {_compositionSize.W}x{_compositionSize.H}");
            SetFullScreen(false); await Task.Delay(800);
            presenter.Restore(); await Task.Delay(1000); sizes.Add($"restored {_compositionSize.W}x{_compositionSize.H}");
            for (int i = 0; i < 60; i++)
            {
                var t = Math.Sin(i / 60.0 * Math.PI);
                AppWindow.Resize(new Windows.Graphics.SizeInt32(baseSize.Width - (int)(t * 700), baseSize.Height - (int)(t * 400)));
                await Task.Delay(16);
            }
            await Task.Delay(500);
        }
        var result = counters.ToDictionary(c => c, c => (_mpv.GetInt64(c) ?? 0) - start[c]);
        var json = JsonSerializer.Serialize(new { seconds = Math.Round(sw.Elapsed.TotalSeconds, 1), drops = result, sizes, file = _mpv.GetString("filename") },
            new JsonSerializerOptions { WriteIndented = true });
        await File.WriteAllTextAsync(Path.Combine(_opts.DataDir, $"transitions-{DateTime.Now:yyyyMMdd-HHmmss}.json"), json);
        Log("transitions " + json);
        if (_opts.Exit) Close();
    }

    // ------------------------------------------------------------------ media keys and the media flyout (case 6)
    void SetupSmtc()
    {
        _smtc = SystemMediaTransportControlsInterop.GetForWindow(_hwnd);
        _smtc.IsEnabled = true;
        _smtc.IsPlayEnabled = _smtc.IsPauseEnabled = _smtc.IsStopEnabled = true;
        _smtc.IsFastForwardEnabled = _smtc.IsRewindEnabled = true;
        _smtc.IsNextEnabled = _smtc.IsPreviousEnabled = true; // mapped to +30 s / -10 s for the spike
        _smtc.PlaybackStatus = MediaPlaybackStatus.Closed;
        _smtc.ButtonPressed += (s, e) => _ui.TryEnqueue(() =>
        {
            Log($"smtc button {e.Button}");
            switch (e.Button)
            {
                case SystemMediaTransportControlsButton.Play: _mpv?.SetPropertyAsync("pause", "no"); break;
                case SystemMediaTransportControlsButton.Pause: _mpv?.SetPropertyAsync("pause", "yes"); break;
                case SystemMediaTransportControlsButton.Stop: _mpv?.SetPropertyAsync("pause", "yes"); break;
                case SystemMediaTransportControlsButton.Next:
                case SystemMediaTransportControlsButton.FastForward: SeekRelative(30); break;
                case SystemMediaTransportControlsButton.Previous:
                case SystemMediaTransportControlsButton.Rewind: SeekRelative(-10); break;
            }
        });
        _smtc.PlaybackPositionChangeRequested += (s, e) => _ui.TryEnqueue(() =>
        {
            Log($"smtc seek {e.RequestedPlaybackPosition}");
            _mpv?.CommandAsync("seek", e.RequestedPlaybackPosition.TotalSeconds.ToString(System.Globalization.CultureInfo.InvariantCulture), "absolute");
        });
    }

    async void UpdateSmtcDisplay()
    {
        if (_smtc == null || _mpv == null) return;
        var u = _smtc.DisplayUpdater;
        u.ClearAll();
        u.Type = MediaPlaybackType.Video;
        u.VideoProperties.Title = _title;
        u.VideoProperties.Subtitle = "Current video spike";
        u.Update();
        // Artwork: a frame grabbed by mpv a few seconds in.
        await Task.Delay(2500);
        var art = Path.Combine(_opts.DataDir, "smtc-art.jpg");
        if (_mpv.Command("screenshot-to-file", art, "video") >= 0 && File.Exists(art))
        {
            u.Thumbnail = RandomAccessStreamReference.CreateFromFile(await StorageFile.GetFileFromPathAsync(art));
            u.Update();
        }
    }

    void UpdateTimeline(bool force)
    {
        if (_smtc == null || _duration <= 0) return;
        if (!force && DateTime.UtcNow - _lastTimelineUpdate < TimeSpan.FromSeconds(5)) return;
        _lastTimelineUpdate = DateTime.UtcNow;
        _smtc.UpdateTimelineProperties(new SystemMediaTransportControlsTimelineProperties
        {
            StartTime = TimeSpan.Zero, MinSeekTime = TimeSpan.Zero,
            EndTime = TimeSpan.FromSeconds(_duration), MaxSeekTime = TimeSpan.FromSeconds(_duration),
            Position = TimeSpan.FromSeconds(_position),
        });
    }

    // ------------------------------------------------------------------ shutdown
    void MainWindow_Closed(object sender, WindowEventArgs args)
    {
        _infoTimer.Stop();
        _swapChainPoll.Stop();
        if (_smtc != null) _smtc.IsEnabled = false;
        Native.SetSwapChain(VideoPanel, IntPtr.Zero);
        if (_swapChain != IntPtr.Zero) System.Runtime.InteropServices.Marshal.Release(_swapChain);
        _swapChain = IntPtr.Zero;
        _mpv?.Dispose();
        _mpv = null;
        Log("closed");
        lock (this) { _log?.Dispose(); _log = null; }
    }
}
