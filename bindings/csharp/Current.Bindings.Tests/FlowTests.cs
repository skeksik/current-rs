using Current.Bindings;

/// <summary>
/// The M1 flows through the public adapters, against the Jellyfin stand-in: opening, adding a server, signing in, the saved-server
/// list, restoring, signing out, and the OS keyring once. Each test has its own stand-in and data folder.
/// </summary>
internal static class FlowTests
{
    private static readonly TimeSpan Wait = TimeSpan.FromSeconds(10);

    private static readonly ServerRef FirstServer = new("jellyfin", "1");

    private static void Check(bool condition, string what)
    {
        if (!condition)
        {
            throw new Exception($"expected: {what}");
        }
    }

    /// <summary>Runs the UI queue until <paramref name="done"/> holds.</summary>
    private static async Task Eventually(string what, ManualDispatcher ui, Func<bool> done)
    {
        var start = DateTime.UtcNow;
        while (true)
        {
            ui.Drain();
            if (done())
            {
                return;
            }
            Check(DateTime.UtcNow - start < Wait, what);
            await Task.Delay(10);
        }
    }

    /// <summary>A fresh data folder, deleted when disposed.</summary>
    private sealed class DataFolder : IDisposable
    {
        public string Path { get; } = Directory.CreateTempSubdirectory("current-tests-").FullName;

        public void Dispose()
        {
            try
            {
                Directory.Delete(Path, recursive: true);
            }
            catch (Exception)
            {
                // A file still open: the system cleans its temp folder later.
            }
        }
    }

    private static async Task<NewServerInfo> Ready(AddServerFlow add, StandIn standIn)
    {
        await add.ProbeAsync(standIn.Address);
        return add.State is AddServerState.Ready ready ? ready.Server : throw new Exception($"not ready: {add.State}");
    }

    private static async Task SignIn(CoreHost core, StandIn standIn)
    {
        using var add = core.AddServer();
        await Ready(add, standIn);
        using var flow = add.SignIn();
        await flow.WithPasswordAsync(StandIn.UserName, StandIn.Password);
        Check(flow.State is SignInState.SignedIn, $"signed in, not {flow.State}");
    }

    private static async Task<ServerStatus> Status(CoreHost core, Func<ServerStatus, bool> done)
    {
        var ui = new ManualDispatcher();
        using var capabilities = core.ObserveCapabilities(ui);
        await Eventually("the status", ui, () => capabilities.Value.Servers.Count == 1 && done(capabilities.Value.Servers[0]));
        return capabilities.Value.Servers[0];
    }

    public static async Task OpenCreatesThenLoads()
    {
        using var folder = new DataFolder();
        using (var core = await CoreHost.OpenAsync(folder.Path, "Example PC", SecretStore.Memory))
        {
            Check(core.LoadReport is LoadReport.Created, "created");
            Check(core.Servers.Count == 0, "no servers");
            var report = await core.ExportDiagnosticsAsync();
            Check(report.StartsWith($"Current {core.AppInfo.Version}\nPlatform: windows x86_64\nSettings: created\n", StringComparison.Ordinal), report);
            Check(!report.Contains(folder.Path, StringComparison.OrdinalIgnoreCase), "no path in the report");
            Check(!report.Contains("Example PC", StringComparison.Ordinal), "no device name in the report");
        }
        using (var core = await CoreHost.OpenAsync(folder.Path, "", SecretStore.Memory))
        {
            Check(core.LoadReport is LoadReport.Loaded, "loaded");
        }
        using (var memory = CoreHost.Create())
        {
            Check(memory.LoadReport is null, "nothing stored");
        }
    }

    public static async Task OpenReportsAFolderItCannotUse()
    {
        using var folder = new DataFolder();
        var file = Path.Combine(folder.Path, "file");
        File.WriteAllText(file, "x");
        try
        {
            using var core = await CoreHost.OpenAsync(file, "", SecretStore.Memory);
            Check(false, "opening fails");
        }
        catch (CoreStoreException error)
        {
            Check(error.Problem is StoreProblem.Io, $"an I/O problem, not {error.Problem}");
        }
    }

