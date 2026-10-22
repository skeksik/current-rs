using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>The core's runtime could not start or lost the task doing the work.</summary>
public sealed class CoreRuntimeException(string message) : Exception(message);

/// <summary>
/// The core ended a call as cancelled although the caller's <see cref="CancellationToken"/> was not cancelled. Not expected today
/// (the token is private to the call); kept distinct from <see cref="OperationCanceledException"/> so it is never mistaken for the
/// caller's own cancellation.
/// </summary>
public sealed class CoreCancelledException() : Exception("the core cancelled the call without a cancellation request");

/// <summary>
/// Runs core calls and maps the generated exceptions to public ones: <c>Cancelled</c> becomes
/// <see cref="OperationCanceledException"/>, <c>Runtime</c> becomes <see cref="CoreRuntimeException"/>.
/// </summary>
internal static class CoreCall
{
    public static T Run<T>(Func<T> call)
    {
        try
        {
            return call();
        }
        catch (Ffi.CoreException.Runtime error)
        {
            throw new CoreRuntimeException(error.@message);
        }
    }

    /// <summary>
    /// Runs a cancellable call with a fresh <c>CancelToken</c> that <paramref name="ct"/> cancels (ADR 0004): UniFFI never cancels
    /// a Rust future by itself, so without this the work would run to its end. <c>Cancelled</c> becomes
    /// <see cref="OperationCanceledException"/> only when <paramref name="ct"/> asked for it.
    /// </summary>
    public static async Task<T> RunAsync<T>(Func<Ffi.CancelToken, Task<T>> call, CancellationToken ct)
    {
        using var token = new Ffi.CancelToken();
        using var registration = ct.Register(token.Cancel);
        try
        {
            return await call(token).ConfigureAwait(false);
        }
        catch (Ffi.CoreException.Cancelled) when (ct.IsCancellationRequested)
        {
            throw new OperationCanceledException(ct);
        }
        catch (Ffi.CoreException.Cancelled)
        {
            throw new CoreCancelledException();
        }
        catch (Ffi.CoreException.Runtime error)
        {
            throw new CoreRuntimeException(error.@message);
        }
    }
}
