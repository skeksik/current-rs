using System.ComponentModel;
using System.Diagnostics;
using System.Globalization;
using System.Text;
using Current.Bindings;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using VirtualKey = Windows.System.VirtualKey;

namespace Current.Shell.Player;

/// <summary>What the player needs from the window it is in.</summary>
internal interface IPlayerHost
{
    bool IsFullScreen { get; }

    /// <summary>The AppWindow's full-screen presenter on or off (ADR 0007 item 3).</summary>
    void SetFullScreen(bool on);

    /// <summary>The window's title bar names what plays (null: nothing).</summary>
    void ShowPlaying(string? title);

    /// <summary>The player has closed: the page it was opened from shows again.</summary>
    void PlayerClosed();
}

/// <summary>
/// The player (ADR 0019 task 5, ADR 0007 "What M2 must do" item 3): libmpv's swap chain in a SwapChainPanel, with the composition size
/// and the display's facts going to the core, auto-hiding transport controls over it, the skip button and the next-episode card as
/// the core reports them, playback info, keyboard shortcuts, full screen with the cursor hidden after inactivity, and the system media
/// controls. It renders the core's PlayerState and sends actions; when anything shows is the core's, the words are PlayerText's.
/// </summary>
public sealed partial class PlayerView : UserControl
{
    /// <summary>The control panel hides this long after the last input while playing: MediaTransportControls'
    /// ControlPanelDisplayTimeoutInSecs, 3.0 (MediaTransportControls_partial.cpp, microsoft-ui-xaml, tag winui3/release/2.5.1, read
    /// 2026-10-25).</summary>
    private static readonly TimeSpan HideAfter = TimeSpan.FromSeconds(3);

    /// <summary>Seeks from the keyboard on the seek bar wait this long for the next key, so holding an arrow makes one seek.</summary>
    private static readonly TimeSpan SeekSettle = TimeSpan.FromMilliseconds(250);

    /// <summary>Left and Right without Ctrl: Media Player does nothing on them (owner's PC, 2026-10-25); 10 s both ways here, as its
    /// Skip back.</summary>
    private static readonly TimeSpan ArrowSeek = TimeSpan.FromSeconds(10);

    private const int VolumeStep = 5;

    private IPlayerHost? _host;
    private Window? _window;
    private PlayerSession? _session;
    private DisplayWatcher? _display;
    private MediaControls? _media;
    private CancellationTokenSource? _play;
    /// <summary>The engine's open, which belongs to the player rather than to one play: a play that replaces another while the engine
    /// opens waits for the same open and plays on its session.</summary>
    private Task _opening = Task.CompletedTask;
    /// <summary>The last engine close, which a play waits for before it opens another (one engine per process, ADR 0019).</summary>
    private Task _closing = Task.CompletedTask;
    private bool _open;
    private readonly DispatcherQueueTimer _hideTimer;
    private readonly DispatcherQueueTimer _seekTimer;
    private readonly DispatcherQueueTimer _infoTimer;
    private InputCursor? _noCursor;
    private bool _cursorHidden;
    private bool _controlsShown = true;
    private bool _pointerOverPanel;
    private bool _showingState;
    private bool _dragging;
    private (uint Width, uint Height) _compositionSize;
    private IntPtr _attached;
    private long _shownSecond = -1;

    public PlayerView()
    {
        InitializeComponent();
        Visibility = Visibility.Collapsed;
        _hideTimer = DispatcherQueue.CreateTimer();
        _hideTimer.Interval = HideAfter;
        _hideTimer.IsRepeating = false;
        _hideTimer.Tick += (_, _) =>
        {
            KeepKeyboardFocus();
            HideControlsIfIdle();
        };
        _seekTimer = DispatcherQueue.CreateTimer();
        _seekTimer.Interval = SeekSettle;
        _seekTimer.IsRepeating = false;
        _seekTimer.Tick += (_, _) => SeekToBar();
        _infoTimer = DispatcherQueue.CreateTimer();
        _infoTimer.Interval = TimeSpan.FromSeconds(1);
        _infoTimer.Tick += (_, _) => _ = ShowInfoAsync();
        // In full screen the pointer goes with the controls, once they have faded out, not as they start to (owner's run, 2026-10-25).
        FadeOut.Completed += (_, _) =>
        {
            if (!_controlsShown && _host?.IsFullScreen == true)
            {
                HideCursor();
            }
        };

        Video.SizeChanged += (_, _) => UpdateCompositionSize();
        Video.CompositionScaleChanged += (_, _) =>
        {
            ApplyMatrixTransform();
            UpdateCompositionSize();
        };
        // handledEventsToo: the slider and the buttons handle the pointer themselves.
        Root.AddHandler(PointerMovedEvent, new PointerEventHandler((_, _) => ShowControls()), true);
        Root.AddHandler(PointerPressedEvent, new PointerEventHandler((_, _) => ShowControls()), true);
        // The keys come from the window (MainWindow.OnKeyDown, HandleKey): with no element focused, Windows sends them to the window's
        // root, above the player, and they never reach it (owner's PC, 2026-10-25).
        Video.PointerPressed += (_, _) => Focus(FocusState.Pointer);
        Video.DoubleTapped += (_, _) => ToggleFullScreen();
        ControlPanel.PointerEntered += (_, _) => _pointerOverPanel = true;
        ControlPanel.PointerExited += (_, _) => _pointerOverPanel = false;
        SeekBar.AddHandler(PointerPressedEvent, new PointerEventHandler((_, _) => _dragging = true), true);
        SeekBar.AddHandler(PointerReleasedEvent, new PointerEventHandler((_, _) => EndDrag()), true);
        SeekBar.AddHandler(PointerCaptureLostEvent, new PointerEventHandler((_, _) => EndDrag()), true);
        SeekBar.GotFocus += (_, _) => ShowSeekStatus();
    }

