using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>What the about and diagnostics screens show about the core.</summary>
public sealed record AppInfo(string Version);

/// <summary>What each configured server offers right now. Empty until server setup exists.</summary>
public sealed record Capabilities(IReadOnlyList<ServerStatus> Servers)
{
    public static Capabilities Empty { get; } = new([]);
}

/// <summary>
/// The state of one configured server. Service, features and permissions are open values: strings passed through unchanged, so a
/// value no release knows still arrives.
/// </summary>
public sealed record ServerStatus(
    string Service,
    string Key,
    bool SignedIn,
    Reachability Reachability,
    UserInfo? User,
    string? Version,
    IReadOnlyList<string> Features);

/// <summary>Whether the server answered the last time Current tried.</summary>
public enum Reachability
{
    Unknown,
    Reachable,
    Unreachable,
}

/// <summary>A user on one server.</summary>
public sealed record UserInfo(string Id, string Name, IReadOnlyList<string> Permissions);

/// <summary>Maps generated types to the public ones.</summary>
internal static class Map
{
    public static AppInfo From(Ffi.AppInfo info) => new(info.Version);

    public static Capabilities From(Ffi.CapabilitiesSnapshot snapshot) =>
        new(Array.ConvertAll(snapshot.Servers, status => From(status)));

    public static ServerStatus From(Ffi.ServerStatusSnapshot status) => new(
        status.Service,
        status.Key,
        status.SignedIn,
        From(status.Reachability),
        status.User is { } user ? new UserInfo(user.Id, user.Name, user.Permissions) : null,
        status.Version,
        status.Features);

    public static Reachability From(Ffi.Reachability reachability) => reachability switch
    {
        Ffi.Reachability.Reachable => Reachability.Reachable,
        Ffi.Reachability.Unreachable => Reachability.Unreachable,
        _ => Reachability.Unknown,
    };
}
