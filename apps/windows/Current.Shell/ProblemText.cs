using Current.Bindings;

namespace Current.Shell;

/// <summary>
/// The core's problems in plain words for the UI (docs/notes/windows-shell.md, "Screens"). Never an address, path, token or network key:
/// the problems hold none, and the texts add none.
/// </summary>
internal static class ProblemText
{
    public const string Unexpected = "Something went wrong in Current. Try again.";

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
