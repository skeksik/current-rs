using uniffi.bridge_spike;

namespace BridgeSpike.ViewModels;

/// <summary>
/// The C# side of the cancellation rule (docs/notes/uniffi-bridge.md): the generated Task ignores cancellation, so
/// every call gets a CancelToken that the caller's CancellationToken cancels.
/// </summary>
internal static class CoreAsync
{
    public static async Task<string> SlowReverseAsync(this Core core, string text, ulong delayMs, CancellationToken ct)
    {
        using var token = new CancelToken();
        using var registration = ct.Register(token.Cancel);
        try
        {
            return await core.SlowReverse(text, delayMs, token);
        }
        catch (BridgeException.Cancelled) when (ct.IsCancellationRequested)
        {
            throw new OperationCanceledException(ct);
        }
    }
}
