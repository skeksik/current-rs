using Current.Bindings;

namespace Current.Shell;

/// <summary>
/// The core's problems in plain words for the UI (docs/notes/windows-shell.md, "Screens"). Never an address, path, token or network key:
/// the problems hold none, and the texts add none.
/// </summary>
internal static class ProblemText
{
    public const string Unexpected = "Something went wrong in Current. Try again.";

    /// <summary>A gateway that asks for a sign-in method Current does not support (ADR 0017).</summary>
    public const string GatewayUnsupported = "The server asks for a sign-in method Current does not support.";

    /// <summary>The add-server page's text for a web sign-in that is not a redirect (AddServerState.GatewaySignIn).</summary>
    public const string GatewayWebPage =
        "This address shows a web page. If it is your server's sign-in, sign in; otherwise check the address.";

    /// <summary>A gateway that refuses without a sign-in (AddServerState.GatewayRefused): extra headers are the way in.</summary>
    public const string GatewayRefused =
        "This address refuses Current without a sign-in. If your gateway gave you a service token or another secret header, add it "
        + "under Advanced.";

    /// <summary>
    /// The sign-in window's text in place of Google's sign-in, which refuses apps' web views (ADR 0017, "Google"). The web view is
    /// never disguised as a browser.
    /// </summary>
    public const string GoogleRefusesWebViews =
        "Google doesn't allow signing in inside apps. Use another sign-in method of your gateway (for example a one-time code), or "
        + "a service token.";

    /// <summary>
    /// The sign-in's client details sent in X-Emby-Authorization were ignored (SignInProblem.EmbyHeaderIgnored): the two
    /// server-side fixes from docs/notes/gateway-auth.md, "Which headers carry the token".
    /// </summary>
    public const string EmbyHeaderIgnored =
        "The server ignored Current's sign-in because the gateway in front of it uses the Authorization header for its own password. "
        + "Change one thing on the server side: turn on EnableLegacyAuthorization in the server's system.xml and set the gateway not to "
        + "pass its own Authorization header on (in Nginx Proxy Manager, leave \"Pass Auth to Upstream\" off); or add a gateway rule "
        + "that lets the server's API through without the password.";

    public static string Gateway(GatewayProblem problem) => problem switch
    {
        GatewayProblem.Password => "This address asks for a password of its own before the server.",
        GatewayProblem.SignIn => "This address shows a sign-in page before the server.",
        GatewayProblem.Refused => "This address refuses Current without a sign-in.",
        _ => GatewayUnsupported,
    };

    /// <summary>Why the gateway credentials were not used: nothing was sent (GatewayInputException).</summary>
    public static string GatewayInput(GatewayInputProblem problem) => problem switch
    {
        GatewayInputProblem.PlainHttpNotConfirmed => "Confirm the connection without encryption first.",
        GatewayInputProblem.NoHeaders => "Add at least one header.",
        GatewayInputProblem.InvalidUserName => "The user name can't contain a colon or control characters.",
        GatewayInputProblem.InvalidPassword => "The password can't contain control characters.",
        GatewayInputProblem.InvalidHeaderName => "A header name can only have letters, digits and the characters ! # $ % & ' * + - . ^ _ ` | ~.",
        GatewayInputProblem.ReservedHeader =>
            "Current sets Authorization, Cookie, Host, Content-Length and the other connection headers itself, so they can't be "
            + "used here.",
        GatewayInputProblem.InvalidHeaderValue => "A header value can only have visible ASCII characters and spaces.",
        _ => "The sign-in page's cookies couldn't be used.",
    };

