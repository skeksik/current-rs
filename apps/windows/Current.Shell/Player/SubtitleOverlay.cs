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

/// <summary>Where an image cue goes: its position and extent, each in percent of the video or in the video's own pixels.</summary>
internal readonly record struct SubtitlePlacement(double X, double Y, bool XyInPercent, double Width, double Height, bool SizeInPercent);

/// <summary>What the native engine shows of its subtitles; every call on the UI thread.</summary>
internal interface ISubtitleSink
{
    void ShowText(string key, IReadOnlyList<SubtitleLine> lines);

    void ShowImage(string key, SoftwareBitmap bitmap, SubtitlePlacement placement);

    void Hide(string key);

    void Clear();

    /// <summary>The video's own size in pixels (0 until known), for placing cues on the picture.</summary>
    void VideoSize(uint width, uint height);

    /// <summary>The caption settings changed or a new file opened: read them again.</summary>
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
/// <item>Image cues (PGS): the bitmap stretched over its position and extent on the picture; W1 §6 saw 0,0 and 100% × 100%, the
/// full PGS canvas, a 1920×1080 premultiplied BGRA bitmap for 1080p and 4K video alike.</item>
/// <item>Above the bar: while the control panel shows, the whole layer moves up just enough for the cues to clear it, and back down
/// when it hides, with the panel's own fade times (0.3 s in, 0.7 s out; PlayerView). libmpv's subtitles do not do this yet
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
    private readonly Dictionary<string, IReadOnlyList<SubtitleLine>> _lines = [];
    private readonly Dictionary<string, (Image Image, SubtitlePlacement Placement)> _pictures = [];
    private SubtitleAppearance _appearance = SubtitleAppearance.Read();
    private (uint Width, uint Height) _videoSize;
    private Rect _picture;
    private Storyboard? _moving;
    private double _panelTop = double.NaN;

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

    /// <summary>How many cues were shown since the overlay was made, for the smoke run.</summary>
    internal int ShownForTest { get; private set; }

    /// <summary>The text of the lines shown now, for the smoke run's comparison (never printed or logged).</summary>
    internal string TextForTest => string.Join("\n",
        _lines.Values.SelectMany(lines => lines).Select(line => string.Concat(line.Spans.Select(span => span.Text))));

    public void ShowText(string key, IReadOnlyList<SubtitleLine> lines)
    {
        _lines[key] = lines;
        ShownForTest++;
        DrawText();
    }

    public void ShowImage(string key, SoftwareBitmap bitmap, SubtitlePlacement placement)
    {
        Hide(key);
        var image = new Image { Stretch = Stretch.Fill, IsHitTestVisible = false };
        _pictures[key] = (image, placement);
        _images.Children.Add(image);
        ShownForTest++;
        Place(image, placement);
        _ = SetBitmapAsync(image, bitmap);
    }

    private static async Task SetBitmapAsync(Image image, SoftwareBitmap bitmap)
    {
        try
        {
            var source = new SoftwareBitmapSource();
            await source.SetBitmapAsync(bitmap);
            image.Source = source;
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: an image subtitle could not be shown: {error.GetType().Name}");
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
            _images.Children.Remove(picture.Image);
        }
    }

    public void Clear()
    {
        _lines.Clear();
        _pictures.Clear();
        _images.Children.Clear();
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
    /// The control panel's top edge in the layer's coordinates (NaN: hidden). The layer moves up so the text's bottom keeps
    /// <see cref="PanelGap"/> above it, animated with the panel's fade times.
    /// </summary>
    public void PanelTop(double top)
    {
        _panelTop = top;
        Lift();
    }

    private void Lift()
    {
        var textBottom = _picture.Bottom - (_picture.Height * BottomShare);
        var target = double.IsNaN(_panelTop) || _picture.Height <= 0 ? 0 : Math.Min(0, _panelTop - PanelGap - textBottom);
        if (Math.Abs(target - _lift.Y) < 0.5)
        {
            return;
        }
        _moving?.Stop();
        var animation = new DoubleAnimation
        {
            To = target,
            Duration = target < _lift.Y ? MoveUp : MoveDown,
            EnableDependentAnimation = true,
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
        foreach (var (image, placement) in _pictures.Values)
        {
            Place(image, placement);
        }
        DrawText();
        Lift();
    }

    private void Place(Image image, SubtitlePlacement placement)
    {
        var x = placement.XyInPercent ? placement.X / 100 : placement.X / Math.Max(1, _videoSize.Width);
        var y = placement.XyInPercent ? placement.Y / 100 : placement.Y / Math.Max(1, _videoSize.Height);
        var w = placement.SizeInPercent ? placement.Width / 100 : placement.Width / Math.Max(1, _videoSize.Width);
        var h = placement.SizeInPercent ? placement.Height / 100 : placement.Height / Math.Max(1, _videoSize.Height);
        Canvas.SetLeft(image, _picture.X + (x * _picture.Width));
        Canvas.SetTop(image, _picture.Y + (y * _picture.Height));
        image.Width = Math.Max(0, w * _picture.Width);
        image.Height = Math.Max(0, h * _picture.Height);
    }

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
        foreach (var line in _lines.Values.SelectMany(lines => lines))
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
