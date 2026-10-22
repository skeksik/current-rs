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

/// <summary>The settings in the data folder could not be read or written. No path in it: it holds the user's account name.</summary>
public sealed class CoreStoreException(StoreProblem problem) : Exception("the settings could not be opened")
{
    public StoreProblem Problem { get; } = problem;
}

/// <summary>The flow is not in the state the call needs, e.g. signing in before setup is Ready.</summary>
public sealed class FlowNotReadyException() : Exception("the flow is not ready for this");

/// <summary>No saved server matches.</summary>
public sealed class UnknownServerException() : Exception("no such server is saved");

/// <summary>The server's address cannot take requests. Setup never saves such an address, so this points to a bug.</summary>
public sealed class InvalidServerAddressException() : Exception("the server's address cannot be used");

/// <summary>
/// Runs core calls and maps the generated exceptions to public ones: <c>Cancelled</c> becomes
/// <see cref="OperationCanceledException"/>, every other variant its own public exception.
/// </summary>
internal static class CoreCall
{
    public static T Run<T>(Func<T> call)
    {
        try
        {
            return call();
        }
        catch (Ffi.CoreException error)
        {
            throw Translate(error);
        }
    }

    /// <summary>Awaits a call that takes no token (it is short, or must not stop halfway).</summary>
    public static async Task<T> RunUncancellableAsync<T>(Func<Task<T>> call)
    {
        try
        {
            return await call().ConfigureAwait(false);
        }
        catch (Ffi.CoreException error)
        {
            throw Translate(error);
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
        catch (Ffi.CoreException error)
        {
            throw Translate(error);
        }
    }

    /// <summary><see cref="RunAsync{T}"/> for a call with no result.</summary>
    public static Task RunVoidAsync(Func<Ffi.CancelToken, Task> call, CancellationToken ct) =>
        RunAsync(async token =>
        {
            await call(token).ConfigureAwait(false);
            return true;
        }, ct);

    private static Exception Translate(Ffi.CoreException error) => error switch
    {
        Ffi.CoreException.Cancelled => new CoreCancelledException(),
        Ffi.CoreException.Runtime runtime => new CoreRuntimeException(runtime.@message),
        Ffi.CoreException.Store store => new CoreStoreException(MapProblems.From(store.@problem)),
        Ffi.CoreException.NotReady => new FlowNotReadyException(),
        Ffi.CoreException.UnknownServer => new UnknownServerException(),
        Ffi.CoreException.InvalidAddress => new InvalidServerAddressException(),
        _ => new CoreRuntimeException(error.Message),
    };
}
