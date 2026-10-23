using System.Runtime.InteropServices;

namespace Current.Shell;

/// <summary>
/// What Jellyfin shows for this device (ADR 0015): the computer's DNS host name, as Jellyfin's own desktop client sends it. That client
/// uses Qt's <c>QHostInfo::localHostName()</c>, which on Windows is <c>GetComputerNameEx(ComputerNameDnsHostname, …)</c>
/// (qtbase <c>src/corelib/global/qsysinfo.cpp</c>, read 2026-10-23). <c>Environment.MachineName</c> is the NetBIOS name, which "may be a
/// truncated version of the DNS host name" (COMPUTER_NAME_FORMAT, learn.microsoft.com, read 2026-10-23), so it is not used.
/// The name is never logged.
/// </summary>
internal static class DeviceName
{
    private const int ComputerNameDnsHostname = 1;
    private const int ErrorMoreData = 234;

    /// <summary>The host name, or "" if Windows does not give one: the core then uses the platform's name.</summary>
    public static string Get()
    {
        // "On output, receives the number of TCHARs copied to the destination buffer, not including the terminating null character. If
        // the buffer is too small, the function fails and GetLastError returns ERROR_MORE_DATA. This parameter receives the size of the
        // buffer required, including the terminating null character." (GetComputerNameExW, learn.microsoft.com, read 2026-10-23)
        var size = 256u;
        for (var attempt = 0; attempt < 2; attempt++)
        {
            var buffer = new char[size];
            if (GetComputerNameExW(ComputerNameDnsHostname, buffer, ref size))
            {
                return new string(buffer, 0, (int)Math.Min(size, (uint)buffer.Length));
            }
            if (Marshal.GetLastWin32Error() != ErrorMoreData)
            {
                break;
            }
        }
        return "";
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool GetComputerNameExW(int nameType, [Out] char[] buffer, ref uint size);
}