    public static async Task PasswordSignInEndToEnd()
    {
        using var standIn = StandIn.Start();
        using var folder = new DataFolder();
        using var core = await CoreHost.OpenAsync(folder.Path, "Example PC", SecretStore.Memory);
        var ui = new ManualDispatcher();
        using var servers = core.ObserveServers(ui);

        using var add = core.AddServer();
        using var addState = add.ObserveState(ui);
        Check(addState.Value is AddServerState.Idle, "idle first");
        try
        {
            add.SignIn().Dispose();
            Check(false, "signing in before Ready fails");
        }
        catch (FlowNotReadyException)
        {
        }
        var server = await Ready(add, standIn);
        Check(server == new NewServerInfo(StandIn.ServerId, StandIn.ServerName, "12.2.0", "Jellyfin Server", false), $"{server}");
        await Eventually("the page sees Ready", ui, () => addState.Value is AddServerState.Ready);

        using var flow = add.SignIn();
        using var signInState = flow.ObserveState(ui);
        await flow.CheckQuickConnectAsync();
        Check(flow.State == new SignInState.Ready(true), $"Quick Connect offered, not {flow.State}");
        await flow.WithPasswordAsync(StandIn.UserName, StandIn.Password);
        await Eventually("the page sees SignedIn", ui, () => signInState.Value is SignInState.SignedIn);
        var signedIn = (SignInState.SignedIn)signInState.Value;
        Check(signedIn.Server == FirstServer, $"{signedIn.Server}");
        Check(signedIn.User.Name == StandIn.UserName && signedIn.User.Id == StandIn.UserId, $"{signedIn.User}");
        Check(signedIn.Remembered, "remembered");

        await Eventually("the list shows the server", ui, () => servers.Items.Count == 1 && servers.Items[0].ActiveUser == StandIn.UserId);
        var saved = servers.Items[0];
        Check(saved.Server == FirstServer && saved.Name == StandIn.ServerName && saved.AddressCount == 1, $"{saved}");
        Check(saved.Users.Count == 1 && saved.Users[0] == new SavedUser(StandIn.UserId, StandIn.UserName), "the user");
        var status = await Status(core, status => status.SignedIn);
        Check(status.Reachability == Reachability.Reachable, "reachable");
        Check(status.AddressPosition is null or 1, "the first address or none");
    }

    public static async Task WrongPasswordFails()
    {
        using var standIn = StandIn.Start();
        using var core = CoreHost.Create();
        using var add = core.AddServer();
        await Ready(add, standIn);
        using var flow = add.SignIn();
        await flow.WithPasswordAsync(StandIn.UserName, "wrong");
        Check(flow.State is SignInState.Failed { Problem: SignInProblem.WrongCredentials }, $"{flow.State}");
        Check(core.Servers.Count == 0, "nothing saved");
    }

    public static async Task BadAddressFailsWithItsReason()
    {
        using var core = CoreHost.Create();
        using var add = core.AddServer();
        await add.ProbeAsync("ftp://media.example.com");
        Check(add.State is AddServerState.Failed { Problem: ProbeProblem.InvalidAddress { Reason: AddressProblem.UnsupportedScheme } },
            $"{add.State}");
        add.Reset();
        Check(add.State is AddServerState.Idle, "idle after reset");
    }

    public static async Task QuickConnectSignsIn()
    {
        using var standIn = StandIn.Start();
        using var core = CoreHost.Create();
        using var add = core.AddServer();
        await Ready(add, standIn);
        using var flow = add.SignIn();
        await flow.WithQuickConnectAsync();
        Check(flow.State is SignInState.SignedIn, $"{flow.State}");
    }

    public static async Task QuickConnectTurnedOffFails()
    {
        using var standIn = StandIn.Start();
        standIn.Send("quick-connect off");
        using var core = CoreHost.Create();
        using var add = core.AddServer();
        await Ready(add, standIn);
        using var flow = add.SignIn();
        await flow.WithQuickConnectAsync();
        Check(flow.State is SignInState.Failed { Problem: SignInProblem.QuickConnectDisabled }, $"{flow.State}");
    }

    public static async Task CancelledSignInReturnsToReady()
    {
        using var standIn = StandIn.Start();
        using var core = CoreHost.Create();
        using var add = core.AddServer();
        await Ready(add, standIn);
        using var flow = add.SignIn();
        using var cts = new CancellationTokenSource();
        cts.Cancel();
        try
        {
            await flow.WithQuickConnectAsync(cts.Token);
            Check(false, "cancelled");
        }
        catch (OperationCanceledException error)
        {
            Check(error.CancellationToken == cts.Token, "the caller's token");
        }
        Check(flow.State is SignInState.Ready, $"{flow.State}");
    }

