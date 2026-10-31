using System.Diagnostics;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.Foundation;
using Windows.Graphics.Imaging;

namespace Current.Shell.Player;

/// <summary>A run of a subtitle line, with the styling a SubRip cue carries (W1 §6: <c>&lt;i&gt;</c> and <c>&lt;b&gt;</c> as
/// subformats).</summary>
internal sealed record SubtitleSpan(string Text, bool Italic, bool Bold);

internal sealed record SubtitleLine(IReadOnlyList<SubtitleSpan> Spans);

/// <summary>Where an image cue goes: its position and extent, each in percent of the subtitle canvas or in the canvas's
/// pixels.</summary>
internal readonly record struct SubtitlePlacement(double X, double Y, bool XyInPercent, double Width, double Height, bool SizeInPercent);

/// <summary>What the native engine shows of its subtitles; every call on the UI thread.</summary>
internal interface ISubtitleSink
{
    /// <summary>A text cue shown from <paramref name="start"/> until <paramref name="end"/>; lines of cues shown together are kept
    /// in the order their cues start. A cue with the same times and text as one shown is the same cue read again (after a track
    /// change or a seek) and is not shown twice.</summary>
    void ShowText(string key, TimeSpan start, TimeSpan end, IReadOnlyList<SubtitleLine> lines);

    /// <summary>An image cue. It replaces every image shown, as a PGS cue ends where the next begins (W1 §6: its Duration is the
    /// maximum). The overlay owns <paramref name="bitmap"/> and disposes it.</summary>
    void ShowImage(string key, TimeSpan start, TimeSpan end, SoftwareBitmap bitmap, SubtitlePlacement placement);

    void Hide(string key);

    /// <summary>Hides every cue whose time does not hold <paramref name="position"/>, whether or not its exit came (W7 §10: a cue
    /// stayed 25 s past its end).</summary>
    void Expire(TimeSpan position);

    void Clear();

    /// <summary>The video's own size in pixels (0 until known), for placing cues on the picture.</summary>
    void VideoSize(uint width, uint height);

    /// <summary>A new file opened: read the caption settings again.</summary>
    void RefreshAppearance();
}

/// <summary>
/// Our subtitle view over Windows' own player (ADR 0025, "Subtitles on the native route"; W5): the cues the native engine hands over,
/// drawn on the picture's rectangle inside the MediaPlayerElement (Stretch Uniform: the video's own size fitted and centred), above
/// our control bar.
/// <list type="bullet">
/// <item>Text cues: bottom-centred lines with their italic and bold runs, in Windows' caption settings (<see cref="SubtitleAppearance"/>;
/// the app has no subtitle settings of its own). A line is 5% of the picture's height at 100% caption size, 4% of it above the
/// picture's bottom, wrapping at 90% of its width: ours, not measured.</item>
/// <item>Image cues (PGS): W1 §6 saw each as a full-canvas bitmap (position 0,0, extent 100% × 100%; 1920×1080 premultiplied BGRA
/// for 1080p and 4K video alike). The canvas, the bitmap's own size, is fitted to the picture's width with its own aspect and its
/// bottom on the picture's bottom, so a 16:9 canvas over a wider (cropped) picture is not squashed; a cue's position and extent are
/// in percent of that canvas or in its pixels. Not verified beyond W1's files (W7).</item>
/// <item>Above the bar: while the control panel shows, the whole layer moves up just enough for the cues to clear it, and back down
/// when it hides, with the panel's own fade times (0.3 s in, 0.7 s out; PlayerView), each move starting where the last one is. The
/// panel's top is measured against the MediaPlayerElement, which nothing moves. libmpv's subtitles do not do this yet
/// (docs/roadmap.md).</item>
/// </list>
/// HDR: the layer is ordinary XAML, composed at the display's SDR white level when Windows HDR is on (not verified; W7).
/// </summary>
internal sealed class SubtitleOverlay : ISubtitleSink
{
    /// <summary>A line's height as a share of the picture's height at 100% caption size.</summary>
    private const double LineShare = 0.05;
    /// <summary>The text's distance from the picture's bottom, as a share of its height.</summary>
    private const double BottomShare = 0.04;
    private const double WidthShare = 0.9;
    /// <summary>The gap kept between the cues and the control panel, in DIPs.</summary>
    private const double PanelGap = 8;
    private static readonly TimeSpan MoveUp = TimeSpan.FromSeconds(0.3);
    private static readonly TimeSpan MoveDown = TimeSpan.FromSeconds(0.7);

