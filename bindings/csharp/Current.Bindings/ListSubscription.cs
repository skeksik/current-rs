using System.Diagnostics;

namespace Current.Bindings;

/// <summary>
/// A list the core owns, mirrored on the UI thread with diffs (ADR 0004). <see cref="Items"/> changes only on the UI thread. The owner
/// disposes it when the page unloads; the finaliser of the core handle is only the backstop.
/// </summary>
public sealed class ListSubscription<T> : IDisposable
{
    private uniffi.current_ffi.TaskHandle? _handle;

    internal ListSubscription()
    {
    }

    /// <summary>Starts empty; the core's list follows at once as a Reset.</summary>
    public DiffObservableCollection<T> Items { get; } = new();

    public bool IsDisposed { get; private set; }

    internal void Attach(uniffi.current_ffi.TaskHandle handle) => _handle = handle;

    /// <summary>Runs on the UI thread. A batch that arrives after <see cref="Dispose"/> is dropped.</summary>
    /// <exception cref="ArgumentOutOfRangeException">The mirror is out of step with the core; re-subscribe.</exception>
    internal void Apply(IReadOnlyList<ListDiff<T>> batch)
    {
        if (IsDisposed)
        {
            return;
        }
        Items.Apply(batch);
    }

    /// <summary>Stops the subscription now: the core drops its listener and the native handle is freed.</summary>
    public void Dispose()
    {
        if (IsDisposed)
        {
            return;
        }
        IsDisposed = true;
        _handle?.Cancel();
        _handle?.Dispose();
        _handle = null;
    }
}

/// <summary>
/// The listener the core holds for a list. Like <see cref="ValueForwarder{TRaw, T}"/>: a weak reference to its subscription, no core,
/// maps on the core worker thread, hops to the UI thread, and catches every exception.
/// </summary>
internal class ListForwarder<TRaw, T>(ListSubscription<T> target, IUiDispatcher ui, Func<TRaw, ListDiff<T>> map)
{
    private readonly WeakReference<ListSubscription<T>> _target = new(target);

    protected void Forward(TRaw[] raw)
    {
        try
        {
            if (!_target.TryGetTarget(out var target) || target.IsDisposed)
            {
                return;
            }
            var batch = Array.ConvertAll(raw, diff => map(diff));
            ui.TryEnqueue(() =>
            {
                try
                {
                    target.Apply(batch);
                }
                catch (Exception error)
                {
                    Report(error);
                }
            });
        }
        catch (Exception error)
        {
            Report(error);
        }
    }

    private static void Report(Exception error) =>
        Trace.TraceError($"Current: a core list listener failed: {error}");
}

internal sealed class SavedServersForwarder(ListSubscription<SavedServer> target, IUiDispatcher ui)
    : ListForwarder<uniffi.current_ffi.SavedServerDiff, SavedServer>(target, ui, Map.From),
      uniffi.current_ffi.SavedServersListener
{
    public void OnDiffs(uniffi.current_ffi.SavedServerDiff[] diffs) => Forward(diffs);
}
