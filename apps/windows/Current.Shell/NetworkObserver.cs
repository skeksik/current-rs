using System.Diagnostics;
using Current.Bindings;
using Microsoft.UI.Dispatching;
using Windows.Networking.Connectivity;

namespace Current.Shell;

/// <summary>
/// Reports the network to the core (ADR 0014): once at start and again on every <c>NetworkInformation.NetworkStatusChanged</c>
/// ("Occurs when the network status changes for a connection", learn.microsoft.com, read 2026-10-23). The page does not say which
/// thread raises it, so every report hops to the UI thread, which also keeps the calls on one thread as <c>NetworkChanged</c> asks.
/// Only facts that need no permission: no Wi-Fi name, which needs location access. The network key goes into the core only and is
/// never logged.
/// </summary>
internal sealed class NetworkObserver(CoreHost core, DispatcherQueue ui) : IDisposable
{
    private bool _started;
    private bool _disposed;

    public void Start()
    {
        if (_started || _disposed)
        {
            return;
        }
        _started = true;
        NetworkInformation.NetworkStatusChanged += OnNetworkStatusChanged;
        Report();
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }
        _disposed = true;
        if (_started)
        {
            NetworkInformation.NetworkStatusChanged -= OnNetworkStatusChanged;
        }
    }

    private void OnNetworkStatusChanged(object sender) => ui.TryEnqueue(Report);

    private void Report()
    {
        if (_disposed)
        {
            return;
        }
        try
        {
            core.NetworkChanged(Read());
        }
        catch (Exception error)
        {
            // The type only: the message could carry the facts.
            Trace.TraceError($"Current: reporting the network failed: {error.GetType().Name}");
        }
    }

    /// <summary>The facts now. Never throws: a failed read is reported as unknown.</summary>
    public static NetworkFacts Read()
    {
        try
        {
            // "Re-query the profile when you receive a network status change event because cached profile objects may be obsolete"
            // (ConnectionProfile, learn.microsoft.com, read 2026-10-23).
            var profile = NetworkInformation.GetInternetConnectionProfile() ?? BestLocalProfile();
            if (profile is null)
            {
                return new NetworkFacts(Connectivity.Offline, null, null, InterfaceKind.Unknown, null, null);
            }
            var adapter = profile.NetworkAdapter;
            return new NetworkFacts(
                ConnectivityOf(profile.GetNetworkConnectivityLevel()),
                Metered(profile.GetConnectionCost()),
                // Windows has no "constrained" (low data mode) fact (ADR 0014).
                null,
                adapter is null ? InterfaceKind.Unknown : InterfaceOf(adapter.IanaInterfaceType),
                // ADR 0014: telling a VPN apart on Windows is not verified, so it stays unknown.
                null,
                adapter is null ? null : KeyOf(adapter));
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: reading the network failed: {error.GetType().Name}");
            return new NetworkFacts(Connectivity.Unknown, null, null, InterfaceKind.Unknown, null, null);
        }
    }

    /// <summary>
    /// Without internet access Windows may have no internet profile while a local network still works, and a Jellyfin server on it is
    /// reachable: "Apps should attempt to connect to their services whenever LocalAccess or higher connectivity is reported"
    /// (NetworkConnectivityLevel, learn.microsoft.com, read 2026-10-23). So take the connected profile with the most access.
    /// </summary>
    private static ConnectionProfile? BestLocalProfile() =>
        NetworkInformation.GetConnectionProfiles()
            .Where(profile => profile.GetNetworkConnectivityLevel() != NetworkConnectivityLevel.None)
            .OrderByDescending(profile => profile.GetNetworkConnectivityLevel())
            .FirstOrDefault();

    private static Connectivity ConnectivityOf(NetworkConnectivityLevel level) => level switch
    {
        NetworkConnectivityLevel.None => Connectivity.Offline,
        NetworkConnectivityLevel.LocalAccess => Connectivity.LocalOnly,
        // "possibly due to a captive portal" (NetworkConnectivityLevel, same page).
        NetworkConnectivityLevel.ConstrainedInternetAccess => Connectivity.CaptivePortal,
        NetworkConnectivityLevel.InternetAccess => Connectivity.Full,
        _ => Connectivity.Unknown,
    };

    /// <summary>The rule in the ConnectionProfile page's own example (learn.microsoft.com, read 2026-10-23).</summary>
    private static bool? Metered(ConnectionCost cost) => cost.NetworkCostType == NetworkCostType.Unknown
        ? null
        : cost.NetworkCostType != NetworkCostType.Unrestricted || cost.Roaming || cost.OverDataLimit;

    /// <summary>IANA interface types: "6: Ethernet", "71: IEEE 802.11 wireless LAN", "243: WWAN" (NetworkAdapter, same date); 244 is
    /// the other WWAN type in the IANA registry.</summary>
    private static InterfaceKind InterfaceOf(uint ianaType) => ianaType switch
    {
        6 => InterfaceKind.Ethernet,
        71 => InterfaceKind.Wifi,
        243 or 244 => InterfaceKind.Cellular,
        _ => InterfaceKind.Other,
    };

    /// <summary>
    /// The connected network's id and the adapter's: stable on one network, different between networks, and the same network seen
    /// through Wi-Fi and through Ethernet stays apart. Opaque to the core; never shown or logged.
    /// </summary>
    private static string KeyOf(NetworkAdapter adapter)
    {
        var network = adapter.NetworkItem?.NetworkId;
        return $"{network?.ToString("D") ?? "none"}/{adapter.NetworkAdapterId:D}";
    }
}
