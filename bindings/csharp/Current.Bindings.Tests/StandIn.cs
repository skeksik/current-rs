using System.Diagnostics;

/// <summary>
/// The Jellyfin stand-in (tools/jellyfin-stand-in) as a child process. test.ps1 passes its path in CURRENT_STAND_IN. Disposing it
/// closes its stdin, which ends it; if this process dies first, the pipe closes all the same, so no stand-in outlives a run.
/// </summary>
internal sealed class StandIn : IDisposable
{
    // The stand-in's fake account (tools/jellyfin-stand-in/src/lib.rs).
    public const string UserName = "stand-in-user";
    public const string Password = "not-a-real-password";
    public const string UserId = "57a2d1e0000000000000000000000002";
    public const string ServerId = "57a2d1e0000000000000000000000001";
    public const string ServerName = "Stand-in Server";
    public const string QuickConnectCode = "123456";

    // Its gateway modes' fake credentials (`gate basic`, `gate portal`, `gate header`; tools/jellyfin-stand-in/src/lib.rs).
    public const string GateUser = "stand-in-gate-user";
    public const string GatePassword = "not-a-real-gate-password";
    public const string GateRealm = "Stand-in Gate";
    public const string GateCookie = "stand_in_gate";
    public const string GateCookieValue = "not-a-real-gate-cookie";
    public const string GateHeader = "x-stand-in-gate";
    public const string GateHeaderValue = "not-a-real-gate-header";

    private readonly Process _process;

    private StandIn(Process process, string address)
    {
        _process = process;
        Address = address;
    }

    public string Address { get; }

    public static StandIn Start()
    {
        var path = Environment.GetEnvironmentVariable("CURRENT_STAND_IN");
        if (string.IsNullOrEmpty(path) || !File.Exists(path))
        {
            throw new Exception($"CURRENT_STAND_IN must name the built jellyfin-stand-in (got '{path}'); test.ps1 sets it");
        }
        var process = Process.Start(new ProcessStartInfo(path)
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            UseShellExecute = false,
        }) ?? throw new Exception("the stand-in did not start");
        try
        {
            var read = process.StandardOutput.ReadLineAsync();
            if (!read.Wait(TimeSpan.FromSeconds(10)) || read.Result is not { } line || !line.StartsWith("listening ", StringComparison.Ordinal))
            {
                throw new Exception("the stand-in did not report its address");
            }
            return new StandIn(process, line["listening ".Length..]);
        }
        catch
        {
            Stop(process);
            throw;
        }
    }

    /// <summary>
    /// Sends a command: <c>revoke</c>; <c>quick-connect off</c>, <c>on</c> or <c>hold</c> (requests stay unapproved);
    /// <c>hold &lt;path&gt;</c>, <c>release &lt;path&gt;</c> (requests to the path wait, unanswered, until released);
    /// <c>gate off</c>, <c>basic</c>, <c>portal</c> or <c>header</c>; <c>gate-cookie &lt;value&gt;</c> (the portal takes this value now).
    /// </summary>
    public void Send(string command)
    {
        _process.StandardInput.WriteLine(command);
        _process.StandardInput.Flush();
    }

    /// <summary>How many requests to <paramref name="path"/> the stand-in received so far.</summary>
    public int Count(string path)
    {
        Send($"count {path}");
        var read = _process.StandardOutput.ReadLineAsync();
        if (!read.Wait(TimeSpan.FromSeconds(10)) || read.Result is not { } line || !line.StartsWith("count ", StringComparison.Ordinal))
        {
            throw new Exception("the stand-in did not answer the count");
        }
        return int.Parse(line["count ".Length..], System.Globalization.CultureInfo.InvariantCulture);
    }

    public void Dispose() => Stop(_process);

    private static void Stop(Process process)
    {
        try
        {
            process.StandardInput.Close();
            if (!process.WaitForExit(5000))
            {
                process.Kill();
            }
        }
        catch (Exception)
        {
            // Already gone.
        }
        process.Dispose();
    }
}
