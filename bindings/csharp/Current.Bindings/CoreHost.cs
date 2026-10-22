using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>
/// The core, created once per process by the app (ADR 0004). The only public way into the generated bindings: everything it returns
/// is a public type of this library.
/// </summary>
public sealed class CoreHost : IDisposable
{
    private readonly Ffi.Core _core;

    private CoreHost(Ffi.Core core)
    {
        _core = core;
    }

    /// <summary>Starts the core.</summary>
    /// <exception cref="CoreRuntimeException">The core's runtime could not start.</exception>
    public static CoreHost Create() => new(CoreCall.Run(() => new Ffi.Core()));

    public AppInfo AppInfo => Map.From(_core.AppInfo());

    /// <summary>The request log as text, oldest first, redacted: what "copy diagnostics" puts on the clipboard.</summary>
    public string DiagnosticsText() => _core.DiagnosticsText();

    /// <summary>A diagnostics report for a bug report.</summary>
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

    /// <summary>How many listeners the core still holds. A diagnostic for leak checks: it falls when subscriptions are disposed.</summary>
    public ulong LiveListeners => _core.LiveListeners();

    public void Dispose() => _core.Dispose();
}
