using Microsoft.UI.Xaml.Media;
using Windows.Media.ClosedCaptioning;
using Windows.UI;

namespace Current.Shell.Player;

/// <summary>
/// How the native route's subtitles look: Windows' own caption settings (Settings, Accessibility, Captions), read through
/// <c>Windows.Media.ClosedCaptioning.ClosedCaptionProperties</c>, "the closed caption formatting settings that the user can set
/// through the system's closed captioning settings page" (ClosedCaptionProperties Class, learn.microsoft.com, ms.date 2017-02-08,
/// read 2026-10-29; Windows 10 10240). The app has no subtitle settings of its own (2026-10-29), so these are the native default.
/// Used: the computed font and background colours (<c>ComputedFontColor</c>, <c>ComputedBackgroundColor</c>), their opacities, the
/// size and the font style (small capitals as Segoe UI's small caps). Not used: the font effect (outline, shadow) and the region
/// colour. Read when a file opens; a change while one plays shows from the next file.
/// </summary>
internal sealed record SubtitleAppearance(Brush Foreground, Brush Background, FontFamily Font, double SizeFactor, bool SmallCaps)
{
    public static SubtitleAppearance Read()
    {
        try
        {
            var font = Opaque(ClosedCaptionProperties.ComputedFontColor, ClosedCaptionProperties.FontOpacity, 1.0);
            // "Default" background opacity: 75 %, ours, so the text reads on a bright picture without hiding much of it.
            var background = Opaque(ClosedCaptionProperties.ComputedBackgroundColor, ClosedCaptionProperties.BackgroundOpacity, 0.75);
            return new SubtitleAppearance(new SolidColorBrush(font), new SolidColorBrush(background),
                new FontFamily(FontFor(ClosedCaptionProperties.FontStyle)), SizeFor(ClosedCaptionProperties.FontSize),
                ClosedCaptionProperties.FontStyle == ClosedCaptionStyle.SmallCapitals);
        }
        catch (Exception error)
        {
            System.Diagnostics.Trace.TraceError($"Current: the caption settings could not be read: {error.GetType().Name}");
            return new SubtitleAppearance(new SolidColorBrush(Color.FromArgb(255, 255, 255, 255)),
                new SolidColorBrush(Color.FromArgb(191, 0, 0, 0)), new FontFamily("Segoe UI"), 1, false);
        }
    }

    private static Color Opaque(Color color, ClosedCaptionOpacity opacity, double byDefault)
    {
        var alpha = opacity switch
        {
            ClosedCaptionOpacity.OneHundredPercent => 1.0,
            ClosedCaptionOpacity.SeventyFivePercent => 0.75,
            ClosedCaptionOpacity.TwentyFivePercent => 0.25,
            ClosedCaptionOpacity.ZeroPercent => 0.0,
            _ => byDefault,
        };
        return Color.FromArgb((byte)Math.Round(alpha * 255), color.R, color.G, color.B);
    }

    private static double SizeFor(ClosedCaptionSize size) => size switch
    {
        ClosedCaptionSize.FiftyPercent => 0.5,
        ClosedCaptionSize.OneHundredFiftyPercent => 1.5,
        ClosedCaptionSize.TwoHundredPercent => 2,
        _ => 1,
    };

    /// <summary>A Windows font for each caption style (ours: the settings page names styles, not fonts).</summary>
    private static string FontFor(ClosedCaptionStyle style) => style switch
    {
        ClosedCaptionStyle.MonospacedWithSerifs => "Courier New",
        ClosedCaptionStyle.ProportionalWithSerifs => "Times New Roman",
        ClosedCaptionStyle.MonospacedWithoutSerifs => "Consolas",
        ClosedCaptionStyle.Casual => "Comic Sans MS",
        ClosedCaptionStyle.Cursive => "Segoe Script",
        _ => "Segoe UI",
    };
}
