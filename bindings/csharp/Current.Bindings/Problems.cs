using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>Why the typed address is not one: the UI asks the user to fix it.</summary>
public enum AddressProblem
{
    Empty,
    /// <summary>A scheme other than http or https.</summary>
    UnsupportedScheme,
    /// <summary>A user name or password in the address.</summary>
    Credentials,
    Invalid,
}

/// <summary>Why server setup found no usable server at an address. Nothing in it holds the address.</summary>
public abstract record ProbeProblem
{
    private ProbeProblem() { }

    public sealed record InvalidAddress(AddressProblem Reason) : ProbeProblem;
    /// <summary>Nothing answered: offline, the name did not resolve, or nothing listens there.</summary>
    public sealed record Unreachable : ProbeProblem;
    public sealed record Timeout : ProbeProblem;
    /// <summary>The secure connection failed, typically an untrusted or mismatched certificate.</summary>
    public sealed record Tls : ProbeProblem;
    public sealed record NotJellyfin : ProbeProblem;
    /// <summary>The server is starting, or does not accept connections from this network.</summary>
    public sealed record Unavailable : ProbeProblem;
    /// <summary>The server points elsewhere; where stays in the core.</summary>
    public sealed record Redirected(int Status) : ProbeProblem;
    public sealed record UnexpectedStatus(int Status) : ProbeProblem;
    public sealed record TooOld(string Version) : ProbeProblem;
    public sealed record SetupIncomplete : ProbeProblem;
    /// <summary>The address leads to another server than the one expected.</summary>
    public sealed record DifferentServer : ProbeProblem;
    /// <summary>
    /// An authentication gateway in front of the server wants something (ADR 0017). Setup's gateway states say what to do; this
    /// appears where no flow can ask, e.g. a saved address checked before a sign-in.
    /// </summary>
    public sealed record Gateway(GatewayProblem Answer) : ProbeProblem;
}

/// <summary>What an authentication gateway in front of the server wants (ADR 0017). Never its realm or any text it sent.</summary>
public enum GatewayProblem
{
    /// <summary>A password (HTTP Basic).</summary>
    Password,
    /// <summary>A sign-in in a web page. Also what a plain web site at the address looks like.</summary>
    SignIn,
    /// <summary>It refuses without a sign-in: it wants something else, such as a service token in extra headers.</summary>
    Refused,
    /// <summary>A sign-in method Current does not support.</summary>
    Unsupported,
}

/// <summary>Why the gateway credentials handed over were not used (ADR 0017). Nothing of what the user typed.</summary>
public enum GatewayInputProblem
{
    /// <summary>The address is plain http outside the local network: confirm it first.</summary>
    PlainHttpNotConfirmed,
    NoHeaders,
    /// <summary>A user name with <c>:</c> or a control character.</summary>
    InvalidUserName,
    /// <summary>A password with a control character.</summary>
    InvalidPassword,
    InvalidHeaderName,
    /// <summary>A header Current sets itself (<c>Authorization</c>, <c>Cookie</c>, <c>Host</c>, the framing headers).</summary>
    ReservedHeader,
    /// <summary>A header value that is not visible ASCII text.</summary>
    InvalidHeaderValue,
    InvalidCookie,
}

/// <summary>Why signing in to a saved server's gateway again failed.</summary>
public abstract record GatewaySignInProblem
{
    private GatewaySignInProblem() { }

    /// <summary>The address could not be checked: not reached, another server, or not a usable server.</summary>
    public sealed record Probe(ProbeProblem Problem) : GatewaySignInProblem;
    /// <summary>The new credentials could not be saved.</summary>
    public sealed record Save(StoreProblem Problem) : GatewaySignInProblem;
    public sealed record ServerRemoved : GatewaySignInProblem;
}

/// <summary>Why a request failed.</summary>
public abstract record NetProblem
{
    private NetProblem() { }

    public sealed record Unreachable : NetProblem;
    public sealed record Timeout : NetProblem;
    public sealed record Tls : NetProblem;
    /// <summary>401 or 403.</summary>
    public sealed record Auth(int Status) : NetProblem;
    public sealed record UnexpectedStatus(int Status) : NetProblem;
    public sealed record Redirected(int Status) : NetProblem;
    public sealed record Decode : NetProblem;
    /// <summary>The request could not be built: a bug, not a server problem.</summary>
    public sealed record InvalidRequest : NetProblem;
}