    /// <summary>The window it is shown in; once, before the first play.</summary>
    internal void Attach(Window window, IPlayerHost host)
    {
        _window = window;
        _host = host;
        window.Activated += (_, args) =>
        {
            _windowActive = args.WindowActivationState != WindowActivationState.Deactivated;
            KeepKeyboardFocus();
        };
    }

    /// <summary>Whether the player is shown (playing, opening or showing a problem).</summary>
    public bool IsOpen => _open;

    /// <summary>Whether the player is shown or its engine is still opening or closing.</summary>
    internal bool IsBusy => _open || !_opening.IsCompleted || !_closing.IsCompleted;

    /// <summary>
    /// Shows the player and plays <paramref name="item"/> from <paramref name="start"/>. The first play opens the engine with the
    /// panel's size and the display's facts; a play while another item plays replaces it.
    /// </summary>
    internal async Task PlayAsync(CoreHost core, ItemRef item, TimeSpan start)
    {
        _play?.Cancel();
        var play = new CancellationTokenSource();
        _play = play;
        _open = true;
        Visibility = Visibility.Visible;
        ShowProblem(null);
        BusyRing.IsActive = true;
        ShowControls();
        // Once it is laid out: a view just made visible takes no focus yet.
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, () =>
        {
            if (!Focus(FocusState.Programmatic))
            {
                Trace.TraceWarning("Current: the player did not take focus");
            }
        });
        try
        {
            // An earlier close finishes first: one engine per process (ADR 0019).
            await _closing;
            if (play.IsCancellationRequested)
            {
                return;
            }
            if (_session is null)
            {
                if (_opening.IsCompleted)
                {
                    _opening = OpenAsync(core);
                }
                await _opening;
                if (play.IsCancellationRequested || _session is null)
                {
                    return;
                }
            }
            await _session.PlayAsync(item, start, play.Token);
        }
        catch (Exception) when (play.IsCancellationRequested)
        {
            // A later play or a close took over, and says what it has to.
        }
        catch (PlayerException error) when (error.Problem is PlayerProblem.Replaced or PlayerProblem.Closed)
        {
            // A later play or a close took over.
        }
        catch (PlayerException error)
        {
            BusyRing.IsActive = false;
            ShowProblem(PlayerText.Player(error.Problem));
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: playing failed: {error.GetType().Name}");
            BusyRing.IsActive = false;
            ShowProblem(ProblemText.Unexpected);
        }
    }

    private async Task OpenAsync(CoreHost core)
    {
        if (_window is null)
        {
            throw new InvalidOperationException("the player is not attached to a window");
        }
        var size = await PanelSizeAsync();
        if (!_open)
        {
            // Closed while laid out, with no play since.
            return;
        }
        var display = new DisplayWatcher(_window);
        PlayerSession session;
        try
        {
            session = await PlayerSession.OpenAsync(core, new CompositionSize(size.Width, size.Height), display.Facts,
                new DispatcherQueueUi(DispatcherQueue));
        }
        catch
        {
            display.Dispose();
            throw;
        }
        if (!_open)
        {
            // Closed while opening, with no play since: the engine closes, and the next play waits for that.
            display.Dispose();
            _closing = CloseSessionAsync(session);
            return;
        }
        _display = display;
        display.Changed += OnDisplayChanged;
        _compositionSize = size;
        _session = session;
        _qualityOptions = null;
        session.State.PropertyChanged += OnState;
        session.NowPlaying.PropertyChanged += OnNowPlaying;
        session.SwapChain.PropertyChanged += OnSwapChain;
        _media = new MediaControls(WinRT.Interop.WindowNative.GetWindowHandle(_window), DispatcherQueue, session, () => _ = CloseAsync());
        OnSwapChain(null, null!);
        OnState(null, null!);
        OnNowPlaying(null, null!);
        // A size change while the engine was opening found no session to tell.
        UpdateCompositionSize();
    }

    /// <summary>The panel's size in physical pixels once it is laid out (ActualSize × CompositionScale, ADR 0007).</summary>
    private async Task<(uint Width, uint Height)> PanelSizeAsync()
    {
        // A view just shown keeps its last layout's size until it is laid out again: after the window moved to a monitor with another
        // scale, that was the old size (1905×969 sent for a 1584×843 panel, owner's PC, 2026-10-25).
        UpdateLayout();
        for (var tries = 0; tries < 60 && (Video.ActualWidth <= 0 || Video.ActualHeight <= 0); tries++)
        {
            await Task.Delay(16);
        }
        return PanelSize();
    }

    private (uint Width, uint Height) PanelSize() => (
        (uint)Math.Max(1, Math.Round(Video.ActualWidth * Video.CompositionScaleX)),
        (uint)Math.Max(1, Math.Round(Video.ActualHeight * Video.CompositionScaleY)));

    private void UpdateCompositionSize()
    {
        if (_session is null)
        {
            return;
        }
        var size = PanelSize();
        if (size == _compositionSize)
        {
            return;
        }
        _compositionSize = size;
        Quick(() => _session.SetCompositionSize(new CompositionSize(size.Width, size.Height)));
    }

    private void OnDisplayChanged(object? sender, EventArgs e)
    {
        if (_session is { } session && _display is { } display)
        {
            Quick(() => session.SetDisplayFacts(display.Facts));
        }
    }

    /// <summary>
    /// A new swap chain from the engine goes to the panel with the transform for the panel's scale (ADR 0007). The reference stays the
    /// session's until a newer one replaces it or the player closes (PlayerSession releases it then); the panel holds its own.
    /// </summary>
    private void OnSwapChain(object? sender, PropertyChangedEventArgs e)
    {
        if (_session?.SwapChain.Value is not { IsReleased: false } swapChain || swapChain.Pointer == _attached)
        {
            return;
        }
        try
        {
            PlayerNative.SetSwapChain(Video, swapChain.Pointer);
            _attached = swapChain.Pointer;
            ApplyMatrixTransform();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: attaching the swap chain failed: {error.GetType().Name}");
        }
    }

    private void ApplyMatrixTransform()
    {
        if (_attached == IntPtr.Zero)
        {
            return;
        }
        try
        {
            PlayerNative.SetMatrixTransform(_attached, Video.CompositionScaleX, Video.CompositionScaleY);
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the swap chain's transform failed: {error.GetType().Name}");
        }
    }

    private void OnNowPlaying(object? sender, PropertyChangedEventArgs e)
    {
        if (_session?.NowPlaying.Value is not { } now)
        {
            return;
        }
        // Another item (the next episode) has its own quality menu: the last one's names are not shown first.
        if (!Equals(now.Item, _qualityItem))
        {
            _qualityItem = now.Item;
            _qualityOptions = null;
        }
        _media?.Show(now);
        _host?.ShowPlaying(now.Title);
    }

    private PlayerState State => _session?.State.Value ?? PlayerState.Idle;

    private void OnState(object? sender, PropertyChangedEventArgs e)
    {
        var state = State;
        KeepKeyboardFocus();
        _showingState = true;
        try
        {
            BusyRing.IsActive = state.State is PlayerStateKind.Preparing or PlayerStateKind.Loading or PlayerStateKind.Buffering
                or PlayerStateKind.Seeking;
            var playing = state.State is PlayerStateKind.Playing or PlayerStateKind.Buffering or PlayerStateKind.Seeking;
            PlayPauseGlyph.Glyph = playing ? "" : "";
            SetName(PlayPauseButton, playing ? "Pause" : "Play");
            ShowPosition(state);
            // Not under the user's hand: their change is on its way to the core.
            if (VolumeSlider.FocusState == FocusState.Unfocused)
            {
                VolumeSlider.Value = state.Volume;
            }
            VolumeValue.Text = state.Volume.ToString(CultureInfo.CurrentCulture);
            MuteButton.IsChecked = state.Muted;
            VolumeSymbol.Symbol = MuteSymbol.Symbol = state.Muted ? Symbol.Mute : Symbol.Volume;
            SetName(MuteButton, state.Muted ? "Unmute" : "Mute");

            SkipButton.Visibility = state.Segment is null ? Visibility.Collapsed : Visibility.Visible;
            if (state.Segment is { } segment)
            {
                SkipButton.Content = PlayerText.Skip(segment.Kind);
            }
            NextCard.Visibility = state.Next is null ? Visibility.Collapsed : Visibility.Visible;
            if (state.Next is { } next)
            {
                NextName.Text = PlayerText.Episode(next);
                NextCountdown.Text = next.Countdown is { } seconds ? PlayerText.Countdown(seconds) : "";
                NextCountdown.Visibility = next.Countdown is null ? Visibility.Collapsed : Visibility.Visible;
            }

            if (state is { State: PlayerStateKind.Failed, Problem: { } problem })
            {
                ShowProblem(PlayerText.Problem(problem));
            }
            else if (state.State is not PlayerStateKind.Idle)
            {
                ShowProblem(null);
            }
            if (!playing)
            {
                ShowControls();
            }
        }
        finally
        {
            _showingState = false;
        }
        // The end with no next episode to go to: back to the page it was played from, as the core has reported the stop.
        if (state is { State: PlayerStateKind.Ended, Next: null })
        {
            _ = CloseAsync();
        }
    }

    /// <summary>
    /// Gives the keyboard back to the player when the window is in front but its content lost it. As playback starts, Windows' focus
    /// moves from the window's XAML content to the top-level window itself, with or without the media controls (GetFocus and
    /// GetGUIThreadInfo on the owner's PC, 2026-10-25; what moves it, in libmpv's start or elsewhere, was not found), and then no key
    /// reaches the player. InputFocusController.TrySetFocus puts it back on the content's island (Microsoft.UI.Input, Windows App SDK
    /// InteractiveExperiences 2.1.9 metadata; its reference page was not read), and the player takes XAML focus if nothing has it.
    /// </summary>
    private void KeepKeyboardFocus()
    {
        if (!_open || !_windowActive || XamlRoot?.ContentIsland is not { } island)
        {
            return;
        }
        try
        {
            var controller = InputFocusController.GetForIsland(island);
            if (!controller.HasFocus)
            {
                controller.TrySetFocus();
            }
            if (FocusManager.GetFocusedElement(XamlRoot) is null)
            {
                Focus(FocusState.Programmatic);
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: taking the keyboard back failed: {error.GetType().Name}");
        }
    }

    /// <summary>Whether the window is in front: the player never takes focus from another app.</summary>
    private bool _windowActive = true;

    /// <summary>The seek bar, its buffered range and the times, unless the user is moving the bar.</summary>
    private void ShowPosition(PlayerState state)
    {
        var duration = state.Duration ?? TimeSpan.Zero;
        SeekBar.IsEnabled = state.Duration is not null;
        SeekBar.Maximum = Math.Max(1, duration.TotalSeconds);
        SeekBar.StepFrequency = 1;
        SeekBar.SmallChange = ArrowSeek.TotalSeconds;
        SeekBar.LargeChange = 60;
        var position = state.Position;
        if (!_dragging && !_seekTimer.IsRunning)
        {
            SeekBar.Value = Math.Min(position.TotalSeconds, SeekBar.Maximum);
        }
        else
        {
            position = TimeSpan.FromSeconds(SeekBar.Value);
        }
        ShowBuffered(state.Buffered, duration);
        var second = (long)position.TotalSeconds;
        if (second == _shownSecond)
        {
            return;
        }
        _shownSecond = second;
        ElapsedText.Text = PlayerText.Time(position);
        var remaining = duration - position;
        RemainingText.Text = PlayerText.Time(remaining);
        SetName(ElapsedText, PlayerText.Elapsed(position));
        SetName(RemainingText, PlayerText.Remaining(remaining));
        if (state.State is not PlayerStateKind.Playing)
        {
            ShowSeekStatus();
        }
    }

    /// <summary>Where the seek bar is, read by Narrator when it gets focus; not raised every second while playing.</summary>
    private void ShowSeekStatus() =>
        AutomationProperties.SetItemStatus(SeekBar, PlayerText.Position(TimeSpan.FromSeconds(SeekBar.Value), State.Duration));

    private Microsoft.UI.Xaml.Shapes.Rectangle? _buffered;

    /// <summary>
    /// The buffered range as a bar in the seek bar's own track: a rectangle beside its template part HorizontalTrackRect, in the
    /// track's colour (the Slider keeps its default template; parts from Slider_themeresources.xaml, microsoft-ui-xaml, not re-read
    /// for this: not verified beyond the running app).
    /// </summary>
    private void ShowBuffered(TimeSpan? buffered, TimeSpan duration)
    {
        if (_buffered is null)
        {
            if (FindDescendant(SeekBar, "HorizontalTrackRect") is not Microsoft.UI.Xaml.Shapes.Rectangle track
                || Microsoft.UI.Xaml.Media.VisualTreeHelper.GetParent(track) is not Grid grid)
            {
                return;
            }
            _buffered = new Microsoft.UI.Xaml.Shapes.Rectangle
            {
                Height = track.Height,
                RadiusX = track.RadiusX,
                RadiusY = track.RadiusY,
                HorizontalAlignment = HorizontalAlignment.Left,
                VerticalAlignment = track.VerticalAlignment,
                Margin = track.Margin,
                IsHitTestVisible = false,
                Fill = track.Fill,
            };
            Grid.SetRow(_buffered, Grid.GetRow(track));
            Grid.SetColumn(_buffered, Grid.GetColumn(track));
            Grid.SetColumnSpan(_buffered, Grid.GetColumnSpan(track));
            grid.Children.Insert(grid.Children.IndexOf(track) + 1, _buffered);
            track.SizeChanged += (_, _) => ShowBuffered(State.Buffered, State.Duration ?? TimeSpan.Zero);
            _trackRect = track;
        }
        var fraction = duration > TimeSpan.Zero && buffered is { } end ? Math.Clamp(end / duration, 0, 1) : 0;
        _buffered.Width = (_trackRect?.ActualWidth ?? 0) * fraction;
    }

    private Microsoft.UI.Xaml.Shapes.Rectangle? _trackRect;

    private static FrameworkElement? FindDescendant(DependencyObject parent, string name)
    {
        for (var i = 0; i < Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChildrenCount(parent); i++)
        {
            var child = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChild(parent, i);
            if (child is FrameworkElement element && element.Name == name)
            {
                return element;
            }
            if (FindDescendant(child, name) is { } found)
            {
                return found;
            }
        }
        return null;
    }

    /// <summary>A control's name for Narrator; a command's label too, which its tool tip shows while labels are collapsed.</summary>
    private static void SetName(DependencyObject element, string name)
    {
        AutomationProperties.SetName(element, name);
        switch (element)
        {
            case AppBarButton button:
                button.Label = name;
                break;
            case AppBarToggleButton toggle:
                toggle.Label = name;
                break;
        }
    }

    private void ShowProblem(string? message)
    {
        if (message is null)
        {
            ProblemInfo.IsOpen = false;
            return;
        }
        if (!ProblemInfo.IsOpen || ProblemInfo.Message != message)
        {
            ProblemInfo.IsOpen = false;
            ProblemInfo.Message = message;
            ProblemInfo.IsOpen = true;
        }
        ShowControls();
    }

    // Controls ---------------------------------------------------------------------------------------------------------------------

    private void ShowControls()
    {
        if (!_open)
        {
            return;
        }
        // Also here, not only on state changes: while the engine waits (a stalled network) no state changes come, and the keys were
        // lost (owner's run, 2026-10-25).
        KeepKeyboardFocus();
        ShowCursor();
        if (!_controlsShown)
        {
            _controlsShown = true;
            ControlPanel.IsHitTestVisible = true;
            FadeOut.Stop();
            FadeIn.Begin();
        }
        _hideTimer.Stop();
        _hideTimer.Start();
    }

    /// <summary>
    /// Hides the controls while playing, unless the pointer is over them, one of their menus is open or keyboard focus is in them;
    /// in full screen the cursor goes too. Pointer focus left on a button goes to the player, so keys reach its shortcuts.
    /// </summary>
    private void HideControlsIfIdle()
    {
        var playing = State.State is PlayerStateKind.Playing;
        var focused = FocusManager.GetFocusedElement(XamlRoot) as UIElement;
        var keyboardInPanel = focused is not null && IsInside(focused, ControlPanel) && focused.FocusState == FocusState.Keyboard;
        var menuOpen = AudioMenu.IsOpen || SubtitlesMenu.IsOpen || QualityMenu.IsOpen || VolumeFlyout.IsOpen;
        if (!playing || _pointerOverPanel || keyboardInPanel || menuOpen || ProblemInfo.IsOpen)
        {
            _hideTimer.Start();
            return;
        }
        if (focused is not null && IsInside(focused, ControlPanel))
        {
            Focus(FocusState.Programmatic);
        }
        _controlsShown = false;
        ControlPanel.IsHitTestVisible = false;
        FadeIn.Stop();
        FadeOut.Begin();
    }

    private static bool IsInside(DependencyObject element, DependencyObject parent)
    {
        for (var at = element; at is not null; at = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetParent(at))
        {
            if (at == parent)
            {
                return true;
            }
        }
        return false;
    }

    /// <summary>
    /// The resource id of the cursor without a visual that build.ps1 adds to the published Current.exe (Add-NoCursor), which WinUI loads
    /// from the executable's own resources (InputDesktopResourceCursor: "You can add a .res resource file that contains a custom cursor
    /// to your project", learn.microsoft.com, updated 2026-07-28, read 2026-10-25). Win32's ShowCursor(false) and SetCursor(NULL), even
    /// every 200 ms, left the pointer showing over the WinUI content, and ProtectedCursor = null shows the default arrow (owner's PC,
    /// 2026-10-25); InputCustomCursor is "Not implemented" (same site and date).
    /// </summary>
    private const uint NoCursorResource = 101;

    /// <summary>Hides the pointer over the player: its cursor becomes the one without a visual, until the pointer moves
    /// (ShowControls). Nothing in a build without that resource.</summary>
    private void HideCursor()
    {
        if (_noCursor is null)
        {
            try
            {
                _noCursor = InputDesktopResourceCursor.CreateFromModule("Current.exe", NoCursorResource);
            }
            catch (Exception error)
            {
                Trace.TraceWarning($"Current: no cursor resource to hide the pointer with: {error.GetType().Name}");
                return;
            }
        }
        ProtectedCursor = _noCursor;
        _cursorHidden = true;
    }

    /// <summary>The default cursor again.</summary>
    private void ShowCursor()
    {
        if (_cursorHidden)
        {
            _cursorHidden = false;
            ProtectedCursor = null;
        }
    }

    // Keyboard ---------------------------------------------------------------------------------------------------------------------

    /// <summary>
    /// The player's keys, where no control took the key first (a focused button takes Space and Enter, the seek bar the arrows):
    /// Space and K play or pause (Space as in Media Player, owner's PC, 2026-10-25), Left and Right seek 10 s, Up and Down change the
    /// volume, F and F11 toggle full screen (F11 as in Media Player), Esc leaves full screen (as in Media Player), M mutes. Ctrl+P,
    /// Ctrl+Left, Ctrl+Right, Ctrl+I and F11 are the buttons' own accelerators (PlayerView.xaml), Media Player's keys.
    /// </summary>
    internal void HandleKey(KeyRoutedEventArgs e)
    {
        ShowControls();
        if (e.Handled || _session is not { } session || IsModifierDown())
        {
            return;
        }
        switch (e.Key)
        {
            case VirtualKey.Space:
            case VirtualKey.K:
                Quick(session.Toggle);
                break;
            case VirtualKey.Left:
                Quick(() => session.SeekBy(-ArrowSeek));
                break;
            case VirtualKey.Right:
                Quick(() => session.SeekBy(ArrowSeek));
                break;
            case VirtualKey.Up:
                Quick(() => session.SetVolume((byte)Math.Min(100, State.Volume + VolumeStep)));
                break;
            case VirtualKey.Down:
                Quick(() => session.SetVolume((byte)Math.Max(0, State.Volume - VolumeStep)));
                break;
            case VirtualKey.F:
                ToggleFullScreen();
                break;
            case VirtualKey.Escape when _host?.IsFullScreen == true:
                _host.SetFullScreen(false);
                break;
            case VirtualKey.M:
                Quick(() => session.SetMuted(!State.Muted));
                break;
            default:
                return;
        }
        e.Handled = true;
    }

    private static bool IsModifierDown()
    {
        static bool Down(VirtualKey key) =>
            InputKeyboardSource.GetKeyStateForCurrentThread(key).HasFlag(Windows.UI.Core.CoreVirtualKeyStates.Down);
        return Down(VirtualKey.Control) || Down(VirtualKey.Menu) || Down(VirtualKey.Shift) || Down(VirtualKey.LeftWindows)
            || Down(VirtualKey.RightWindows);
    }

    // Full screen ------------------------------------------------------------------------------------------------------------------

    /// <summary>Only while the player is shown: its accelerators (F11) stay registered while it is collapsed.</summary>
    private void ToggleFullScreen()
    {
        if (_open)
        {
            _host?.SetFullScreen(_host.IsFullScreen != true);
        }
    }

    /// <summary>The window's presenter changed (the button, a key, or Windows): the button follows, and the cursor shows again.</summary>
    internal void FullScreenChanged(bool on)
    {
        FullScreenSymbol.Symbol = on ? Symbol.BackToWindow : Symbol.FullScreen;
        SetName(FullScreenButton, on ? "Exit full screen" : "Full screen");
        ShowControls();
    }

    // Actions ----------------------------------------------------------------------------------------------------------------------

    private void Quick(Action action)
    {
        try
        {
            action();
        }
        catch (PlayerException)
        {
            // Closed meanwhile.
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: a player action failed: {error.GetType().Name}");
        }
    }

    private async void Long(Func<CancellationToken, Task> action)
    {
        var play = _play;
        try
        {
            await action(play?.Token ?? CancellationToken.None);
        }
        catch (PlayerException error) when (error.Problem is PlayerProblem.Replaced or PlayerProblem.Closed)
        {
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: a player action failed: {error.GetType().Name}");
        }
    }

    private void OnPlayPause(object sender, RoutedEventArgs e) => Quick(() => _session?.Toggle());

    private void OnSkipBack(object sender, RoutedEventArgs e) => Quick(() => _session?.SeekBy(TimeSpan.FromSeconds(-10)));

    private void OnSkipForward(object sender, RoutedEventArgs e) => Quick(() => _session?.SeekBy(TimeSpan.FromSeconds(30)));

    private void OnFullScreen(object sender, RoutedEventArgs e) => ToggleFullScreen();

    private void OnSkip(object sender, RoutedEventArgs e) => Quick(() => _session?.SkipSegment());

    private void OnPlayNext(object sender, RoutedEventArgs e) => Long(ct => _session?.PlayNextAsync(ct) ?? Task.CompletedTask);

    private void OnDismissNext(object sender, RoutedEventArgs e) => Quick(() => _session?.DismissNext());

    private void OnProblemBack(object sender, RoutedEventArgs e) => _ = CloseAsync();

    private void OnMute(object sender, RoutedEventArgs e) => Quick(() => _session?.SetMuted(MuteButton.IsChecked == true));

    private void OnVolumeChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        VolumeValue.Text = ((int)e.NewValue).ToString(CultureInfo.CurrentCulture);
        if (!_showingState)
        {
            Quick(() => _session?.SetVolume((byte)Math.Clamp(e.NewValue, 0, 100)));
        }
    }

    private void OnSeekBarChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (_showingState || _session is null)
        {
            return;
        }
        var position = TimeSpan.FromSeconds(e.NewValue);
        ElapsedText.Text = PlayerText.Time(position);
        // A drag seeks when it ends; keys seek once they settle.
        if (!_dragging)
        {
            _seekTimer.Stop();
            _seekTimer.Start();
        }
    }

    private void EndDrag()
    {
        if (!_dragging)
        {
            return;
        }
        _dragging = false;
        SeekToBar();
    }

    private void SeekToBar()
    {
        _seekTimer.Stop();
        var position = TimeSpan.FromSeconds(SeekBar.Value);
        Quick(() => _session?.SeekTo(position));
    }

    private void OnAudioMenuOpening(object sender, object e)
    {
        AudioMenu.Items.Clear();
        var state = State;
        foreach (var track in state.AudioTracks)
        {
            var item = new RadioMenuFlyoutItem { Text = PlayerText.Track(track), GroupName = "audio", IsChecked = track.Index == state.Audio };
            var index = track.Index;
            item.Click += (_, _) => Long(ct => _session?.SelectAudioAsync(index, ct) ?? Task.CompletedTask);
            AudioMenu.Items.Add(item);
        }
        if (AudioMenu.Items.Count == 0)
        {
            AudioMenu.Items.Add(new MenuFlyoutItem { Text = "No audio tracks", IsEnabled = false });
        }
    }

    private void OnSubtitlesMenuOpening(object sender, object e)
    {
        SubtitlesMenu.Items.Clear();
        var state = State;
        var off = new RadioMenuFlyoutItem { Text = "Off", GroupName = "subtitles", IsChecked = state.Subtitle is SubtitleSelection.Off };
        off.Click += (_, _) => Long(ct => _session?.SelectSubtitleAsync(new SubtitleSelection.Off(), ct) ?? Task.CompletedTask);
        SubtitlesMenu.Items.Add(off);
        foreach (var track in state.SubtitleTracks)
        {
            var item = new RadioMenuFlyoutItem
            {
                Text = PlayerText.Track(track),
                GroupName = "subtitles",
                IsChecked = state.Subtitle is SubtitleSelection.Stream stream && stream.Index == track.Index,
            };
            var index = track.Index;
            item.Click += (_, _) =>
                Long(ct => _session?.SelectSubtitleAsync(new SubtitleSelection.Stream(index), ct) ?? Task.CompletedTask);
            SubtitlesMenu.Items.Add(item);
        }
    }

    /// <summary>The quality menu's entries as the core last named them for the version playing; null before the first.</summary>
    private IReadOnlyList<QualityOption>? _qualityOptions;

    /// <summary>The item <see cref="_qualityOptions"/> were named for.</summary>
    private object? _qualityItem;

    /// <summary>
    /// Fills the menu at once with the entries last named (or the core's bare steps the first time), then asks the core for the
    /// version playing and fills it again if they changed. async void, as event handlers are: it catches everything itself.
    /// </summary>
    private async void OnQualityMenuOpening(object sender, object e)
    {
        if (App.Core is not { } core)
        {
            QualityMenu.Items.Clear();
            return;
        }
        var shown = _qualityOptions;
        FillQualityMenu(core, shown);
        if (_session is not { } session)
        {
            return;
        }
        try
        {
            var options = await session.QualityOptionsAsync(core.PlaybackSettings().ShowsAllQualities);
            if (shown is null || !options.SequenceEqual(shown))
            {
                _qualityOptions = options;
                FillQualityMenu(core, options);
            }
        }
        catch (Exception error)
        {
            Trace.TraceWarning($"Current: the quality menu kept its last names: {error.GetType().Name}");
        }
    }

    /// <summary>
    /// The core's menu for <paramref name="options"/> (null: the bare steps) at the quality in use: a limit below every entry is listed
    /// as its own entry, checked, so the menu never claims Original while the stream is capped (the manager's review, S1, 2026-10-27).
    /// </summary>
    private void FillQualityMenu(CoreHost core, IReadOnlyList<QualityOption>? options)
    {
        QualityMenu.Items.Clear();
        var withResolution = core.PlaybackSettings().ShowsQualityResolution;
        foreach (var entry in core.QualityMenu(options, State.Quality, withResolution))
        {
            var item = new RadioMenuFlyoutItem
            {
                Text = entry.Label,
                GroupName = "quality",
                IsChecked = entry.Checked,
            };
            var chosen = entry.Option.Quality;
            item.Click += (_, _) => Long(ct => _session?.SetQualityAsync(chosen, ct) ?? Task.CompletedTask);
            QualityMenu.Items.Add(item);
        }
    }

    // Playback info ----------------------------------------------------------------------------------------------------------------

    private void OnInfo(object sender, RoutedEventArgs e)
    {
        var on = InfoButton.IsChecked == true;
        InfoPanel.Visibility = on ? Visibility.Visible : Visibility.Collapsed;
        if (on)
        {
            _infoTimer.Start();
            _ = ShowInfoAsync();
        }
        else
        {
            _infoTimer.Stop();
        }
    }

    private async Task ShowInfoAsync()
    {
        if (_session is not { } session || InfoPanel.Visibility != Visibility.Visible)
        {
            return;
        }
        try
        {
            var info = await session.PlaybackInfoAsync();
            if (App.Core is { } core)
            {
                InfoText.Text = InfoLines(core, info);
            }
        }
        catch (PlayerException)
        {
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: playback info failed: {error.GetType().Name}");
        }
    }

    /// <summary>The playback info as lines: route, reasons, notes, the source and its streams, the server's transcode, the engine's
    /// output, and the display the window is on. No URL: the record holds none.</summary>
    private string InfoLines(CoreHost core, PlaybackInfo info)
    {
        var text = new StringBuilder();
        void Line(string line) => text.AppendLine(line);
        static string Join(params string?[] parts) => string.Join(" · ", parts.Where(part => !string.IsNullOrEmpty(part)));
        if (info.Route is { } route)
        {
            Line(Join(PlayerText.Route(route.Kind), route.Protocol?.ToString().ToUpperInvariant(), route.Container));
        }
        foreach (var reason in info.Reasons)
        {
            Line($"• {PlayerText.Reason(reason)}");
        }
        foreach (var note in info.Notes)
        {
            Line($"• {PlayerText.Note(core, note)}");
        }
        if (info.Source is { } source)
        {
            Line("");
            Line(Join("Source", source.Container, source.Bitrate is { } bitrate ? core.BitrateLabel(bitrate) : null,
                source.Size is { } size ? $"{size / 1_000_000_000.0:0.0} GB" : null));
            foreach (var stream in source.Streams)
            {
                Line(Join($"#{stream.Index} {stream.Kind}", stream.Codec, stream.Profile,
                    stream.Width is { } w && stream.Height is { } h ? $"{w}×{h}" : null,
                    stream.FrameRate is { } fps ? $"{fps:0.###} fps" : null,
                    stream.BitDepth is { } depth ? $"{depth}-bit" : null, stream.RangeType,
                    stream.Channels is { } channels ? $"{channels} ch" : null, stream.Language, stream.Label,
                    stream.IsExternal ? "external" : null));
            }
        }
        if (info.Transcoding is { } t)
        {
            Line("");
            Line(Join("Server", t.VideoDirect ? "video copied" : $"video to {t.VideoCodec}", t.AudioDirect ? "audio copied" : $"audio to {t.AudioCodec}",
                t.Container, t.Bitrate is { } bitrate ? core.BitrateLabel(bitrate) : null,
                t.Width is { } w && t.Height is { } h ? $"{w}×{h}" : null, t.FrameRate is { } fps ? $"{fps:0.###} fps" : null,
                t.HardwareAcceleration, t.Completion is { } done ? $"{done:0}% done" : null));
            foreach (var reason in t.Reasons)
            {
                Line($"• {PlayerText.Reason(reason)}");
            }
        }
        var engine = info.Engine;
        Line("");
        Line(Join("Player", engine.HardwareDecoder is { Length: > 0 } hw ? $"decoder {hw}" : "software decoder",
            engine.VideoWidth is { } vw && engine.VideoHeight is { } vh ? $"{vw}×{vh}" : null, engine.PixelFormat, engine.Primaries,
            engine.Transfer));
        if (_display?.Facts is { } display)
        {
            Line(Join("Display", display.Hdr ? "HDR" : "SDR", display.PeakNits > 0 ? $"peak {display.PeakNits:0} nits" : null,
                display.SdrWhiteNits > 0 ? $"SDR white {display.SdrWhiteNits:0} nits" : null,
                display.RefreshHz > 0 ? $"{display.RefreshHz:0.###} Hz" : null));
        }
        var scale = XamlRoot?.RasterizationScale ?? 1;
        Line(Join("Panel", $"{_compositionSize.Width}×{_compositionSize.Height}",
            $"view {ActualWidth * scale:0.##}×{ActualHeight * scale:0.##}",
            XamlRoot is { } root ? $"content {root.Size.Width * scale:0.##}×{root.Size.Height * scale:0.##}" : null,
            _window is { } window ? $"window {window.AppWindow.ClientSize.Width}×{window.AppWindow.ClientSize.Height}" : null));
        return text.ToString().TrimEnd();
    }

    // Leaving ----------------------------------------------------------------------------------------------------------------------

    /// <summary>
    /// Closes the player in ADR 0007's order: the panel lets go of the swap chain, the core's reference is given back, then the player
    /// closes (reporting the stop and destroying the engine, PlayerSession.CloseAsync). The view hides at once; the close finishes in
    /// the background, and a play started meanwhile waits for it.
    /// </summary>
    internal Task CloseAsync()
    {
        if (!_open)
        {
            return _closing;
        }
        _open = false;
        _play?.Cancel();
        _play = null;
        _hideTimer.Stop();
        _seekTimer.Stop();
        _infoTimer.Stop();
        // Each step on its own: whatever fails, the engine still closes below.
        Try("showing the cursor", ShowCursor);
        Try("leaving full screen", () =>
        {
            if (_host?.IsFullScreen == true)
            {
                _host.SetFullScreen(false);
            }
        });
        Try("turning the media controls off", () => _media?.Dispose());
        _media = null;
        Try("detaching the swap chain", () => PlayerNative.SetSwapChain(Video, IntPtr.Zero));
        _attached = IntPtr.Zero;
        var session = _session;
        _session = null;
        _qualityOptions = null;
        if (session is not null)
        {
            session.SwapChain.Value?.Release();
            session.State.PropertyChanged -= OnState;
            session.NowPlaying.PropertyChanged -= OnNowPlaying;
            session.SwapChain.PropertyChanged -= OnSwapChain;
        }
        if (_display is not null)
        {
            _display.Changed -= OnDisplayChanged;
            _display.Dispose();
            _display = null;
        }
        Visibility = Visibility.Collapsed;
        ProblemInfo.IsOpen = false;
        InfoButton.IsChecked = false;
        InfoPanel.Visibility = Visibility.Collapsed;
        SkipButton.Visibility = Visibility.Collapsed;
        NextCard.Visibility = Visibility.Collapsed;
        BusyRing.IsActive = false;
        _shownSecond = -1;
        Try("showing the page again", () =>
        {
            _host?.ShowPlaying(null);
            _host?.PlayerClosed();
        });
        // Without a session an earlier close may still run (or an open, which closes its own engine): _closing stays that.
        if (session is not null)
        {
            _closing = CloseSessionAsync(session);
        }
        return _closing;
    }

    /// <summary>Closes the player and waits until no engine is left: the close, and an open still running, which closes the engine it
    /// opens once it finds the player closed (OpenAsync). For the window's close, before the core goes.</summary>
    internal async Task CloseAndWaitAsync()
    {
        _ = CloseAsync();
        try
        {
            await _opening;
        }
        catch (Exception)
        {
            // The open failed: no engine to close.
        }
        // Read after the open: an open nobody wanted set it.
        await _closing;
    }

    private static void Try(string what, Action action)
    {
        try
        {
            action();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: {what} failed while closing the player: {error.GetType().Name}");
        }
    }

    private static async Task CloseSessionAsync(PlayerSession session)
    {
        try
        {
            await session.CloseAsync();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: closing the player failed: {error.GetType().Name}");
        }
    }
}
