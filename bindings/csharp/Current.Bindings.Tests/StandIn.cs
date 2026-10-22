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

    /// <summary>Sends a command: <c>revoke</c>, <c>quick-connect off</c> or <c>quick-connect on</c>.</summary>
    public void Send(string command)
    {
        _process.StandardInput.WriteLine(command);
        _process.StandardInput.Flush();
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