/// <summary>Why the settings could not be read or saved. No path: it holds the user's account name.</summary>
public abstract record StoreProblem
{
    private StoreProblem() { }

    /// <summary>Reading or writing the data folder failed: <c>Step</c> says which, <c>Kind</c> the kind of I/O error.</summary>
    public sealed record Io(string Step, string Kind) : StoreProblem;
    /// <summary>The settings belong to a newer version of Current: they are used read-only.</summary>
    public sealed record NewerVersion(ulong Found) : StoreProblem;
    public sealed record Random : StoreProblem;
}

/// <summary>Why a sign-in failed.</summary>
public abstract record SignInProblem
{
    private SignInProblem() { }

    /// <summary>Unknown user or wrong password.</summary>
    public sealed record WrongCredentials : SignInProblem;
    /// <summary>The server knows the user but refuses them.</summary>
    public sealed record Refused : SignInProblem;
    public sealed record QuickConnectDisabled : SignInProblem;
    public sealed record QuickConnectExpired : SignInProblem;
    public sealed record SessionRevoked : SignInProblem;
    public sealed record NotSignedIn : SignInProblem;
    public sealed record Network(NetProblem Problem) : SignInProblem;
    /// <summary>The saved address no longer answers as the saved server.</summary>
    public sealed record Probe(ProbeProblem Problem) : SignInProblem;
    /// <summary>Signed in, but the sign-in could not be saved.</summary>
    public sealed record Save(StoreProblem Problem) : SignInProblem;
    public sealed record ServerRemoved : SignInProblem;
    /// <summary>An authentication gateway in front of the server answered instead (ADR 0017): sign in to it first.</summary>
    public sealed record Gateway(GatewayProblem Answer) : SignInProblem;
    /// <summary>
    /// The server ignored the sign-in's client details sent in <c>X-Emby-Authorization</c> (ADR 0017). Explain the two server-side
    /// fixes: turn on legacy authorization on the server and stop the gateway passing its own <c>Authorization</c> header on; or
    /// add a gateway rule that lets the server's API through.
    /// </summary>
    public sealed record EmbyHeaderIgnored : SignInProblem;
    /// <summary>The server's answer is being checked (mid-session only): try again in a moment.</summary>
    public sealed record Unconfirmed : SignInProblem;
}

/// <summary>Why the search for local servers failed.</summary>
public enum DiscoveryProblem
{
    /// <summary>The system refused.</summary>
    NotPermitted,
    NoNetwork,
    Failed,
}

internal static class MapProblems
{
    public static AddressProblem From(Ffi.AddressProblem problem) => problem switch
    {
        Ffi.AddressProblem.Empty => AddressProblem.Empty,
        Ffi.AddressProblem.UnsupportedScheme => AddressProblem.UnsupportedScheme,
        Ffi.AddressProblem.Credentials => AddressProblem.Credentials,
        _ => AddressProblem.Invalid,
    };

