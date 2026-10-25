using System.ComponentModel;
using System.Diagnostics;

namespace Current.Bindings;

/// <summary>
/// A list the core owns, mirrored on the UI thread with diffs (ADR 0004). <see cref="Items"/> changes only on the UI thread. The owner
/// disposes it when the page unloads; the finaliser of the core handle is only the backstop.
/// </summary>
/// <remarks>
/// The mirror heals itself. If a batch cannot be applied (an index out of range) or never reached the UI thread (the dispatcher
/// refused it, or mapping it failed), the mirror is out of step with the core. It then subscribes again, which starts with a fresh
/// Reset, and drops whatever the old subscription still had queued. <see cref="Recoveries"/> counts these. A failure that repeats
/// cannot loop: after <see cref="MaxRecoveries"/> recoveries within <see cref="RecoveryWindow"/> the mirror stops, traces an error
/// and reports <see cref="IsFaulted"/>; the owner may subscribe anew.
/// </remarks>
public sealed class ListSubscription<T> : INotifyPropertyChanged, IDisposable
{
    /// <summary>At most this many recoveries within <see cref="RecoveryWindow"/>; one more faults the mirror.</summary>
    public const int MaxRecoveries = 3;

    public static readonly TimeSpan RecoveryWindow = TimeSpan.FromSeconds(10);

    private readonly Func<ListSubscription<T>, int, uniffi.current_ffi.TaskHandle> _subscribe;
    private readonly Queue<DateTime> _recent = new();
    private uniffi.current_ffi.TaskHandle? _handle;
    private int _generation;

    /// <param name="subscribe">Subscribes a forwarder for this mirror, tagged with a generation; called again to recover.</param>
    internal ListSubscription(Func<ListSubscription<T>, int, uniffi.current_ffi.TaskHandle> subscribe)
    {
        _subscribe = subscribe;
        Subscribe();
    }

    /// <summary>Starts empty; the core's list follows at once as a Reset.</summary>
    public DiffObservableCollection<T> Items { get; } = new();

    public bool IsDisposed { get; private set; }

    /// <summary>How often the mirror fell out of step and subscribed again. A diagnostic: it stays 0 in normal use.</summary>
    public int Recoveries { get; private set; }

    /// <summary>
    /// The mirror fell out of step too often and stopped: <see cref="Items"/> no longer follows the core. <see cref="PropertyChanged"/>
    /// reports it, on the UI thread.
    /// </summary>
    public bool IsFaulted { get; private set; }

    /// <summary>Raised on the UI thread when <see cref="IsFaulted"/> becomes true.</summary>
    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>The current subscription's generation: batches from an older one are dropped.</summary>
    internal int Generation => Volatile.Read(ref _generation);

    /// <summary>Runs on the UI thread. A batch from an older subscription, or after <see cref="Dispose"/>, is dropped.</summary>
    internal void Apply(IReadOnlyList<ListDiff<T>> batch, int generation)
    {
        if (IsDisposed || IsFaulted || generation != Generation)
        {
            return;
        }
        try
        {
            Items.Apply(batch);
        }
        catch (ArgumentOutOfRangeException)
        {
            // The batch changed nothing (it is checked first); the mirror is behind.
            Recover(generation);
        }
    }

    /// <summary>Runs on the UI thread: the subscription of <paramref name="generation"/> lost a batch; subscribe again.</summary>
    internal void Recover(int generation)
    {
        if (IsDisposed || IsFaulted || generation != Generation)
        {
            return;
        }
        var now = DateTime.UtcNow;
        while (_recent.Count > 0 && now - _recent.Peek() > RecoveryWindow)
        {
            _recent.Dequeue();
        }
        Release();
        if (_recent.Count >= MaxRecoveries)
        {
            IsFaulted = true;
            // Batches still queued from the released subscription are dropped.
            Interlocked.Increment(ref _generation);
            Trace.TraceError($"Current: a core list fell out of step {MaxRecoveries + 1} times within {RecoveryWindow}; it stopped");
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsFaulted)));
            return;
        }
        _recent.Enqueue(now);
        Recoveries++;
        Subscribe();
    }

    private void Subscribe()
    {
        var generation = Interlocked.Increment(ref _generation);
        _handle = _subscribe(this, generation);
    }

    private void Release()
    {
        _handle?.Cancel();
        _handle?.Dispose();
        _handle = null;
    }

    /// <summary>Stops the subscription now: the core drops its listener and the native handle is freed.</summary>
    public void Dispose()
    {
        if (IsDisposed)
        {
            return;
        }
        IsDisposed = true;
        Release();
    }
}

/// <summary>
/// The listener the core holds for a list. Like <see cref="ValueForwarder{TRaw, T}"/>: a weak reference to its subscription, no core,
/// maps on the core thread, hops to the UI thread, and catches every exception. A batch it could not deliver makes the next
/// delivery a recovery instead (<see cref="ListSubscription{T}.Recover"/>).
/// </summary>
internal class ListForwarder<TRaw, T>(
    ListSubscription<T> target,
    IUiDispatcher ui,
    int generation,
    Func<TRaw, ListDiff<T>> map)
{
    private readonly WeakReference<ListSubscription<T>> _target = new(target);
    private int _lost;

    protected void Forward(TRaw[] raw)
    {
        try
        {
            if (!_target.TryGetTarget(out var target) || target.IsDisposed)
            {
                return;
            }
            if (Volatile.Read(ref _lost) == 1)
            {
                // A batch was lost: this one cannot be applied on top. Recover instead (the new subscription sends a Reset).
                if (Enqueue(() => target.Recover(generation)))
                {
                    Volatile.Write(ref _lost, 0);
                }
                return;
            }
            ListDiff<T>[] batch;
            try
            {
                batch = Array.ConvertAll(raw, diff => map(diff));
            }
            catch (Exception error)
            {
                Report(error);
                Lose(target);
                return;
            }
            if (!Enqueue(() => target.Apply(batch, generation)))
            {
                Lose(target);
            }
        }
        catch (Exception error)
        {
            Report(error);
        }
    }

    /// <summary>Asks for a recovery now if the UI thread takes it; otherwise the next delivery asks.</summary>
    private void Lose(ListSubscription<T> target)
    {
        Volatile.Write(ref _lost, 1);
        if (Enqueue(() => target.Recover(generation)))
        {
            Volatile.Write(ref _lost, 0);
        }
    }

    private bool Enqueue(Action work)
    {
        try
        {
            return ui.TryEnqueue(() =>
            {
                try
                {
                    work();
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
            return false;
        }
    }

    private static void Report(Exception error) =>
        Trace.TraceError($"Current: a core list listener failed: {error}");
}

internal sealed class SavedServersForwarder(ListSubscription<SavedServer> target, IUiDispatcher ui, int generation)
    : ListForwarder<uniffi.current_ffi.SavedServerDiff, SavedServer>(target, ui, generation, Map.From),
      uniffi.current_ffi.SavedServersListener
{
    public void OnDiffs(uniffi.current_ffi.SavedServerDiff[] diffs) => Forward(diffs);
}