    private readonly Grid _layer;
    private readonly FrameworkElement _video;
    private readonly Canvas _images = new() { IsHitTestVisible = false };
    private readonly StackPanel _text = new()
    {
        HorizontalAlignment = HorizontalAlignment.Center,
        VerticalAlignment = VerticalAlignment.Bottom,
        IsHitTestVisible = false,
    };
    private readonly TranslateTransform _lift = new();
    private readonly Dictionary<string, (TimeSpan Start, TimeSpan End, long Order, IReadOnlyList<SubtitleLine> Lines)> _lines = [];
    private readonly Dictionary<string, Picture> _pictures = [];
    private SubtitleAppearance _appearance = SubtitleAppearance.Read();
    private (uint Width, uint Height) _videoSize;
    private Rect _picture;
    private Storyboard? _moving;
    private double _liftTarget;
    private double _panelTop = double.NaN;
    private long _order;

    private sealed class Picture(Image image, SubtitlePlacement placement, double canvasWidth, double canvasHeight)
    {
        public Image Image { get; } = image;
        public TimeSpan Start { get; init; }
        public TimeSpan End { get; init; }
        public SubtitlePlacement Placement { get; } = placement;
        public double CanvasWidth { get; } = canvasWidth;
        public double CanvasHeight { get; } = canvasHeight;
        public SoftwareBitmapSource? Source { get; set; }
        public bool Gone { get; set; }
    }

    /// <param name="layer">The grid over the MediaPlayerElement, under the controls; the overlay owns its children.</param>
    /// <param name="video">The MediaPlayerElement: the picture is fitted into its size.</param>
    public SubtitleOverlay(Grid layer, FrameworkElement video)
    {
        _layer = layer;
        _video = video;
        _layer.IsHitTestVisible = false;
        _layer.RenderTransform = _lift;
        _layer.Children.Add(_images);
        _layer.Children.Add(_text);
        _video.SizeChanged += (_, _) => Layout();
    }

    /// <summary>The element the panel's top is measured against: the MediaPlayerElement, which has no transform.</summary>
    public FrameworkElement Frame => _video;

    /// <summary>How many cues were shown since the overlay was made, for the smoke run.</summary>
    internal int ShownForTest { get; private set; }

    /// <summary>The text of the lines shown now, for the smoke run's comparison (never printed or logged).</summary>
    internal string TextForTest => string.Join("\n",
        OrderedLines().Select(line => string.Concat(line.Spans.Select(span => span.Text))));

    public void ShowText(string key, TimeSpan start, TimeSpan end, IReadOnlyList<SubtitleLine> lines)
    {
        var text = Text(lines);
        if (_lines.Any(shown => shown.Key != key && shown.Value.Start == start && shown.Value.End == end
            && Text(shown.Value.Lines) == text))
        {
            return;
        }
        _lines[key] = (start, end, _order++, lines);
        ShownForTest++;
        DrawText();
    }

    public void ShowImage(string key, TimeSpan start, TimeSpan end, SoftwareBitmap bitmap, SubtitlePlacement placement)
    {
        foreach (var other in _pictures.Keys.ToList())
        {
            Hide(other);
        }
        var image = new Image { Stretch = Stretch.Fill, IsHitTestVisible = false };
        var picture = new Picture(image, placement, bitmap.PixelWidth, bitmap.PixelHeight) { Start = start, End = end };
        _pictures[key] = picture;
        _images.Children.Add(image);
        ShownForTest++;
        Place(picture);
        _ = SetBitmapAsync(picture, bitmap);
    }

