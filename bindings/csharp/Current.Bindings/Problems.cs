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
        _ => throw new ArgumentException($"unknown problem {problem.GetType().Name}", nameof(problem)),
    };

    public static DiscoveryProblem From(Ffi.DiscoveryProblem problem) => problem switch
    {
        Ffi.DiscoveryProblem.NotPermitted => DiscoveryProblem.NotPermitted,
        Ffi.DiscoveryProblem.NoNetwork => DiscoveryProblem.NoNetwork,
        _ => DiscoveryProblem.Failed,
    };
}
