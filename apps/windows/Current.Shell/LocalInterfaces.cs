using System.Diagnostics;
using System.Net.NetworkInformation;
using System.Net.Sockets;
using Current.Bindings;

namespace Current.Shell;

/// <summary>
/// The up IPv4 interfaces, read just before each local search (ADR 0016) so that the core sends from each of them. Sources
/// (learn.microsoft.com, .NET API reference, ms.date 2025-07-01, read 2026-10-23):
/// <list type="bullet">
/// <item><c>NetworkInterface.GetAllNetworkInterfaces</c> "Returns objects that describe the network interfaces on the local computer",
/// "or an empty array if no interfaces are detected", and throws <c>NetworkInformationException</c> when "A Windows system function
/// call failed".</item>
/// <item><c>OperationalStatus.Up</c>: "The network interface is up; it can transmit data packets."</item>
/// <item><c>NetworkInterfaceType.Loopback</c>: "no traffic is sent over the wire"; <c>Tunnel</c>: "The network interface uses a
/// tunnel connection." Both are skipped: neither reaches a LAN by broadcast.</item>
/// <item><c>UnicastIPAddressInformation.PrefixLength</c>: "The length, in bits, of the prefix or network part of the IP
/// address."</item>
/// </list>
/// The addresses go into the core only and are never logged.
/// </summary>
internal static class LocalInterfaces
{
    /// <summary>The list, or as much of it as could be read; empty if none (the core then searches from an unbound socket only).</summary>
    public static IReadOnlyList<LocalInterface> Read()
    {
        var found = new List<LocalInterface>();
        NetworkInterface[] adapters;
        try
        {
            adapters = NetworkInterface.GetAllNetworkInterfaces();
        }
        catch (NetworkInformationException error)
        {
            Trace.TraceWarning($"Current: listing network interfaces failed: {error.GetType().Name}");
            return found;
        }
        foreach (var adapter in adapters)
        {
            if (adapter.OperationalStatus != OperationalStatus.Up
                || adapter.NetworkInterfaceType is NetworkInterfaceType.Loopback or NetworkInterfaceType.Tunnel)
            {
                continue;
            }
            try
            {
                foreach (var unicast in adapter.GetIPProperties().UnicastAddresses)
                {
                    if (unicast.Address.AddressFamily == AddressFamily.InterNetwork)
                    {
                        found.Add(new LocalInterface(unicast.Address, unicast.PrefixLength));
                    }
                }
            }
            catch (NetworkInformationException error)
            {
                // One adapter that cannot be read must not hide the others.
                Trace.TraceWarning($"Current: reading a network interface failed: {error.GetType().Name}");
            }
        }
        return found;
    }
}