    /// <summary>The bitmap into the image; it is disposed once copied, and the source too if the cue went meanwhile.</summary>
    private static async Task SetBitmapAsync(Picture picture, SoftwareBitmap bitmap)
    {
        var source = new SoftwareBitmapSource();
        try
        {
            await source.SetBitmapAsync(bitmap);
            if (picture.Gone)
            {
                source.Dispose();
                return;
            }
            picture.Source = source;
            picture.Image.Source = source;
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: an image subtitle could not be shown: {error.GetType().Name}");
            source.Dispose();
        }
        finally
        {
            bitmap.Dispose();
        }
    }

    public void Hide(string key)
    {
        if (_lines.Remove(key))
        {
            DrawText();
        }
        if (_pictures.Remove(key, out var picture))
        {
            Drop(picture);
        }
    }

    public void Expire(TimeSpan position)
    {
        foreach (var key in _lines.Where(cue => !Holds(cue.Value.Start, cue.Value.End, position)).Select(cue => cue.Key).ToList())
        {
            Hide(key);
        }
        foreach (var key in _pictures.Where(cue => !Holds(cue.Value.Start, cue.Value.End, position)).Select(cue => cue.Key).ToList())
        {
            Hide(key);
        }
    }

    /// <summary>Whether a cue's time holds <paramref name="position"/>, allowing <see cref="EarlySlack"/> before its start: the
    /// position is read every 250 ms (NativeEngine) and may lag a cue that has just entered.</summary>
    internal static bool Holds(TimeSpan start, TimeSpan end, TimeSpan position) =>
        position < end && position >= start - EarlySlack;

    /// <summary>Ours, not measured: a little more than the position's reporting interval.</summary>
    internal static readonly TimeSpan EarlySlack = TimeSpan.FromSeconds(0.5);

    private static string Text(IReadOnlyList<SubtitleLine> lines) =>
        string.Join("\n", lines.Select(line => string.Concat(line.Spans.Select(span => span.Text))));

    private void Drop(Picture picture)
    {
        picture.Gone = true;
        _images.Children.Remove(picture.Image);
        picture.Image.Source = null;
        picture.Source?.Dispose();
        picture.Source = null;
    }

    public void Clear()
    {
        _lines.Clear();
        foreach (var picture in _pictures.Values)
        {
            Drop(picture);
        }
        _pictures.Clear();
        _text.Children.Clear();
    }

    public void VideoSize(uint width, uint height)
    {
        _videoSize = (width, height);
        Layout();
    }

    public void RefreshAppearance()
    {
        _appearance = SubtitleAppearance.Read();
        DrawText();
    }

    /// <summary>
    /// The control panel's top edge in the MediaPlayerElement's coordinates (<see cref="Frame"/>; NaN: hidden). The layer moves up so
    /// the text's bottom keeps <see cref="PanelGap"/> above it.
    /// </summary>
    public void PanelTop(double top)
    {
        _panelTop = top;
        Lift();
    }

    /// <summary>Moves the layer to where it should be, from where it is now (an unfinished move included).</summary>
    private void Lift()
    {
        var textBottom = _picture.Bottom - (_picture.Height * BottomShare);
        var target = double.IsNaN(_panelTop) || _picture.Height <= 0 ? 0 : Math.Min(0, _panelTop - PanelGap - textBottom);
        if (Math.Abs(target - _liftTarget) < 0.5)
        {
            return;
        }
        // Read before stopping: the animated value, which stopping would drop back to the base value.
        var from = _lift.Y;
        _moving?.Stop();
        _lift.Y = from;
        var down = target > _liftTarget;
        _liftTarget = target;
        var animation = new DoubleAnimation
        {
            From = from,
            To = target,
            Duration = down ? MoveDown : MoveUp,
        };
        Storyboard.SetTarget(animation, _lift);
        Storyboard.SetTargetProperty(animation, "Y");
        _moving = new Storyboard();
        _moving.Children.Add(animation);
        _moving.Begin();
    }

