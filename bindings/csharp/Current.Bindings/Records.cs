using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>What the about and diagnostics screens show about the core.</summary>
public sealed record AppInfo(string Version);

/// <summary>What each configured server offers right now. Empty until server setup exists.</summary>
/// <remarks>
/// The records here hold lists, and record equality compares lists by reference: two equal values from two deliveries are not
/// <c>==</c>. Do not use <c>==</c> to skip an unchanged value; the core already sends only changes.
/// </remarks>
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
    /// <summary>Which saved address answers, counted from 1 in the user's order; null while none does.</summary>
    int? AddressPosition,
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

/// <summary>A server as calls name it: its service and the key the app gave it. Never an address.</summary>
public sealed record ServerRef(string Service, string Key);

/// <summary>A saved server. Its addresses stay in the core: only how many there are.</summary>
public sealed record SavedServer(
    ServerRef Server,
    string Id,
    string Name,
    int AddressCount,
    IReadOnlyList<SavedUser> Users,
    string? ActiveUser);

/// <summary>A user who signed in on a saved server.</summary>
public sealed record SavedUser(string Id, string Name);

/// <summary>What the settings store found when the core opened.</summary>
public abstract record LoadReport
{
    private LoadReport() { }

    /// <summary>First start: new settings were written.</summary>
    public sealed record Created : LoadReport;
    public sealed record Loaded : LoadReport;
    /// <summary>The settings were in an older format, migrated and saved.</summary>
    public sealed record Migrated(ulong From) : LoadReport;
    /// <summary>The settings could not be read: kept as <c>BackupFile</c> (a file name in the data folder) and reset.</summary>
    public sealed record ResetCorrupt(string BackupFile) : LoadReport;
    /// <summary>From a newer version of Current: used read-only, and signing in fails until it is updated.</summary>
    public sealed record NewerVersion(ulong Found) : LoadReport;
}

/// <summary>Where the core keeps sign-in tokens.</summary>
public enum SecretStore
{
    /// <summary>Windows Credential Manager. What the app uses.</summary>
    Keyring,
    /// <summary>Memory only, for tests: tokens are lost when the process ends.</summary>
    Memory,
}

/// <summary>
/// The platform's report of the network (ADR 0014). <c>NetworkKey</c> is opaque and goes into the core only; <c>ToString()</c>
/// shows it as <c>[redacted]</c>, since it can name the Wi-Fi network.
/// </summary>
public sealed record NetworkFacts(
    Connectivity Connectivity,
    bool? Metered,
    bool? Constrained,
    InterfaceKind Interface,
    bool? Vpn,
    string? NetworkKey)
{
    private bool PrintMembers(System.Text.StringBuilder builder)
    {
        builder.Append($"Connectivity = {Connectivity}, Metered = {Metered}, Constrained = {Constrained}, Interface = {Interface}, ");
        builder.Append($"Vpn = {Vpn}, NetworkKey = {(NetworkKey is null ? "null" : "[redacted]")}");
        return true;
    }
}

/// <summary>
/// One of the device's up IPv4 interfaces, as the app lists it just before a search (ADR 0016): its address and the prefix length
/// of its subnet (24 for a netmask of 255.255.255.0). It is the device's own address, so <c>ToString()</c> shows it as
/// <c>[redacted]</c>.
/// </summary>
public sealed record LocalInterface(System.Net.IPAddress Address, int PrefixLength)
{
    private bool PrintMembers(System.Text.StringBuilder builder)
    {
        builder.Append($"Address = [redacted], PrefixLength = {PrefixLength}");
        return true;
    }
}

public enum Connectivity
{
    /// <summary>No network at all.</summary>
    Offline,
    LocalOnly,
    CaptivePortal,
    Full,
    Unknown,
}

public enum InterfaceKind
{
    Wifi,
    Ethernet,
    Cellular,
    Other,
    Unknown,
}

