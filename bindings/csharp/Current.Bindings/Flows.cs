using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>A server that setup found. Its address stays in the core.</summary>
public sealed record NewServerInfo(string Id, string Name, string Version, string Product, bool HttpsFailedTls);

/// <summary>A server found on the local network. <c>Id</c> is what <see cref="AddServerFlow.Pick"/> takes.</summary>
public sealed record DiscoveredServer(string Id, string Name, string Version, bool NeedsConfirmation);

/// <summary>Where adding a server stands. The address the user typed is not repeated: the page has it.</summary>
public abstract record AddServerState
{
    private AddServerState() { }

    public sealed record Idle : AddServerState;
    public sealed record Probing : AddServerState;
    public sealed record Failed(ProbeProblem Problem) : AddServerState;
    /// <summary>A server answered over plain http outside the local network: the user confirms once (ADR 0012).</summary>
    public sealed record ConfirmPlainHttp(NewServerInfo Server) : AddServerState;
    /// <summary>Ready to sign in: call <see cref="AddServerFlow.SignIn"/>.</summary>
    public sealed record Ready(NewServerInfo Server) : AddServerState;
}

/// <summary>Where the search for local servers stands. Servers are listed in the order they answered.</summary>
/// <remarks>Record equality compares the lists by reference: do not use <c>==</c> to skip an unchanged value.</remarks>
public abstract record DiscoveryState
{
    private DiscoveryState() { }

    public sealed record Idle : DiscoveryState;
    public sealed record Searching(IReadOnlyList<DiscoveredServer> Found) : DiscoveryState;
    public sealed record Done(IReadOnlyList<DiscoveredServer> Found) : DiscoveryState;
    public sealed record Failed(DiscoveryProblem Problem) : DiscoveryState;
}

/// <summary>Where a sign-in stands.</summary>
public abstract record SignInState
{
    private SignInState() { }

    /// <summary>Waiting for the user. <c>QuickConnect</c> is whether the server offers it, once known.</summary>
    public sealed record Ready(bool? QuickConnect) : SignInState;
    public sealed record SigningIn : SignInState;
    /// <summary>The user enters <c>Code</c> in another signed-in client to approve this device.</summary>
    public sealed record QuickConnectWaiting(string Code) : SignInState;
    public sealed record Failed(SignInProblem Problem) : SignInState;
    /// <summary>Signed in and saved. <c>Remembered</c> is false when the keyring refused the token: the session lasts for this run.</summary>
    public sealed record SignedIn(ServerRef Server, UserInfo User, bool Remembered) : SignInState;
}

/// <summary>
/// Adding a server: probe a typed address or pick a discovered server, confirm plain http if asked, then sign in. One per page;
/// dispose it with the page.
/// </summary>
public sealed class AddServerFlow : IDisposable
{
    private readonly Ffi.AddServerFlow _flow;

    internal AddServerFlow(Ffi.AddServerFlow flow)
    {
        _flow = flow;
    }

    public AddServerState State => MapStates.From(_flow.State());

