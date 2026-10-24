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
    /// <summary>
    /// A password gateway (HTTP Basic) in front of the address (ADR 0017): ask for its user name and password
    /// (<see cref="AddServerFlow.SubmitPasswordAsync"/>). <c>Origin</c> names the address in the prompt. <c>Realm</c> is the site's
    /// own text: show it only as "The site says: ...", never as a title. <c>Wrong</c>: the password just given was refused.
    /// <c>PlainHttp</c>: plain http outside the local network, so the prompt also asks to confirm that.
    /// </summary>
    public sealed record GatewayPassword(string Origin, string? Realm, bool Wrong, bool PlainHttp) : AddServerState;
    /// <summary>
    /// The address shows a web page where the server's answer was expected: a gateway's sign-in, or a plain web site. Say so
    /// ("This address shows a web page. If it is your server's sign-in, sign in; otherwise check the address.") and offer "Sign in",
    /// which opens the sign-in window on <c>Url</c>, titled with <c>Origin</c>. After each navigation that lands on <c>Origin</c>, hand
    /// the window's cookies over (<see cref="AddServerFlow.SubmitCookiesAsync"/>). <c>Redirected</c>: a redirect to another host or
    /// port, which may be a moved server: also offer "Use the address it points to" (<see cref="AddServerFlow.FollowRedirectAsync"/>),
    /// naming <c>LocationOrigin</c> when known.
    /// </summary>
    public sealed record GatewaySignIn(string Url, string Origin, bool Redirected, string? LocationOrigin, bool PlainHttp)
        : AddServerState;
    /// <summary>
    /// The gateway refuses without a sign-in: offer "Advanced: extra headers" (<see cref="AddServerFlow.SubmitHeadersAsync"/>) with
    /// an explanation.
    /// </summary>
    public sealed record GatewayRefused(string Origin, bool PlainHttp) : AddServerState;
    /// <summary>The address asks for a sign-in method Current does not support.</summary>
    public sealed record GatewayUnsupported(string Origin) : AddServerState;
}

/// <summary>
/// A cookie the sign-in window's web view holds, read after a navigation (ADR 0017). <c>Domain</c> as the web view reports it: with
/// a leading dot for a domain cookie. <c>Expires</c> is null for a session cookie. Its text form shows no name and no value.
/// </summary>
public sealed record GatewayCookie(
    string Name, string Value, string Domain, string Path, bool Secure, bool HttpOnly, DateTimeOffset? Expires)
{
    private bool PrintMembers(System.Text.StringBuilder builder)
    {
        builder.Append("[redacted]");
        return true;
    }
}

/// <summary>An extra header the user typed (advanced, ADR 0017). Its text form shows no name and no value.</summary>
public sealed record GatewayHeader(string Name, string Value)
{
    private bool PrintMembers(System.Text.StringBuilder builder)
    {
        builder.Append("[redacted]");
        return true;
    }
}

/// <summary>
/// Where signing in to a saved server's gateway again stands (ADR 0017). As <see cref="AddServerState"/>'s gateway states:
/// <c>Realm</c> only as "The site says: ...".
/// </summary>
public abstract record GatewaySignInState
{
    private GatewaySignInState() { }

    /// <summary>Asking the address what its gateway wants, or trying what was just given.</summary>
    public sealed record Checking : GatewaySignInState;
    public sealed record Password(string Origin, string? Realm, bool Wrong) : GatewaySignInState;
    /// <summary>Open the sign-in window on <c>Url</c> and hand its cookies over after each navigation that lands on <c>Origin</c>.</summary>
    public sealed record WebSignIn(string Url, string Origin, bool Redirected, string? LocationOrigin) : GatewaySignInState;
    /// <summary>Extra headers (advanced).</summary>
    public sealed record Refused(string Origin) : GatewaySignInState;
    public sealed record Unsupported(string Origin) : GatewaySignInState;
    /// <summary>Signed in to the gateway again and saved: the session goes on. <c>Remembered</c> is false when the keyring refused.</summary>
    public sealed record Done(bool Remembered) : GatewaySignInState;
    public sealed record Failed(GatewaySignInProblem Problem) : GatewaySignInState;
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

