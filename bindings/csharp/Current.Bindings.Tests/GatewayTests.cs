using Current.Bindings;

/// <summary>
/// Authentication gateways through the public adapters (ADR 0017), against the Jellyfin stand-in's gateway modes: setup's gateway
/// states and actions, the input exceptions, and signing in to the gateway again once its session ended.
/// </summary>
internal static class GatewayTests
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

    private static GatewayCookie Cookie(string value) =>
        new(StandIn.GateCookie, value, "127.0.0.1", "/", Secure: false, HttpOnly: true, Expires: null);

    private static async Task<T> Throws<T>(Func<Task> call, string what) where T : Exception
    {
        try
        {
            await call();
        }
        catch (T error)
        {
            return error;
        }
        throw new Exception($"expected {typeof(T).Name}: {what}");
    }

    private static async Task SignIn(AddServerFlow add)
    {
        using var flow = add.SignIn();
        await flow.WithPasswordAsync(StandIn.UserName, StandIn.Password);
        Check(flow.State is SignInState.SignedIn, $"signed in, not {flow.State}");
    }

    public static async Task BasicGatewayAsksForItsPassword()
    {
        using var standIn = StandIn.Start();
        standIn.Send("gate basic");
        using var core = CoreHost.Create();
        using var add = core.AddServer();
        await add.ProbeAsync(standIn.Address);
        Check(add.State == new AddServerState.GatewayPassword(standIn.Address, StandIn.GateRealm, Wrong: false, PlainHttp: false),
            $"the password prompt, not {add.State}");
        var invalid = await Throws<GatewayInputException>(
            () => add.SubmitPasswordAsync("a:b", StandIn.GatePassword, confirmPlainHttp: false), "a user name with a colon");
        Check(invalid.Problem == GatewayInputProblem.InvalidUserName, $"{invalid.Problem}");
        await Throws<FlowNotReadyException>(
            () => add.SubmitCookiesAsync([Cookie(StandIn.GateCookieValue)], confirmPlainHttp: false), "cookies are not asked for");
        await add.SubmitPasswordAsync(StandIn.GateUser, "wrong", confirmPlainHttp: false);
        Check(add.State is AddServerState.GatewayPassword { Wrong: true }, $"asked again, not {add.State}");
        await add.SubmitPasswordAsync(StandIn.GateUser, StandIn.GatePassword, confirmPlainHttp: false);
        Check(add.State is AddServerState.Ready, $"ready, not {add.State}");
        await SignIn(add);
        var diagnostics = core.DiagnosticsText();
        Check(!diagnostics.Contains(StandIn.GatePassword, StringComparison.Ordinal), "no password in the log");
    }

    public static async Task HeaderGateTakesExtraHeaders()
    {
        using var standIn = StandIn.Start();
        standIn.Send("gate header");
        using var core = CoreHost.Create();
        using var add = core.AddServer();
        await add.ProbeAsync(standIn.Address);
        Check(add.State == new AddServerState.GatewayRefused(standIn.Address, PlainHttp: false), $"refused, not {add.State}");
        var reserved = await Throws<GatewayInputException>(
            () => add.SubmitHeadersAsync([new GatewayHeader("Cookie", "x")], confirmPlainHttp: false), "a reserved name");
        Check(reserved.Problem == GatewayInputProblem.ReservedHeader, $"{reserved.Problem}");
        await add.SubmitHeadersAsync([new GatewayHeader(StandIn.GateHeader, StandIn.GateHeaderValue)], confirmPlainHttp: false);
        Check(add.State is AddServerState.Ready, $"ready, not {add.State}");
        Check(!core.DiagnosticsText().Contains(StandIn.GateHeaderValue, StringComparison.Ordinal), "no header value in the log");
    }

    public static async Task PortalSignInAndAgain()
    {
        using var standIn = StandIn.Start();
        standIn.Send("gate portal");
        using var core = CoreHost.Create();
        using (var add = core.AddServer())
        {
            await add.ProbeAsync(standIn.Address);
            Check(add.State == new AddServerState.GatewaySignIn(standIn.Address, standIn.Address, Redirected: false, LocationOrigin: null,
                PlainHttp: false), $"the sign-in window, not {add.State}");
            await Throws<FlowNotReadyException>(() => add.FollowRedirectAsync(), "no redirect to follow");
            // A cookie the portal does not take yet: still waiting.
            await add.SubmitCookiesAsync([Cookie("not-yet")], confirmPlainHttp: false);
            Check(add.State is AddServerState.GatewaySignIn, $"still waiting, not {add.State}");
            await add.SubmitCookiesAsync([Cookie(StandIn.GateCookieValue)], confirmPlainHttp: false);
            Check(add.State is AddServerState.Ready, $"ready, not {add.State}");
            await SignIn(add);
        }
        var ui = new ManualDispatcher();
        using var capabilities = core.ObserveCapabilities(ui);
        ServerStatus? Status() => capabilities.Value.Servers.Count == 1 ? capabilities.Value.Servers[0] : null;
        await Eventually("the address in use", ui, () => Status()?.AddressPosition == 1);
        Check(Status() is { GatewaySignInNeeded: false }, "no gateway sign-in needed yet");

        // The portal's session ends; on the next network the server cannot be reached through it, and asks for the gateway.
        standIn.Send("gate-cookie renewed-gate-cookie");
        core.NetworkChanged(new NetworkFacts(Connectivity.Full, null, null, InterfaceKind.Wifi, null, "away"));
        await Eventually("the gateway asks again", ui, () => Status() is { GatewaySignInNeeded: true });
        Check(Status() is { SignedIn: true }, "still signed in");

        using var flow = core.SignInToGateway(FirstServer);
        await flow.StartAsync();
        Check(flow.State is GatewaySignInState.WebSignIn { Redirected: false }, $"the sign-in window, not {flow.State}");
        await flow.SubmitCookiesAsync([Cookie("renewed-gate-cookie")]);
        Check(flow.State is GatewaySignInState.Done, $"done, not {flow.State}");
        await Eventually("the status clears", ui, () => Status() is { GatewaySignInNeeded: false });
        var diagnostics = core.DiagnosticsText();
        Check(!diagnostics.Contains("renewed-gate-cookie", StringComparison.Ordinal), "no cookie in the log");
        try
        {
            core.SignInToGateway(new ServerRef("jellyfin", "9"));
            Check(false, "an unknown server is refused");
        }
        catch (UnknownServerException)
        {
        }
    }

    public static Task GatewayInputsHideTheirValues()
    {
        var text = $"{Cookie("secret-cookie-value")} {new GatewayHeader("X-Secret-Name", "secret-header-value")}";
        Check(!text.Contains("secret", StringComparison.OrdinalIgnoreCase), text);
        Check(!text.Contains(StandIn.GateCookie, StringComparison.Ordinal), text);
        return Task.CompletedTask;
    }
}
