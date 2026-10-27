using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

// The core's problems in plain words, the same on every screen. Never an address, path, token or network key: the problems hold none,
// and the core's texts add none. The few words that name the platform ("this PC", "Windows") come from the platform passed once
// (CoreHost.SetPlatform), so those texts are the core's (CoreHost); the rest need no core.

/// <summary>The platform the app runs on, for the few texts that name it.</summary>
public enum Platform
{
    Windows,
    MacOs,
    Linux,
}

/// <summary>The problems' texts that name no platform (the core's words).</summary>
public static class ProblemTexts
{
    /// <summary>"Something went wrong in Current. Try again."</summary>
    public static string Unexpected => Ffi.CurrentFfiMethods.UnexpectedProblemText();

    /// <summary>A gateway that asks for a sign-in method Current does not support (ADR 0017).</summary>
    public static string GatewayUnsupported => Ffi.CurrentFfiMethods.GatewayUnsupportedText();

    /// <summary>The add-server page's text for a web sign-in that is not a redirect (AddServerState.GatewaySignIn).</summary>
    public static string GatewayWebPage => Ffi.CurrentFfiMethods.GatewayWebPageText();

    /// <summary>A gateway that refuses without a sign-in (AddServerState.GatewayRefused): extra headers are the way in.</summary>
    public static string GatewayRefused => Ffi.CurrentFfiMethods.GatewayRefusedText();

    /// <summary>The sign-in window's text in place of Google's sign-in, which refuses apps' web views (ADR 0017, "Google").</summary>
    public static string GoogleRefusesWebViews => Ffi.CurrentFfiMethods.GoogleRefusesWebViewsText();

    /// <summary>What a gateway in front of the address wants, as setup says it.</summary>
    public static string Gateway(GatewayProblem problem) => Ffi.CurrentFfiMethods.GatewayProblemText(MapProblems.ToFfi(problem));

    /// <summary>Why the gateway credentials were not used: nothing was sent.</summary>
    public static string GatewayInput(GatewayInputProblem problem) =>
        Ffi.CurrentFfiMethods.GatewayInputProblemText(MapProblems.ToFfi(problem));

    /// <summary>The step and kind of a store error. No path: it holds the user's account name.</summary>
    public static string Store(StoreProblem problem)
    {
        ArgumentNullException.ThrowIfNull(problem);
        return Ffi.CurrentFfiMethods.StoreProblemText(MapProblems.ToFfi(problem));
    }
}

public sealed partial class CoreHost
{
    /// <summary>The platform the app runs on, for the few texts that name it ("this PC", "Windows"): pass it once at start.</summary>
    public void SetPlatform(Platform platform) => _core.SetPlatform(platform switch
    {
        Platform.Windows => Ffi.Platform.Windows,
        Platform.MacOs => Ffi.Platform.MacOs,
        Platform.Linux => Ffi.Platform.Linux,
        _ => throw new ArgumentOutOfRangeException(nameof(platform), platform, "unknown platform"),
    });

    /// <summary>Why setup found no usable server at an address, in plain words.</summary>
    public string ProblemText(ProbeProblem problem)
    {
        ArgumentNullException.ThrowIfNull(problem);
        return _core.ProbeProblemText(MapProblems.ToFfi(problem));
    }

    /// <summary>Why a sign-in failed, in plain words.</summary>
    public string ProblemText(SignInProblem problem)
    {
        ArgumentNullException.ThrowIfNull(problem);
        return _core.SignInProblemText(MapProblems.ToFfi(problem));
    }

    /// <summary>Why signing in to a saved server's gateway again failed, in plain words.</summary>
    public string ProblemText(GatewaySignInProblem problem)
    {
        ArgumentNullException.ThrowIfNull(problem);
        return _core.GatewaySignInProblemText(MapProblems.ToFfi(problem));
    }

    /// <summary>Why a request failed, in plain words.</summary>
    public string ProblemText(NetProblem problem)
    {
        ArgumentNullException.ThrowIfNull(problem);
        return _core.NetProblemText(MapProblems.ToFfi(problem));
    }

    /// <summary>Why the search for servers on the local network failed, in plain words.</summary>
    public string ProblemText(DiscoveryProblem problem) => _core.DiscoveryProblemText(MapProblems.ToFfi(problem));

    /// <summary>Why a browse request failed, in plain words.</summary>
    public string ProblemText(BrowseProblem problem)
    {
        ArgumentNullException.ThrowIfNull(problem);
        return _core.BrowseProblemText(MapBrowse.ToFfi(problem));
    }
}
