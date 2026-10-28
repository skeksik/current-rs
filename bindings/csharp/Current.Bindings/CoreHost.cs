using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>
/// The core, created once per process by the app (ADR 0004). The only public way into the generated bindings: everything it returns
/// is a public type of this library.
/// </summary>
public sealed partial class CoreHost : IDisposable
{
    private readonly Ffi.Core _core;

    private CoreHost(Ffi.Core core)
    {
        _core = core;
    }

    /// <summary>A core that stores nothing: no saved servers, sign-ins last for the run. For tests and the smoke run.</summary>
    /// <exception cref="CoreRuntimeException">The core's runtime could not start.</exception>
    public static CoreHost Create() => new(CoreCall.Run(() => new Ffi.Core()));

    /// <summary>
    /// Opens the core on the app's data and cache folders (ADR 0013, ADR 0018). The file and keyring work runs on the core's own
    /// threads, so this may be awaited on the UI thread. Then call <see cref="RestoreSessionsAsync"/> in the background. The app passes
    /// <see cref="SecretStore.Keyring"/>; <see cref="SecretStore.Memory"/> is for tests.
    /// </summary>
    /// <param name="cacheDirectory">Where the core keeps what it can fetch again (images): the app passes %LOCALAPPDATA%\Current\Cache.
    /// A folder that cannot be used leaves images in memory only.</param>
    /// <param name="deviceName">What Jellyfin shows for this device: the computer's name. Empty becomes the platform's name.</param>
    /// <exception cref="CoreStoreException">The data folder cannot be read or written.</exception>
    public static async Task<CoreHost> OpenAsync(string dataDirectory, string cacheDirectory, string deviceName, SecretStore secrets) =>
        new(await CoreCall.RunUncancellableAsync(
            () => Ffi.CurrentFfiMethods.OpenCore(dataDirectory, cacheDirectory, deviceName, Map.ToFfi(secrets))).ConfigureAwait(false));

    /// <summary>The generated core, for the adapters in this library (the player's).</summary>
    internal Ffi.Core Core => _core;

    public AppInfo AppInfo => Map.From(_core.AppInfo());

    /// <summary>What the settings store found when the core opened. Null for <see cref="Create"/>.</summary>
    public LoadReport? LoadReport => _core.LoadReport() is { } report ? Map.From(report) : null;

    /// <summary>The request log as text, oldest first, redacted.</summary>
    public string DiagnosticsText() => _core.DiagnosticsText();

    /// <summary>What "copy diagnostics" puts on the clipboard: the version, platform and settings state, then the request log.</summary>
    /// <exception cref="OperationCanceledException"><paramref name="ct"/> was cancelled first.</exception>
    public Task<string> ExportDiagnosticsAsync(CancellationToken ct = default) =>
        CoreCall.RunAsync(token => _core.ExportDiagnostics(token), ct);