    /// <summary>
    /// The gateway's user name and password, in GatewayPassword: the address is probed again with them, never with a Jellyfin token.
    /// <paramref name="confirmPlainHttp"/> is the user's agreement where the state says <c>PlainHttp</c>. Cancelling stops only the
    /// waiting: the probe ends and sets the state.
    /// </summary>
    /// <exception cref="FlowNotReadyException">The state does not ask for a password.</exception>
    /// <exception cref="GatewayInputException">They cannot be used, or plain http is not confirmed: nothing was sent.</exception>
    public Task SubmitPasswordAsync(string user, string password, bool confirmPlainHttp, CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => _flow.SubmitPassword(user, password, confirmPlainHttp, token), ct);

    /// <summary>
    /// The sign-in window's cookies, in GatewaySignIn, after each navigation that lands on the state's <c>Origin</c>. Pass every
    /// cookie the web view holds: only those that apply to the server's address are kept. The state moves on once they let the
    /// server through; until then keep the window open.
    /// </summary>
    /// <exception cref="FlowNotReadyException">The state is not GatewaySignIn.</exception>
    /// <exception cref="GatewayInputException">Plain http is not confirmed.</exception>
    public Task SubmitCookiesAsync(IReadOnlyList<GatewayCookie> cookies, bool confirmPlainHttp, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(cookies);
        var native = MapStates.ToFfi(cookies);
        return CoreCall.RunVoidAsync(token => _flow.SubmitCookies(native, confirmPlainHttp, token), ct);
    }

    /// <summary>Extra headers (advanced), in GatewayPassword, GatewaySignIn or GatewayRefused.</summary>
    /// <exception cref="FlowNotReadyException">No gateway state asks for them.</exception>
    /// <exception cref="GatewayInputException">None given, a name Current sets itself, or a value outside visible ASCII.</exception>
    public Task SubmitHeadersAsync(IReadOnlyList<GatewayHeader> headers, bool confirmPlainHttp, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(headers);
        var native = MapStates.ToFfi(headers);
        return CoreCall.RunVoidAsync(token => _flow.SubmitHeaders(native, confirmPlainHttp, token), ct);
    }

    /// <summary>"Use the address it points to", in GatewaySignIn after a redirect: probes it as if typed. The address stays in the core.</summary>
    /// <exception cref="FlowNotReadyException">Not after a redirect, or the server sent no address.</exception>
    public Task FollowRedirectAsync(CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => _flow.FollowRedirect(token), ct);

    /// <summary>Back to Idle, dropping a running probe and the gateway credentials given so far.</summary>
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

/// <summary>
/// Signing in to a saved server's gateway again, when its status says <c>GatewaySignInNeeded</c> (ADR 0017). Call
/// <see cref="StartAsync"/> first. One per sign-in window or prompt; dispose it with them.
/// </summary>
public sealed class GatewaySignInFlow : IDisposable
{
    private readonly Ffi.GatewaySignInFlow _flow;

    internal GatewaySignInFlow(Ffi.GatewaySignInFlow flow)
    {
        _flow = flow;
    }

    public GatewaySignInState State => MapStates.From(_flow.State());

    /// <summary>The state, mirrored on the UI thread. Dispose it when the window goes away.</summary>
    public ValueSubscription<GatewaySignInState> ObserveState(IUiDispatcher ui)
    {
        var subscription = new ValueSubscription<GatewaySignInState>(State);
        subscription.Attach(_flow.SubscribeState(new GatewaySignInStateForwarder(subscription, ui)));
        return subscription;
    }

    /// <summary>Asks the address what its gateway wants: a gateway state, or Done when it lets the server through already.</summary>
    public Task StartAsync(CancellationToken ct = default) => CoreCall.RunVoidAsync(token => _flow.Start(token), ct);

    /// <summary>The gateway's user name and password, in Password.</summary>
    /// <exception cref="FlowNotReadyException">The state does not ask for a password.</exception>
    /// <exception cref="GatewayInputException">They cannot be used: nothing was sent.</exception>
    public Task SubmitPasswordAsync(string user, string password, CancellationToken ct = default) =>
        CoreCall.RunVoidAsync(token => _flow.SubmitPassword(user, password, token), ct);