    public static async Task SignOutClearsTheUser()
    {
        using var standIn = StandIn.Start();
        using var folder = new DataFolder();
        using var core = await CoreHost.OpenAsync(folder.Path, "", SecretStore.Memory);
        await SignIn(core, standIn);
        var result = await core.SignOutAsync(FirstServer);
        Check(result.ServerTold, "the server was told");
        Check(core.Servers[0].ActiveUser is null, "no active user");
        await Status(core, status => !status.SignedIn);
        Check(!(await core.SignOutAsync(FirstServer)).ServerTold, "a second sign-out tells no one");
        try
        {
            await core.SignOutAsync(new ServerRef("jellyfin", "9"));
            Check(false, "an unknown server fails");
        }
        catch (UnknownServerException)
        {
        }
    }

    public static async Task WithoutTheTokenRestoreSignsOut()
    {
        using var standIn = StandIn.Start();
        using var folder = new DataFolder();
        using (var core = await CoreHost.OpenAsync(folder.Path, "", SecretStore.Memory))
        {
            await SignIn(core, standIn);
        }
        // Memory secrets die with their core: the next one finds the saved user but no token.
        using var next = await CoreHost.OpenAsync(folder.Path, "", SecretStore.Memory);
        Check(next.Servers[0].ActiveUser == StandIn.UserId, "the saved user");
        await next.RestoreSessionsAsync();
        await Status(next, status => !status.SignedIn);
        var ui = new ManualDispatcher();
        using var servers = next.ObserveServers(ui);
        await Eventually("the active user is cleared", ui, () => servers.Items.Count == 1 && servers.Items[0].ActiveUser is null);

        using var again = next.SignInAgain(FirstServer);
        await again.WithPasswordAsync(StandIn.UserName, StandIn.Password);
        Check(again.State is SignInState.SignedIn, $"{again.State}");
    }

    /// <summary>
    /// The one test of the real keyring (Windows Credential Manager): a second core restores the session from it. The entry is
    /// deleted at the end even when the test fails, by signing out.
    /// </summary>
    public static async Task KeyringKeepsTheSession()
    {
        using var standIn = StandIn.Start();
        using var folder = new DataFolder();
        using var core = await CoreHost.OpenAsync(folder.Path, "", SecretStore.Keyring);
        try
        {
            using (var add = core.AddServer())
            {
                await Ready(add, standIn);
                using var flow = add.SignIn();
                await flow.WithPasswordAsync(StandIn.UserName, StandIn.Password);
                Check(flow.State is SignInState.SignedIn { Remembered: true }, $"remembered in the keyring, not {flow.State}");
            }
            using var next = await CoreHost.OpenAsync(folder.Path, "", SecretStore.Keyring);
            await next.RestoreSessionsAsync();
            var status = await Status(next, status => status.SignedIn && status.Reachability == Reachability.Reachable);
            Check(status.User?.Name == StandIn.UserName, "the user from the restored session");
        }
        finally
        {
            // Deletes the token from the keyring whatever happened above.
            if (core.Servers.Count > 0)
            {
                await core.SignOutAsync(FirstServer);
            }
        }
    }

    public static async Task DiscoveryEnds()
    {
        using var core = CoreHost.Create();
        using var discovery = core.DiscoverServers();
        await discovery.SearchAsync();
        // Whether the runner may broadcast, and whether anything answers, depends on the machine: only the shape is checked.
        Check(discovery.State is DiscoveryState.Done or DiscoveryState.Failed, $"{discovery.State}");
        using var add = core.AddServer();
        Check(!add.Pick(discovery, "not-listed"), "a server not listed cannot be picked");
        discovery.Reset();
        Check(discovery.State is DiscoveryState.Idle, "idle after reset");
    }

    public static async Task FlowSubscriptionsAreReleased()
    {
        using var core = CoreHost.Create();
        var ui = new ManualDispatcher();
        using var add = core.AddServer();
        var state = add.ObserveState(ui);
        var servers = core.ObserveServers(ui);
        Check(core.LiveListeners == 2, "two listeners");
        state.Dispose();
        servers.Dispose();
        await Eventually("the core drops both", ui, () => core.LiveListeners == 0);
    }

    public static Task NetworkReportsAreAccepted()
    {
        using var core = CoreHost.Create();
        core.NetworkChanged(new NetworkFacts(Connectivity.Full, false, null, InterfaceKind.Wifi, null, "home"));
        core.ReturnedToForeground();
        return Task.CompletedTask;
    }
}