/// <summary>How a sign-out went. The app is signed out either way; <c>ServerTold</c> false means the server did not hear of it.</summary>
public sealed record SignOutResult(bool ServerTold);

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
        status.AddressPosition,
        status.User is { } user ? From(user) : null,
        status.Version,
        status.Features);

    public static UserInfo From(Ffi.UserSnapshot user) => new(user.Id, user.Name, user.Permissions);

    public static ServerRef From(Ffi.ServerRef server) => new(server.Service, server.Key);

    public static Ffi.ServerRef ToFfi(ServerRef server) => new(server.Service, server.Key);

    public static SavedServer From(Ffi.SavedServerSnapshot saved) => new(
        From(saved.Server),
        saved.Id,
        saved.Name,
        (int)Math.Min(saved.AddressCount, int.MaxValue),
        Array.ConvertAll(saved.Users, user => new SavedUser(user.Id, user.Name)),
        saved.ActiveUser);

    public static LoadReport From(Ffi.LoadReport report) => report switch
    {
        Ffi.LoadReport.Created => new LoadReport.Created(),
        Ffi.LoadReport.Loaded => new LoadReport.Loaded(),
        Ffi.LoadReport.Migrated migrated => new LoadReport.Migrated(migrated.From),
        Ffi.LoadReport.ResetCorrupt reset => new LoadReport.ResetCorrupt(reset.BackupFile),
        Ffi.LoadReport.NewerVersion newer => new LoadReport.NewerVersion(newer.Found),
        _ => throw new ArgumentException($"unknown load report {report.GetType().Name}", nameof(report)),
    };

    /// <summary>Both values explicitly: an unknown one must never fall back to memory and lose the user's sign-ins.</summary>
    public static Ffi.SecretStore ToFfi(SecretStore store) => store switch
    {
        SecretStore.Keyring => Ffi.SecretStore.Keyring,
        SecretStore.Memory => Ffi.SecretStore.Memory,
        _ => throw new ArgumentOutOfRangeException(nameof(store), store, "unknown secret store"),
    };

    /// <summary>Only IPv4 addresses with a prefix of 0 to 32: the core searches over IPv4 only.</summary>
    public static Ffi.LocalInterface[] ToFfi(IReadOnlyList<LocalInterface> interfaces) => interfaces
        .Where(i => i.Address.AddressFamily == System.Net.Sockets.AddressFamily.InterNetwork && i.PrefixLength is >= 0 and <= 32)
        .Select(i => new Ffi.LocalInterface(i.Address.ToString(), (byte)i.PrefixLength))
        .ToArray();

    public static Ffi.NetworkFacts ToFfi(NetworkFacts facts) => new(
        facts.Connectivity switch
        {
            Connectivity.Offline => Ffi.Connectivity.Offline,
            Connectivity.LocalOnly => Ffi.Connectivity.LocalOnly,
            Connectivity.CaptivePortal => Ffi.Connectivity.CaptivePortal,
            Connectivity.Full => Ffi.Connectivity.Full,
            _ => Ffi.Connectivity.Unknown,
        },
        facts.Metered,
        facts.Constrained,
        facts.Interface switch
        {
            InterfaceKind.Wifi => Ffi.InterfaceKind.Wifi,
            InterfaceKind.Ethernet => Ffi.InterfaceKind.Ethernet,
            InterfaceKind.Cellular => Ffi.InterfaceKind.Cellular,
            InterfaceKind.Other => Ffi.InterfaceKind.Other,
            _ => Ffi.InterfaceKind.Unknown,
        },
        facts.Vpn,
        facts.NetworkKey);

    public static ListDiff<SavedServer> From(Ffi.SavedServerDiff diff) => diff switch
    {
        Ffi.SavedServerDiff.Append append => new ListDiff<SavedServer>.Append(Array.ConvertAll(append.Values, value => From(value))),
        Ffi.SavedServerDiff.Insert insert => new ListDiff<SavedServer>.Insert(Index(insert.Index), From(insert.Value)),
        Ffi.SavedServerDiff.Set set => new ListDiff<SavedServer>.Set(Index(set.Index), From(set.Value)),
        Ffi.SavedServerDiff.Remove remove => new ListDiff<SavedServer>.Remove(Index(remove.Index)),
        Ffi.SavedServerDiff.Reset reset => new ListDiff<SavedServer>.Reset(Array.ConvertAll(reset.Values, value => From(value))),
        _ => throw new ArgumentException($"unknown diff {diff.GetType().Name}", nameof(diff)),
    };

    /// <summary>The core saturates indices at <c>uint.MaxValue</c>; one that does not fit is out of range for any list.</summary>
    private static int Index(uint index) => (int)Math.Min(index, int.MaxValue);

    public static Reachability From(Ffi.Reachability reachability) => reachability switch
    {
        Ffi.Reachability.Reachable => Reachability.Reachable,
        Ffi.Reachability.Unreachable => Reachability.Unreachable,
        _ => Reachability.Unknown,
    };
}