    public static string Probe(ProbeProblem problem) => problem switch
    {
        ProbeProblem.InvalidAddress invalid => invalid.Reason switch
        {
            AddressProblem.Empty => "Enter the server's address.",
            AddressProblem.UnsupportedScheme => "Use an address that starts with http:// or https://.",
            AddressProblem.Credentials => "Leave the user name and password out of the address. You'll sign in next.",
            _ => "That isn't a valid address.",
        },
        ProbeProblem.Unreachable => "Nothing answered at that address. Check the address and that the server is on.",
        ProbeProblem.Timeout => "The server took too long to answer.",
        ProbeProblem.Tls => "The secure connection failed. The server's certificate may not be trusted by this PC.",
        ProbeProblem.NotJellyfin => "That address doesn't lead to a Jellyfin server.",
        ProbeProblem.Unavailable => "The server is starting or doesn't accept connections from this network. Try again in a moment.",
        ProbeProblem.Redirected redirected => $"The server sent Current to another address (HTTP {redirected.Status}).",
        ProbeProblem.UnexpectedStatus status => $"The server answered with an unexpected error (HTTP {status.Status}).",
        ProbeProblem.TooOld old => $"This server runs Jellyfin {old.Version}, which Current doesn't support. Update the server.",
        ProbeProblem.SetupIncomplete => "This server hasn't finished its first-time setup. Finish it in the server's web page.",
        ProbeProblem.DifferentServer => "That address now leads to a different server.",
        ProbeProblem.Gateway gateway => Gateway(gateway.Answer),
        _ => Unexpected,
    };

    public static string SignIn(SignInProblem problem) => problem switch
    {
        SignInProblem.WrongCredentials => "The user name or password is incorrect.",
        SignInProblem.Refused => "The server refused this user. Ask the server's administrator.",
        SignInProblem.QuickConnectDisabled => "Quick Connect is turned off on this server.",
        SignInProblem.QuickConnectExpired => "The code expired. Try again for a new one.",
        SignInProblem.SessionRevoked or SignInProblem.NotSignedIn => "The server ended the session. Sign in again.",
        SignInProblem.Network network => Net(network.Problem),
        SignInProblem.Probe probe => Probe(probe.Problem),
        SignInProblem.Save { Problem: StoreProblem.NewerVersion } =>
            "These settings are from a newer version of Current, so this sign-in can't be saved. Update Current.",
        SignInProblem.Save => "Current couldn't save the sign-in to its settings folder.",
        SignInProblem.ServerRemoved => "This server was removed.",
        SignInProblem.Gateway gateway => gateway.Answer == GatewayProblem.Unsupported
            ? GatewayUnsupported
            : "The gateway in front of the server wants you to sign in to it first.",
        SignInProblem.EmbyHeaderIgnored => EmbyHeaderIgnored,
        SignInProblem.Unconfirmed => "The server's answer is being checked. Try again in a moment.",
        _ => Unexpected,
    };

    /// <summary>Why signing in to a saved server's gateway again failed.</summary>
    public static string GatewaySignIn(GatewaySignInProblem problem) => problem switch
    {
        GatewaySignInProblem.Probe probe => Probe(probe.Problem),
        GatewaySignInProblem.Save { Problem: StoreProblem.NewerVersion } =>
            "These settings are from a newer version of Current, so this sign-in can't be saved. Update Current.",
        GatewaySignInProblem.Save => "Current couldn't save the sign-in to its settings folder.",
        GatewaySignInProblem.ServerRemoved => "This server was removed.",
        _ => Unexpected,
    };

    public static string Net(NetProblem problem) => problem switch
    {
        NetProblem.Unreachable => "Can't reach the server.",
        NetProblem.Timeout => "The server took too long to answer.",
        NetProblem.Tls => "The secure connection failed. The server's certificate may not be trusted by this PC.",
        NetProblem.Auth auth => $"The server answered with an error (HTTP {auth.Status}).",
        NetProblem.UnexpectedStatus status => $"The server answered with an error (HTTP {status.Status}).",
        NetProblem.Redirected redirected => $"The server answered with an error (HTTP {redirected.Status}).",
        NetProblem.Decode => "The server's answer couldn't be read.",
        _ => Unexpected,
    };

    public static string Discovery(DiscoveryProblem problem) => problem switch
    {
        DiscoveryProblem.NotPermitted => "Windows didn't allow Current to search the network.",
        DiscoveryProblem.NoNetwork => "You're not connected to a network.",
        _ => "The search didn't work. Enter the address instead.",
    };

    /// <summary>The step and kind of a store error. No path: it holds the user's account name.</summary>
    public static string Store(StoreProblem problem) => problem switch
    {
        StoreProblem.Io io => $"Step: {io.Step}. Error: {io.Kind}.",
        StoreProblem.NewerVersion newer => $"The settings are from a newer version of Current (format {newer.Found}).",
        _ => "No secure random numbers were available.",
    };
}