    /// <summary>
    /// The capabilities state, mirrored on the UI thread through <paramref name="ui"/>. Starts empty; the core's current value follows
    /// at once. Dispose it when the page goes away.
    /// </summary>
    public ValueSubscription<Capabilities> ObserveCapabilities(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<Capabilities>(Capabilities.Empty);
        subscription.Attach(_core.SubscribeCapabilities(new CapabilitiesForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>The theme the app shows now: the saved choice, <see cref="AppTheme.System"/> until one is made. Apply it before the
    /// window first shows.</summary>
    public AppTheme AppTheme => Map.From(_core.AppTheme());

    /// <summary>
    /// The app theme, mirrored on the UI thread through <paramref name="ui"/>. Starts at <see cref="AppTheme"/>; changes follow.
    /// Dispose it when the window goes away.
    /// </summary>
    public ValueSubscription<AppTheme> ObserveAppTheme(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<AppTheme>(AppTheme);
        subscription.Attach(_core.SubscribeAppTheme(new AppThemeForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>
    /// Changes the app theme and saves it at once, on the core's threads; observers see it once it is saved. A core that stores
    /// nothing (<see cref="Create"/>) only publishes it. Not cancellable: it is one small write.
    /// </summary>
    /// <exception cref="CoreStoreException">The settings could not be saved; the theme stays as it was.</exception>
    public Task SetAppThemeAsync(AppTheme theme) =>
        CoreCall.RunUncancellableAsync(async () =>
        {
            await _core.SetAppTheme(Map.ToFfi(theme)).ConfigureAwait(false);
            return true;
        });

    /// <summary>
    /// The saved servers now, in the order they were added, read from the core: a change shows here as soon as its call returns.
    /// </summary>
    public IReadOnlyList<SavedServer> Servers => Array.ConvertAll(_core.Servers(), saved => Map.From(saved));

    /// <summary>The saved servers, mirrored on the UI thread with diffs. Dispose it when the page goes away.</summary>
    public ListSubscription<SavedServer> ObserveServers(IUiDispatcher ui) =>
        new((subscription, generation) => _core.SubscribeServers(new SavedServersForwarder(subscription, ui, generation)));

    /// <summary>A new "add a server" flow.</summary>
    public AddServerFlow AddServer() => new(_core.AddServer());

    /// <summary>A new search for servers on the local network.</summary>
    public DiscoveryFlow DiscoverServers() => new(_core.DiscoverServers());

    /// <summary>A sign-in to a saved server, after its session ended.</summary>
    /// <exception cref="UnknownServerException">No such server is saved.</exception>
    /// <exception cref="ServerHasNoAddressException">The saved server has no address.</exception>
    public SignInFlow SignInAgain(ServerRef server) => new(CoreCall.Run(() => _core.SignInAgain(Map.ToFfi(server))));

    /// <summary>
    /// A sign-in to a saved server's gateway again (ADR 0017), when its status says <c>GatewaySignInNeeded</c>: for the address whose
    /// gateway asked. Call <see cref="GatewaySignInFlow.StartAsync"/> first. The session and its token were kept and go on.
    /// </summary>
    /// <exception cref="UnknownServerException">No such server is saved.</exception>
    /// <exception cref="ServerHasNoAddressException">The saved server has no usable address.</exception>
    public GatewaySignInFlow SignInToGateway(ServerRef server) =>
        new(CoreCall.Run(() => _core.SignInToGateway(Map.ToFfi(server))));

    /// <summary>Checks every saved session; the capabilities state shows the outcome per server.</summary>
    /// <exception cref="OperationCanceledException"><paramref name="ct"/> was cancelled first.</exception>
    public Task RestoreSessionsAsync(CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => _core.RestoreSessions(token), ct);

    /// <summary>
    /// Signs out of a server: this device forgets the session and token at once, then the server is told (at most 10 s). Not
    /// cancellable: half a sign-out would leave the token behind.
    /// </summary>
    /// <exception cref="UnknownServerException">No such server is saved.</exception>
    public async Task<SignOutResult> SignOutAsync(ServerRef server)
    {
        var result = await CoreCall.RunUncancellableAsync(() => _core.SignOut(Map.ToFfi(server))).ConfigureAwait(false);
        return new SignOutResult(result.ServerTold);
    }

    /// <summary>
    /// Removes a saved server: this device signs out every user there, deletes their tokens and forgets the server and its addresses
    /// at once; the server is told in the background. Removing a server that is not saved does nothing. Not cancellable, as
    /// sign-out.
    /// </summary>
    /// <exception cref="CoreStoreException">The settings could not be saved; nothing changed.</exception>
    public async Task<RemoveServerResult> RemoveServerAsync(ServerRef server)
    {
        var result = await CoreCall.RunUncancellableAsync(() => _core.RemoveServer(Map.ToFfi(server))).ConfigureAwait(false);
        return new RemoveServerResult(result.Removed, result.CredentialsLeft);
    }

    /// <summary>The platform's report of the network, on start and on every change (ADR 0014). Call it from one thread.</summary>
    public void NetworkChanged(NetworkFacts facts) => _core.NetworkChanged(Map.ToFfi(facts));

    /// <summary>The app came back to the foreground: unreachable servers try again at once.</summary>
    public void ReturnedToForeground() => _core.ReturnedToForeground();

    /// <summary>
    /// The main window is minimized, or no longer is: report it on every change. The live updates' socket closes once the window has
    /// been minimized for a minute and opens again on its restore (ADR 0023). A restore is also a return to the foreground.
    /// </summary>
    public void WindowMinimized(bool minimized) => _core.WindowMinimized(minimized);

    /// <summary>How many listeners the core still holds. A diagnostic for leak checks: it falls when subscriptions are disposed.</summary>
    public ulong LiveListeners => _core.LiveListeners();

    public void Dispose() => _core.Dispose();
}