    /// <summary>The picture's rectangle: the video's size fitted into the element and centred (Stretch Uniform); the whole element
    /// while the size is not known.</summary>
    private void Layout()
    {
        var width = _video.ActualWidth;
        var height = _video.ActualHeight;
        if (_videoSize.Width > 0 && _videoSize.Height > 0 && width > 0 && height > 0)
        {
            var scale = Math.Min(width / _videoSize.Width, height / _videoSize.Height);
            var fitted = new Size(_videoSize.Width * scale, _videoSize.Height * scale);
            _picture = new Rect((width - fitted.Width) / 2, (height - fitted.Height) / 2, fitted.Width, fitted.Height);
        }
        else
        {
            _picture = new Rect(0, 0, width, height);
        }
        foreach (var picture in _pictures.Values)
        {
            Place(picture);
        }
        DrawText();
        Lift();
    }

    /// <summary>The canvas (the bitmap's own size) fitted to the picture's width, bottom on its bottom; the cue placed on it.</summary>
    private void Place(Picture picture)
    {
        if (_picture.Width <= 0 || picture.CanvasWidth <= 0 || picture.CanvasHeight <= 0)
        {
            return;
        }
        var canvasHeight = _picture.Width * picture.CanvasHeight / picture.CanvasWidth;
        var canvas = new Rect(_picture.X, _picture.Bottom - canvasHeight, _picture.Width, canvasHeight);
        var placement = picture.Placement;
        var x = placement.XyInPercent ? placement.X / 100 : placement.X / picture.CanvasWidth;
        var y = placement.XyInPercent ? placement.Y / 100 : placement.Y / picture.CanvasHeight;
        var w = placement.SizeInPercent ? placement.Width / 100 : placement.Width / picture.CanvasWidth;
        var h = placement.SizeInPercent ? placement.Height / 100 : placement.Height / picture.CanvasHeight;
        Canvas.SetLeft(picture.Image, canvas.X + (x * canvas.Width));
        Canvas.SetTop(picture.Image, canvas.Y + (y * canvas.Height));
        picture.Image.Width = Math.Max(0, w * canvas.Width);
        picture.Image.Height = Math.Max(0, h * canvas.Height);
    }

    private IEnumerable<SubtitleLine> OrderedLines() =>
        _lines.Values.OrderBy(cue => cue.Start).ThenBy(cue => cue.Order).SelectMany(cue => cue.Lines);

    private void DrawText()
    {
        _text.Children.Clear();
        if (_lines.Count == 0 || _picture.Height <= 0)
        {
            return;
        }
        var size = _picture.Height * LineShare * _appearance.SizeFactor;
        _text.MaxWidth = _picture.Width * WidthShare;
        _text.Margin = new Thickness(0, 0, 0, _layer.ActualHeight - _picture.Bottom + (_picture.Height * BottomShare));
        foreach (var line in OrderedLines())
        {
            var block = new TextBlock
            {
                FontSize = Math.Max(1, size * 0.8),
                FontFamily = _appearance.Font,
                Foreground = _appearance.Foreground,
                TextAlignment = TextAlignment.Center,
                TextWrapping = TextWrapping.WrapWholeWords,
                IsTextScaleFactorEnabled = false,
            };
            if (_appearance.SmallCaps)
            {
                Typography.SetCapitals(block, FontCapitals.SmallCaps);
            }
            foreach (var span in line.Spans)
            {
                block.Inlines.Add(new Run
                {
                    Text = span.Text,
                    FontStyle = span.Italic ? Windows.UI.Text.FontStyle.Italic : Windows.UI.Text.FontStyle.Normal,
                    FontWeight = span.Bold ? FontWeights.Bold : FontWeights.Normal,
                });
            }
            _text.Children.Add(new Border
            {
                Background = _appearance.Background,
                Padding = new Thickness(size * 0.25, 0, size * 0.25, 0),
                HorizontalAlignment = HorizontalAlignment.Center,
                Child = block,
            });
        }
    }
}