    /// <summary>The state, mirrored on the UI thread. Dispose it when the page goes away.</summary>
    public ValueSubscription<AddServerState> ObserveState(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<AddServerState>(State);
        subscription.Attach(_flow.SubscribeState(new AddServerStateForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>Probes the address the user typed; the outcome is the state.</summary>
    /// <exception cref="OperationCanceledException"><paramref name="ct"/> was cancelled; the flow is back to Idle.</exception>
    public Task ProbeAsync(string input, CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => _flow.Probe(input, token), ct);

    /// <summary>Picks a server <paramref name="discovery"/> lists. False if it does not list <paramref name="serverId"/>.</summary>
    public bool Pick(DiscoveryFlow discovery, string serverId) => _flow.Pick(discovery.Native, serverId);

    /// <summary>Confirms plain http when the state asks for it. False in any other state.</summary>
    public bool ConfirmPlainHttp() => _flow.ConfirmPlainHttp();

    /// <summary>Back to Idle, dropping a running probe.</summary>
    public void Reset() => _flow.Reset();

    /// <summary>A sign-in to the server the state is Ready with.</summary>
    /// <exception cref="FlowNotReadyException">The state is not Ready.</exception>
    public SignInFlow SignIn() => new(CoreCall.Run(() => _flow.SignIn()));

    public void Dispose() => _flow.Dispose();
}

/// <summary>Finding Jellyfin servers on the local network (ADR 0016). One per page; dispose it with the page.</summary>
public sealed class DiscoveryFlow : IDisposable
{
    internal DiscoveryFlow(Ffi.DiscoveryFlow flow)
    {
        Native = flow;
    }

    internal Ffi.DiscoveryFlow Native { get; }

    public DiscoveryState State => MapStates.From(Native.State());

    /// <summary>The state, mirrored on the UI thread. Dispose it when the page goes away.</summary>
    public ValueSubscription<DiscoveryState> ObserveState(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<DiscoveryState>(State);
        subscription.Attach(Native.SubscribeState(new DiscoveryStateForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>Searches once (a few seconds): Searching lists servers as they answer, then Done.</summary>
    /// <param name="interfaces">
    /// The device's up IPv4 interfaces, listed just before the call: the request goes out from each of them as well as from an
    /// unbound socket (ADR 0016). Empty searches from the unbound socket only. Other address families are skipped.
    /// </param>
    /// <exception cref="ArgumentNullException"><paramref name="interfaces"/> is null: pass an empty list instead.</exception>
    /// <exception cref="OperationCanceledException"><paramref name="ct"/> was cancelled; the flow is back to Idle.</exception>
    public Task SearchAsync(IReadOnlyList<LocalInterface> interfaces, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(interfaces);
        var native = Map.ToFfi(interfaces);
        return CoreCall.RunVoidAsync(token => Native.Search(native, token), ct);
    }

    /// <summary>Back to Idle, dropping a running search.</summary>
    public void Reset() => Native.Reset();

    public void Dispose() => Native.Dispose();
}

/// <summary>Signing in to one server with a password or Quick Connect (ADR 0015). Success saves the server and its user.</summary>
public sealed class SignInFlow : IDisposable
{
    private readonly Ffi.SignInFlow _flow;

    internal SignInFlow(Ffi.SignInFlow flow)
    {
        _flow = flow;
    }

    public SignInState State => MapStates.From(_flow.State());

    /// <summary>The state, mirrored on the UI thread. Dispose it when the page goes away.</summary>
    public ValueSubscription<SignInState> ObserveState(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<SignInState>(State);
        subscription.Attach(_flow.SubscribeState(new SignInStateForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>Asks whether the server offers Quick Connect; the answer shows in Ready. A failure leaves it unknown.</summary>
    public Task CheckQuickConnectAsync(CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => _flow.CheckQuickConnect(token), ct);

    /// <summary>Signs in with a user name and password. The core keeps the password nowhere.</summary>
    /// <exception cref="OperationCanceledException"><paramref name="ct"/> was cancelled; the flow is back to Ready.</exception>
    public Task WithPasswordAsync(string username, string password, CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => _flow.WithPassword(username, password, token), ct);

    /// <summary>Signs in with Quick Connect: the state shows the code until another client approves it.</summary>
    /// <exception cref="OperationCanceledException"><paramref name="ct"/> was cancelled; polling stops, the flow is back to Ready.</exception>
    public Task WithQuickConnectAsync(CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => _flow.WithQuickConnect(token), ct);

    /// <summary>Stops a running attempt and goes back to Ready. A finished sign-in stays signed in.</summary>
    public void Cancel() => _flow.Cancel();

    public void Dispose() => _flow.Dispose();
}

internal static class MapStates
{
    public static NewServerInfo From(Ffi.NewServerInfo server) =>
        new(server.Id, server.Name, server.Version, server.Product, server.HttpsFailedTls);

    public static DiscoveredServer From(Ffi.DiscoveredServerInfo server) =>
        new(server.Id, server.Name, server.Version, server.NeedsConfirmation);

    public static AddServerState From(Ffi.AddServerState state) => state switch
    {
        Ffi.AddServerState.Idle => new AddServerState.Idle(),
        Ffi.AddServerState.Probing => new AddServerState.Probing(),
        Ffi.AddServerState.Failed failed => new AddServerState.Failed(MapProblems.From(failed.Problem)),
        Ffi.AddServerState.ConfirmPlainHttp confirm => new AddServerState.ConfirmPlainHttp(From(confirm.Server)),
        Ffi.AddServerState.Ready ready => new AddServerState.Ready(From(ready.Server)),
        _ => throw new ArgumentException($"unknown state {state.GetType().Name}", nameof(state)),
    };

    public static DiscoveryState From(Ffi.DiscoveryState state) => state switch
    {
        Ffi.DiscoveryState.Idle => new DiscoveryState.Idle(),
        Ffi.DiscoveryState.Searching searching =>
            new DiscoveryState.Searching(Array.ConvertAll(searching.Found, server => From(server))),
        Ffi.DiscoveryState.Done done => new DiscoveryState.Done(Array.ConvertAll(done.Found, server => From(server))),
        Ffi.DiscoveryState.Failed failed => new DiscoveryState.Failed(MapProblems.From(failed.Problem)),
        _ => throw new ArgumentException($"unknown state {state.GetType().Name}", nameof(state)),
    };

    public static SignInState From(Ffi.SignInState state) => state switch
    {
        Ffi.SignInState.Ready ready => new SignInState.Ready(ready.QuickConnect),
        Ffi.SignInState.SigningIn => new SignInState.SigningIn(),
        Ffi.SignInState.QuickConnectWaiting waiting => new SignInState.QuickConnectWaiting(waiting.Code),
        Ffi.SignInState.Failed failed => new SignInState.Failed(MapProblems.From(failed.Problem)),
        Ffi.SignInState.SignedIn signedIn =>
            new SignInState.SignedIn(Map.From(signedIn.Server), Map.From(signedIn.User), signedIn.Remembered),
        _ => throw new ArgumentException($"unknown state {state.GetType().Name}", nameof(state)),
    };
}

internal sealed class AddServerStateForwarder(ValueSubscription<AddServerState> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.AddServerState, AddServerState>(target, ui, MapStates.From), Ffi.AddServerStateListener
{
    public void OnValue(Ffi.AddServerState value) => Forward(value);
}

internal sealed class DiscoveryStateForwarder(ValueSubscription<DiscoveryState> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.DiscoveryState, DiscoveryState>(target, ui, MapStates.From), Ffi.DiscoveryStateListener
{
    public void OnValue(Ffi.DiscoveryState value) => Forward(value);
}

internal sealed class SignInStateForwarder(ValueSubscription<SignInState> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.SignInState, SignInState>(target, ui, MapStates.From), Ffi.SignInStateListener
{
    public void OnValue(Ffi.SignInState value) => Forward(value);
}
