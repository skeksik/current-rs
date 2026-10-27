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
        using (var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "Example PC", SecretStore.Memory))
        {
            Check(core.LoadReport is LoadReport.Created, "created");
            Check(core.Servers.Count == 0, "no servers");
            var report = await core.ExportDiagnosticsAsync();
            Check(report.StartsWith($"Current {core.AppInfo.Version}\nPlatform: windows x86_64\nSettings: created\n", StringComparison.Ordinal), report);
            Check(!report.Contains(folder.Path, StringComparison.OrdinalIgnoreCase), "no path in the report");
            Check(!report.Contains("Example PC", StringComparison.Ordinal), "no device name in the report");
        }
        using (var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory))
        {
            Check(core.LoadReport is LoadReport.Loaded, "loaded");
        }
        using (var memory = CoreHost.Create())
        {
            Check(memory.LoadReport is null, "nothing stored");
        }
    }

    public static async Task AppThemeIsSavedAndReadAtTheNextOpen()
    {
        using var folder = new DataFolder();
        var ui = new ManualDispatcher();
        using (var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory))
        {
            Check(core.AppTheme == AppTheme.System, "System until one is chosen");
            using var theme = core.ObserveAppTheme(ui);
            Check(theme.Value == AppTheme.System, "starts at the current theme");
            await core.SetAppThemeAsync(AppTheme.Dark);
            Check(core.AppTheme == AppTheme.Dark, "read back at once");
            await Eventually("the observer sees Dark", ui, () => theme.Value == AppTheme.Dark);
        }
        using (var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory))
        {
            Check(core.AppTheme == AppTheme.Dark, "kept for the next open");
        }
    }

    public static async Task SearchChoicesAreRememberedWhileTheirSwitchIsOn()
    {
        using var folder = new DataFolder();
        var chosen = new SearchChoices(new SearchFilter("series", SearchSource.Library), new LibraryView(LibraryView.Wide, ShowTitles: false));
        using (var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory))
        {
            await core.RememberSearchChoicesAsync(chosen);
            Check(core.SearchChoices == new SearchChoices(), "nothing remembered while off");
            await core.SetRememberFilterAsync(true);
            await core.RememberSearchChoicesAsync(chosen);
            Check(core.SearchChoices == new SearchChoices(chosen.Filter), "the filter only, while only its switch is on");
            await core.SetRememberViewAsync(true);
            await core.RememberSearchChoicesAsync(new SearchChoices(View: chosen.View));
        }
        using (var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory))
        {
            Check(core.SearchChoices == chosen, $"kept for the next open, not {core.SearchChoices}");
        }
    }

    public static async Task LibraryChoicesAreRememberedWhileTheirSwitchIsOn()
    {
        using var folder = new DataFolder();
        var library = new ItemRef(new ServerRef("jellyfin", "1"), "abc");
        var everything = new LibraryChoices(
            new SortChoice("date_created", SortOrder.Descending), ["unplayed"], new LibraryView(LibraryView.Wide, ShowTitles: false));
        using (var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory))
        {
            Check(core.LibraryMemory == new LibraryMemorySettings(false, false, false), "all off by default");
            await core.RememberLibraryChoicesAsync(library, everything);
            Check(core.LibraryChoicesFor(library) == new LibraryChoices(), "nothing remembered while off");
            await core.SetRememberSortAsync(true);
            await core.SetRememberFilterAsync(true);
            await core.SetRememberViewAsync(true);
            await core.RememberLibraryChoicesAsync(library, everything);
            var choices = core.LibraryChoicesFor(library);
            Check(choices.Sort == everything.Sort && choices.View == everything.View, "sort and view given back");
            Check(choices.Filters is ["unplayed"], "filters given back");
            // "All" is an empty list; a part not given stays.
            await core.RememberLibraryChoicesAsync(library, new LibraryChoices(Filters: []));
            choices = core.LibraryChoicesFor(library);
            Check(choices.Filters is [] && choices.Sort == everything.Sort, "All, and the sort kept");
            await core.SetRememberViewAsync(false);
            Check(core.LibraryChoicesFor(library).View is null, "off forgets the view");
        }
        using (var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory))
        {
            Check(core.LibraryMemory == new LibraryMemorySettings(true, true, false), "the switches kept for the next open");
            Check(core.LibraryChoicesFor(library).Sort == everything.Sort, "the sort kept for the next open");
        }
    }

    public static async Task LibraryChoiceThatCannotBeSavedIsRefused()
    {
        using var folder = new DataFolder();
        File.WriteAllText(Path.Combine(folder.Path, "settings.json"), "{\"version\": 99, \"device_id\": \"abc\"}");
        using var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory);
        try
        {
            await core.SetRememberSortAsync(true);
            Check(false, "saving fails");
        }
        catch (CoreStoreException error)
        {
            Check(error.Problem is StoreProblem.NewerVersion { Found: 99 }, $"a newer version, not {error.Problem}");
        }
        Check(!core.LibraryMemory.RememberSort, "unchanged");
    }

    public static async Task AppThemeThatCannotBeSavedIsRefused()
    {
        using var folder = new DataFolder();
        File.WriteAllText(Path.Combine(folder.Path, "settings.json"), "{\"version\": 99, \"device_id\": \"abc\"}");
        using var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory);
        try
        {
            await core.SetAppThemeAsync(AppTheme.Light);
            Check(false, "saving fails");
        }
        catch (CoreStoreException error)
        {
            Check(error.Problem is StoreProblem.NewerVersion { Found: 99 }, $"a newer version, not {error.Problem}");
        }
        Check(core.AppTheme == AppTheme.System, "unchanged");
    }

    public static async Task OpenReportsAFolderItCannotUse()
    {
        using var folder = new DataFolder();
        var file = Path.Combine(folder.Path, "file");
        File.WriteAllText(file, "x");
        try
        {
            using var core = await CoreHost.OpenAsync(file, Path.Combine(file, "Cache"), "", SecretStore.Memory);
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
        using var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "Example PC", SecretStore.Memory);
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

        // Nothing that crossed the bridge holds the address or the password, as the UI or a log would print it.
        var port = standIn.Address[(standIn.Address.LastIndexOf(':') + 1)..];
        var seen = string.Join("\n", addState.Value, signInState.Value, saved, status, add.State, flow.State,
            string.Join(",", saved.Users));
        foreach (var secret in new[] { "127.0.0.1", port, StandIn.Password })
        {
            Check(!seen.Contains(secret, StringComparison.Ordinal), $"{secret} in {seen}");
        }
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
        using var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory);
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

    public static async Task RemovingAServerForgetsIt()
    {
        using var standIn = StandIn.Start();
        using var folder = new DataFolder();
        using (var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory))
        {
            await SignIn(core, standIn);
            var result = await core.RemoveServerAsync(FirstServer);
            Check(result == new RemoveServerResult(true, false), $"{result}");
            Check(core.Servers.Count == 0, "no saved server");
            var ui = new ManualDispatcher();
            using var capabilities = core.ObserveCapabilities(ui);
            await Eventually("no status", ui, () => capabilities.Value.Servers.Count == 0);
            await Eventually("the server is told", ui, () => standIn.Count("/Sessions/Logout") == 1);
            Check(!(await core.RemoveServerAsync(FirstServer)).Removed, "a second removal does nothing");
        }
        using (var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory))
        {
            Check(core.Servers.Count == 0, "the next start does not bring it back");
        }
    }

    public static async Task WithoutTheTokenRestoreSignsOut()
    {
        using var standIn = StandIn.Start();
        using var folder = new DataFolder();
        using (var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory))
        {
            await SignIn(core, standIn);
        }
        // Memory secrets die with their core: the next one finds the saved user but no token.
        using var next = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory);
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
        using var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Keyring);
        try
        {
            using (var add = core.AddServer())
            {
                await Ready(add, standIn);
                using var flow = add.SignIn();
                await flow.WithPasswordAsync(StandIn.UserName, StandIn.Password);
                Check(flow.State is SignInState.SignedIn { Remembered: true }, $"remembered in the keyring, not {flow.State}");
            }
            using var next = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Keyring);
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
        // Loopback is dropped by the core and IPv6 by the adapter, so only the unbound socket sends.
        var interfaces = new[]
        {
            new LocalInterface(System.Net.IPAddress.Loopback, 8),
            new LocalInterface(System.Net.IPAddress.IPv6Loopback, 128),
        };
        Check(!interfaces[0].ToString().Contains("127.0.0.1"), "ToString hides the address");
        await discovery.SearchAsync(interfaces);
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

    public static async Task DiagnosticsAfterASignInHoldNothingPrivate()
    {
        using var standIn = StandIn.Start();
        using var folder = new DataFolder();
        using var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "Example PC", SecretStore.Memory);
        await SignIn(core, standIn);
        var report = await core.ExportDiagnosticsAsync();
        Check(report.Contains("/Users/AuthenticateByName", StringComparison.Ordinal), report);
        var port = standIn.Address[(standIn.Address.LastIndexOf(':') + 1)..];
        // The device name goes to the server in every request's header; it must not reach the report.
        foreach (var secret in new[] { "Example PC", StandIn.Password, StandIn.UserName, "127.0.0.1", port, folder.Path })
        {
            Check(!report.Contains(secret, StringComparison.OrdinalIgnoreCase), $"{secret} in the report");
        }
    }

    public static async Task CancellingWhileSigningInReturnsToReady()
    {
        const string signInPath = "/Users/AuthenticateByName";
        using var standIn = StandIn.Start();
        using var core = CoreHost.Create();
        using var add = core.AddServer();
        await Ready(add, standIn);
        using var flow = add.SignIn();
        standIn.Send($"hold {signInPath}");
        using var cts = new CancellationTokenSource();
        var call = flow.WithPasswordAsync(StandIn.UserName, StandIn.Password, cts.Token);
        var ui = new ManualDispatcher();
        await Eventually("the request waits at the server", ui, () => standIn.Count(signInPath) == 1 && flow.State is SignInState.SigningIn);
        cts.Cancel();
        try
        {
            await call;
            Check(false, "cancelled");
        }
        catch (OperationCanceledException)
        {
        }
        Check(flow.State is SignInState.Ready, $"{flow.State}");

        // The server answers after all: too late, nothing is saved.
        standIn.Send($"release {signInPath}");
        await Task.Delay(300);
        Check(flow.State is SignInState.Ready, $"still ready, not {flow.State}");
        Check(core.Servers.Count == 0, "nothing saved");
    }

    public static async Task CancellingWhileQuickConnectWaitsStopsPolling()
    {
        const string connectPath = "/QuickConnect/Connect";
        using var standIn = StandIn.Start();
        standIn.Send("quick-connect hold");
        using var core = CoreHost.Create();
        using var add = core.AddServer();
        await Ready(add, standIn);
        using var flow = add.SignIn();
        using var cts = new CancellationTokenSource();
        var call = flow.WithQuickConnectAsync(cts.Token);
        var ui = new ManualDispatcher();
        await Eventually("waiting for approval", ui, () =>
            flow.State is SignInState.QuickConnectWaiting { Code: StandIn.QuickConnectCode } && standIn.Count(connectPath) >= 1);
        cts.Cancel();
        try
        {
            await call;
            Check(false, "cancelled");
        }
        catch (OperationCanceledException)
        {
        }
        Check(flow.State is SignInState.Ready, $"{flow.State}");
        // The core asks every 5 s (Jellyfin's interval): a poll would show within 6 s.
        var polls = standIn.Count(connectPath);
        await Task.Delay(TimeSpan.FromSeconds(6));
        Check(standIn.Count(connectPath) == polls, "no polling after the cancel");
    }

    public static async Task ServerListRecoversFromABadBatch()
    {
        using var core = CoreHost.Create();
        var ui = new ManualDispatcher();
        using var servers = core.ObserveServers(ui);
        await Eventually("the first Reset", ui, () => servers.Generation == 1 && ui.Pending == 0);
        // A batch the mirror cannot apply: it subscribes again and gets a fresh Reset.
        servers.Apply([new ListDiff<SavedServer>.Remove(5)], servers.Generation);
        Check(servers.Recoveries == 1 && servers.Generation == 2, "recovering");
        await Eventually("the core holds one listener", ui, () => core.LiveListeners == 1);
        // A batch still queued from the old subscription is dropped.
        servers.Apply([new ListDiff<SavedServer>.Remove(9)], 1);
        Check(servers.Recoveries == 1, "the old subscription's batch is ignored");
    }

    public static async Task ServerListRecoversFromALostBatch()
    {
        using var standIn = StandIn.Start();
        using var folder = new DataFolder();
        using var core = await CoreHost.OpenAsync(folder.Path, Path.Combine(folder.Path, "Cache"), "", SecretStore.Memory);
        var ui = new RefusingDispatcher();
        using var servers = core.ObserveServers(ui);
        await Eventually("the first Reset", ui.Queue, () => ui.Accepted >= 1);
        // The batch with the new server is refused (the queue was shutting down): the mirror must not miss it.
        ui.RefuseNext = true;
        await SignIn(core, standIn);
        await Eventually("the mirror recovers with the server", ui.Queue, () =>
            servers.Items.Count == 1 && servers.Items[0].ActiveUser == StandIn.UserId);
        Check(servers.Recoveries >= 1, "it recovered");
    }

    /// <summary>A dispatcher that refuses one enqueue on request, then works again.</summary>
    private sealed class RefusingDispatcher : IUiDispatcher
    {
        public ManualDispatcher Queue { get; } = new();
        public volatile bool RefuseNext;
        public int Accepted;

        public bool TryEnqueue(Action work)
        {
            if (RefuseNext)
            {
                RefuseNext = false;
                return false;
            }
            Interlocked.Increment(ref Accepted);
            return Queue.TryEnqueue(work);
        }
    }

    public static Task NetworkFactsHideTheKey()
    {
        var facts = new NetworkFacts(Connectivity.Full, null, null, InterfaceKind.Wifi, null, "home-wifi-192.168.1.10");
        var text = facts.ToString();
        Check(text.Contains("[redacted]", StringComparison.Ordinal) && !text.Contains("home-wifi", StringComparison.Ordinal), text);
        return Task.CompletedTask;
    }

    public static async Task ServerListStopsAfterRepeatedFailures()
    {
        using var core = CoreHost.Create();
        var ui = new ManualDispatcher();
        using var servers = core.ObserveServers(ui);
        var reported = new List<string?>();
        servers.PropertyChanged += (_, e) => reported.Add(e.PropertyName);
        // One bad batch more than the mirror recovers from within the window.
        for (var i = 0; i <= ListSubscription<SavedServer>.MaxRecoveries; i++)
        {
            await Eventually("the Reset of the current subscription", ui, () => ui.Pending == 0);
            servers.Apply([new ListDiff<SavedServer>.Remove(5)], servers.Generation);
        }
        Check(servers.IsFaulted, "faulted");
        Check(reported.SequenceEqual([nameof(ListSubscription<SavedServer>.IsFaulted)]), $"reported once: {string.Join(", ", reported)}");
        Check(servers.Recoveries == ListSubscription<SavedServer>.MaxRecoveries, $"{servers.Recoveries} recoveries");
        await Eventually("the core drops the listener", ui, () => core.LiveListeners == 0);
    }
}
