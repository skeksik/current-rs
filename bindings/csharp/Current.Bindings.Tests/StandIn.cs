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

    // Its fake libraries (tools/jellyfin-stand-in/src/library.rs and paging.rs).
    public const string FilmsLibraryId = "57a2d1e0000000000000000000001001";
    public const string FilmsLibraryName = "Example Films";
    public static readonly string[] FilmIds =
    [
        "57a2d1e0000000000000000000002001",
        "57a2d1e0000000000000000000002002",
        "57a2d1e0000000000000000000002003",
    ];
    public const string FirstFilmName = "Example Film One";
    /// <summary>The first film's primary image tag.</summary>
    public const string FirstFilmPosterTag = "57a2d1e0a0000001";
    /// <summary>What every image request gets: a PNG.</summary>
    public static readonly byte[] PngSignature = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    public const string BigLibraryId = "57a2d1e0000000000000000000006000";
    public const int BigLibrarySize = 450;

    /// <summary>The generated library's item name at <paramref name="index"/>.</summary>
    public static string BigItemName(int index) => $"Example Item {index:D4}";

    /// <summary>What <c>fail</c>, <c>hold</c> and <see cref="Cancelled"/> take for the generated library's page at <paramref name="start"/>.</summary>
    public static string PageKey(int start) => $"/Items@{start}";

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
    /// <c>gate off</c>, <c>basic</c>, <c>portal</c> or <c>header</c>; <c>gate-cookie &lt;value&gt;</c> (the portal takes this value now);
    /// <c>fail &lt;path or page&gt;</c>, <c>unfail &lt;path or page&gt;</c> (answers 500 until unfailed; a page of the generated library is
    /// <c>/Items@&lt;start&gt;</c>).
    /// </summary>
    public void Send(string command)
    {
        _process.StandardInput.WriteLine(command);
        _process.StandardInput.Flush();
    }

    /// <summary>How many requests to <paramref name="path"/> the stand-in received so far.</summary>
    public int Count(string path) => Ask("count", path);

    /// <summary>How many held requests to <paramref name="path"/> their client gave up (closed the connection) before a release.</summary>
    public int Cancelled(string path) => Ask("cancelled", path);

    private int Ask(string command, string path)
    {
        Send($"{command} {path}");
        var read = _process.StandardOutput.ReadLineAsync();
        var prefix = command + " ";
        if (!read.Wait(TimeSpan.FromSeconds(10)) || read.Result is not { } line || !line.StartsWith(prefix, StringComparison.Ordinal))
        {
            throw new Exception($"the stand-in did not answer {command}");
        }
        return int.Parse(line[prefix.Length..], System.Globalization.CultureInfo.InvariantCulture);
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
