using Microsoft.UI.Xaml;

namespace VideoSpike;

public partial class App : Application
{
    MainWindow? _window;

    public App() => InitializeComponent();

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _window = new MainWindow(Options.Parse(Environment.GetCommandLineArgs().Skip(1).ToArray()));
        _window.Activate();
    }
}

/// <summary>Command line for scripted runs: --file PATH --measure SECONDS --exit --fullscreen --size WxH --pos X,Y --colour MODE --transitions --loop.</summary>
public sealed record Options(string? File, int MeasureSeconds, bool Exit, bool FullScreen, string? Size, string? Pos,
    string Colour, string DataDir, bool Transitions, bool Loop)
{
    public static Options Parse(string[] a)
    {
        string? Get(string name) { var i = Array.IndexOf(a, name); return i >= 0 && i + 1 < a.Length ? a[i + 1] : null; }
        return new Options(Get("--file"), int.TryParse(Get("--measure"), out var m) ? m : 0, a.Contains("--exit"),
            a.Contains("--fullscreen"), Get("--size"), Get("--pos"), Get("--colour") ?? "auto",
            Get("--data") ?? @"C:\crs2-data\logs", a.Contains("--transitions"), a.Contains("--loop"));
    }
}