    public static ProbeProblem From(Ffi.ProbeProblem problem) => problem switch
    {
        Ffi.ProbeProblem.InvalidAddress invalid => new ProbeProblem.InvalidAddress(From(invalid.Reason)),
        Ffi.ProbeProblem.Unreachable => new ProbeProblem.Unreachable(),
        Ffi.ProbeProblem.Timeout => new ProbeProblem.Timeout(),
        Ffi.ProbeProblem.Tls => new ProbeProblem.Tls(),
        Ffi.ProbeProblem.NotJellyfin => new ProbeProblem.NotJellyfin(),
        Ffi.ProbeProblem.Unavailable => new ProbeProblem.Unavailable(),
        Ffi.ProbeProblem.Redirected redirected => new ProbeProblem.Redirected(redirected.Status),
        Ffi.ProbeProblem.UnexpectedStatus status => new ProbeProblem.UnexpectedStatus(status.Status),
        Ffi.ProbeProblem.TooOld old => new ProbeProblem.TooOld(old.Version),
        Ffi.ProbeProblem.SetupIncomplete => new ProbeProblem.SetupIncomplete(),
        Ffi.ProbeProblem.DifferentServer => new ProbeProblem.DifferentServer(),
        Ffi.ProbeProblem.Gateway gateway => new ProbeProblem.Gateway(From(gateway.Answer)),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static NetProblem From(Ffi.NetProblem problem) => problem switch
    {
        Ffi.NetProblem.Unreachable => new NetProblem.Unreachable(),
        Ffi.NetProblem.Timeout => new NetProblem.Timeout(),
        Ffi.NetProblem.Tls => new NetProblem.Tls(),
        Ffi.NetProblem.Auth auth => new NetProblem.Auth(auth.Status),
        Ffi.NetProblem.UnexpectedStatus status => new NetProblem.UnexpectedStatus(status.Status),
        Ffi.NetProblem.Redirected redirected => new NetProblem.Redirected(redirected.Status),
        Ffi.NetProblem.Decode => new NetProblem.Decode(),
        Ffi.NetProblem.InvalidRequest => new NetProblem.InvalidRequest(),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static StoreProblem From(Ffi.StoreProblem problem) => problem switch
    {
        Ffi.StoreProblem.Io io => new StoreProblem.Io(io.Step, io.Kind),
        Ffi.StoreProblem.NewerVersion newer => new StoreProblem.NewerVersion(newer.Found),
        Ffi.StoreProblem.Random => new StoreProblem.Random(),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static SignInProblem From(Ffi.SignInProblem problem) => problem switch
    {
        Ffi.SignInProblem.WrongCredentials => new SignInProblem.WrongCredentials(),
        Ffi.SignInProblem.Refused => new SignInProblem.Refused(),
        Ffi.SignInProblem.QuickConnectDisabled => new SignInProblem.QuickConnectDisabled(),
        Ffi.SignInProblem.QuickConnectExpired => new SignInProblem.QuickConnectExpired(),
        Ffi.SignInProblem.SessionRevoked => new SignInProblem.SessionRevoked(),
        Ffi.SignInProblem.NotSignedIn => new SignInProblem.NotSignedIn(),
        Ffi.SignInProblem.Network network => new SignInProblem.Network(From(network.Problem)),
        Ffi.SignInProblem.Probe probe => new SignInProblem.Probe(From(probe.Problem)),
        Ffi.SignInProblem.Save save => new SignInProblem.Save(From(save.Problem)),
        Ffi.SignInProblem.ServerRemoved => new SignInProblem.ServerRemoved(),
        Ffi.SignInProblem.Gateway gateway => new SignInProblem.Gateway(From(gateway.Answer)),
        Ffi.SignInProblem.EmbyHeaderIgnored => new SignInProblem.EmbyHeaderIgnored(),
        Ffi.SignInProblem.Unconfirmed => new SignInProblem.Unconfirmed(),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static GatewayProblem From(Ffi.GatewayProblem problem) => problem switch
    {
        Ffi.GatewayProblem.Password => GatewayProblem.Password,
        Ffi.GatewayProblem.SignIn => GatewayProblem.SignIn,
        Ffi.GatewayProblem.Refused => GatewayProblem.Refused,
        _ => GatewayProblem.Unsupported,
    };

    public static GatewayInputProblem From(Ffi.GatewayInputProblem problem) => problem switch
    {
        Ffi.GatewayInputProblem.PlainHttpNotConfirmed => GatewayInputProblem.PlainHttpNotConfirmed,
        Ffi.GatewayInputProblem.NoHeaders => GatewayInputProblem.NoHeaders,
        Ffi.GatewayInputProblem.InvalidUserName => GatewayInputProblem.InvalidUserName,
        Ffi.GatewayInputProblem.InvalidPassword => GatewayInputProblem.InvalidPassword,
        Ffi.GatewayInputProblem.InvalidHeaderName => GatewayInputProblem.InvalidHeaderName,
        Ffi.GatewayInputProblem.ReservedHeader => GatewayInputProblem.ReservedHeader,
        Ffi.GatewayInputProblem.InvalidHeaderValue => GatewayInputProblem.InvalidHeaderValue,
        _ => GatewayInputProblem.InvalidCookie,
    };

    public static GatewaySignInProblem From(Ffi.GatewaySignInProblem problem) => problem switch
    {
        Ffi.GatewaySignInProblem.Probe probe => new GatewaySignInProblem.Probe(From(probe.Problem)),
        Ffi.GatewaySignInProblem.Save save => new GatewaySignInProblem.Save(From(save.Problem)),
        Ffi.GatewaySignInProblem.ServerRemoved => new GatewaySignInProblem.ServerRemoved(),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static DiscoveryProblem From(Ffi.DiscoveryProblem problem) => problem switch
    {
        Ffi.DiscoveryProblem.NotPermitted => DiscoveryProblem.NotPermitted,
        Ffi.DiscoveryProblem.NoNetwork => DiscoveryProblem.NoNetwork,
        _ => DiscoveryProblem.Failed,
    };

    // Back to the core's shapes, for its texts.

    public static Ffi.AddressProblem ToFfi(AddressProblem problem) => problem switch
    {
        AddressProblem.Empty => Ffi.AddressProblem.Empty,
        AddressProblem.UnsupportedScheme => Ffi.AddressProblem.UnsupportedScheme,
        AddressProblem.Credentials => Ffi.AddressProblem.Credentials,
        AddressProblem.Invalid => Ffi.AddressProblem.Invalid,
        _ => throw new ArgumentOutOfRangeException(nameof(problem), problem, "unknown problem"),
    };

    public static Ffi.ProbeProblem ToFfi(ProbeProblem problem) => problem switch
    {
        ProbeProblem.InvalidAddress invalid => new Ffi.ProbeProblem.InvalidAddress(ToFfi(invalid.Reason)),
        ProbeProblem.Unreachable => new Ffi.ProbeProblem.Unreachable(),
        ProbeProblem.Timeout => new Ffi.ProbeProblem.Timeout(),
        ProbeProblem.Tls => new Ffi.ProbeProblem.Tls(),
        ProbeProblem.NotJellyfin => new Ffi.ProbeProblem.NotJellyfin(),
        ProbeProblem.Unavailable => new Ffi.ProbeProblem.Unavailable(),
        ProbeProblem.Redirected redirected => new Ffi.ProbeProblem.Redirected(Status(redirected.Status)),
        ProbeProblem.UnexpectedStatus status => new Ffi.ProbeProblem.UnexpectedStatus(Status(status.Status)),
        ProbeProblem.TooOld old => new Ffi.ProbeProblem.TooOld(old.Version),
        ProbeProblem.SetupIncomplete => new Ffi.ProbeProblem.SetupIncomplete(),
        ProbeProblem.DifferentServer => new Ffi.ProbeProblem.DifferentServer(),
        ProbeProblem.Gateway gateway => new Ffi.ProbeProblem.Gateway(ToFfi(gateway.Answer)),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static Ffi.NetProblem ToFfi(NetProblem problem) => problem switch
    {
        NetProblem.Unreachable => new Ffi.NetProblem.Unreachable(),
        NetProblem.Timeout => new Ffi.NetProblem.Timeout(),
        NetProblem.Tls => new Ffi.NetProblem.Tls(),
        NetProblem.Auth auth => new Ffi.NetProblem.Auth(Status(auth.Status)),
        NetProblem.UnexpectedStatus status => new Ffi.NetProblem.UnexpectedStatus(Status(status.Status)),
        NetProblem.Redirected redirected => new Ffi.NetProblem.Redirected(Status(redirected.Status)),
        NetProblem.Decode => new Ffi.NetProblem.Decode(),
        NetProblem.InvalidRequest => new Ffi.NetProblem.InvalidRequest(),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static Ffi.StoreProblem ToFfi(StoreProblem problem) => problem switch
    {
        StoreProblem.Io io => new Ffi.StoreProblem.Io(io.Step, io.Kind),
        StoreProblem.NewerVersion newer => new Ffi.StoreProblem.NewerVersion(newer.Found),
        StoreProblem.Random => new Ffi.StoreProblem.Random(),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static Ffi.SignInProblem ToFfi(SignInProblem problem) => problem switch
    {
        SignInProblem.WrongCredentials => new Ffi.SignInProblem.WrongCredentials(),
        SignInProblem.Refused => new Ffi.SignInProblem.Refused(),
        SignInProblem.QuickConnectDisabled => new Ffi.SignInProblem.QuickConnectDisabled(),
        SignInProblem.QuickConnectExpired => new Ffi.SignInProblem.QuickConnectExpired(),
        SignInProblem.SessionRevoked => new Ffi.SignInProblem.SessionRevoked(),
        SignInProblem.NotSignedIn => new Ffi.SignInProblem.NotSignedIn(),
        SignInProblem.Network network => new Ffi.SignInProblem.Network(ToFfi(network.Problem)),
        SignInProblem.Probe probe => new Ffi.SignInProblem.Probe(ToFfi(probe.Problem)),
        SignInProblem.Save save => new Ffi.SignInProblem.Save(ToFfi(save.Problem)),
        SignInProblem.ServerRemoved => new Ffi.SignInProblem.ServerRemoved(),
        SignInProblem.Gateway gateway => new Ffi.SignInProblem.Gateway(ToFfi(gateway.Answer)),
        SignInProblem.EmbyHeaderIgnored => new Ffi.SignInProblem.EmbyHeaderIgnored(),
        SignInProblem.Unconfirmed => new Ffi.SignInProblem.Unconfirmed(),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static Ffi.GatewayProblem ToFfi(GatewayProblem problem) => problem switch
    {
        GatewayProblem.Password => Ffi.GatewayProblem.Password,
        GatewayProblem.SignIn => Ffi.GatewayProblem.SignIn,
        GatewayProblem.Refused => Ffi.GatewayProblem.Refused,
        GatewayProblem.Unsupported => Ffi.GatewayProblem.Unsupported,
        _ => throw new ArgumentOutOfRangeException(nameof(problem), problem, "unknown problem"),
    };

    public static Ffi.GatewayInputProblem ToFfi(GatewayInputProblem problem) => problem switch
    {
        GatewayInputProblem.PlainHttpNotConfirmed => Ffi.GatewayInputProblem.PlainHttpNotConfirmed,
        GatewayInputProblem.NoHeaders => Ffi.GatewayInputProblem.NoHeaders,
        GatewayInputProblem.InvalidUserName => Ffi.GatewayInputProblem.InvalidUserName,
        GatewayInputProblem.InvalidPassword => Ffi.GatewayInputProblem.InvalidPassword,
        GatewayInputProblem.InvalidHeaderName => Ffi.GatewayInputProblem.InvalidHeaderName,
        GatewayInputProblem.ReservedHeader => Ffi.GatewayInputProblem.ReservedHeader,
        GatewayInputProblem.InvalidHeaderValue => Ffi.GatewayInputProblem.InvalidHeaderValue,
        GatewayInputProblem.InvalidCookie => Ffi.GatewayInputProblem.InvalidCookie,
        _ => throw new ArgumentOutOfRangeException(nameof(problem), problem, "unknown problem"),
    };

    public static Ffi.GatewaySignInProblem ToFfi(GatewaySignInProblem problem) => problem switch
    {
        GatewaySignInProblem.Probe probe => new Ffi.GatewaySignInProblem.Probe(ToFfi(probe.Problem)),
        GatewaySignInProblem.Save save => new Ffi.GatewaySignInProblem.Save(ToFfi(save.Problem)),
        GatewaySignInProblem.ServerRemoved => new Ffi.GatewaySignInProblem.ServerRemoved(),
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static Ffi.DiscoveryProblem ToFfi(DiscoveryProblem problem) => problem switch
    {
        DiscoveryProblem.NotPermitted => Ffi.DiscoveryProblem.NotPermitted,
        DiscoveryProblem.NoNetwork => Ffi.DiscoveryProblem.NoNetwork,
        DiscoveryProblem.Failed => Ffi.DiscoveryProblem.Failed,
        _ => throw new ArgumentOutOfRangeException(nameof(problem), problem, "unknown problem"),
    };

    /// <summary>An HTTP status as the core holds it (16 bits); the problems only ever hold the core's own.</summary>
    private static ushort Status(int status) => checked((ushort)status);
}