    /// <summary>The sign-in window's cookies, in WebSignIn, after each navigation that lands on the state's <c>Origin</c>.</summary>
    /// <exception cref="FlowNotReadyException">The state is not WebSignIn.</exception>
    public Task SubmitCookiesAsync(IReadOnlyList<GatewayCookie> cookies, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(cookies);
        var native = MapStates.ToFfi(cookies);
        return CoreCall.RunVoidAsync(token => _flow.SubmitCookies(native, token), ct);
    }

    /// <summary>Extra headers (advanced), in Password, WebSignIn or Refused.</summary>
    /// <exception cref="FlowNotReadyException">No state asks for them.</exception>
    /// <exception cref="GatewayInputException">None given, a reserved name, or a value outside visible ASCII.</exception>
    public Task SubmitHeadersAsync(IReadOnlyList<GatewayHeader> headers, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(headers);
        var native = MapStates.ToFfi(headers);
        return CoreCall.RunVoidAsync(token => _flow.SubmitHeaders(native, token), ct);
    }

    /// <summary>Stops the flow (the window or prompt closed): a running attempt's result is dropped.</summary>
    public void Cancel() => _flow.Cancel();

    public void Dispose() => _flow.Dispose();
}

internal static class MapStates
{
    public static Ffi.GatewayCookie[] ToFfi(IReadOnlyList<GatewayCookie> cookies) => cookies
        .Select(c => new Ffi.GatewayCookie(
            c.Name, c.Value, c.Domain, c.Path, c.Secure, c.HttpOnly, c.Expires?.ToUnixTimeMilliseconds()))
        .ToArray();

    public static Ffi.GatewayHeader[] ToFfi(IReadOnlyList<GatewayHeader> headers) => headers
        .Select(h => new Ffi.GatewayHeader(h.Name, h.Value))
        .ToArray();

    public static GatewaySignInState From(Ffi.GatewaySignInState state) => state switch
    {
        Ffi.GatewaySignInState.Checking => new GatewaySignInState.Checking(),
        Ffi.GatewaySignInState.Password password =>
            new GatewaySignInState.Password(password.Origin, password.Realm, password.Wrong),
        Ffi.GatewaySignInState.WebSignIn web =>
            new GatewaySignInState.WebSignIn(web.Url, web.Origin, web.Redirected, web.LocationOrigin),
        Ffi.GatewaySignInState.Refused refused => new GatewaySignInState.Refused(refused.Origin),
        Ffi.GatewaySignInState.Unsupported unsupported => new GatewaySignInState.Unsupported(unsupported.Origin),
        Ffi.GatewaySignInState.Done done => new GatewaySignInState.Done(done.Remembered),
        Ffi.GatewaySignInState.Failed failed => new GatewaySignInState.Failed(MapProblems.From(failed.Problem)),
        _ => throw new ArgumentException($"unknown state {state.GetType().Name}", nameof(state)),
    };

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
        Ffi.AddServerState.GatewayPassword password =>
            new AddServerState.GatewayPassword(password.Origin, password.Realm, password.Wrong, password.PlainHttp),
        Ffi.AddServerState.GatewaySignIn web =>
            new AddServerState.GatewaySignIn(web.Url, web.Origin, web.Redirected, web.LocationOrigin, web.PlainHttp),
        Ffi.AddServerState.GatewayRefused refused => new AddServerState.GatewayRefused(refused.Origin, refused.PlainHttp),
        Ffi.AddServerState.GatewayUnsupported unsupported => new AddServerState.GatewayUnsupported(unsupported.Origin),
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

internal sealed class GatewaySignInStateForwarder(ValueSubscription<GatewaySignInState> target, IUiDispatcher ui)
    : ValueForwarder<Ffi.GatewaySignInState, GatewaySignInState>(target, ui, MapStates.From), Ffi.GatewaySignInStateListener
{
    public void OnValue(Ffi.GatewaySignInState value) => Forward(value);
}
