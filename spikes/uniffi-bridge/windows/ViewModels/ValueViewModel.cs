using System.Diagnostics;
using Microsoft.UI.Dispatching;
using uniffi.bridge_spike;

namespace BridgeSpike.ViewModels;

/// <summary>
/// Wraps the core's observable value. Owns the subscription's TaskHandle; the listener it gives the core holds only a
/// weak reference back, so the core never keeps the view model alive (docs/notes/uniffi-bridge.md, "Lifetimes").
/// </summary>
public sealed class ValueViewModel : Observable, IDisposable
{
    private readonly Core _core;
    private readonly DispatcherQueue _ui;
    private TaskHandle? _subscription;
    private bool _isSubscribed;
    private ulong _count;
    private string _threads = "";

    internal ValueViewModel(Core core, DispatcherQueue ui)
    {
        _core = core;
        _ui = ui;
        Subscribe();
    }

    public ulong Count { get => _count; private set => Set(ref _count, value); }

    /// <summary>Which thread the last callback arrived on, and which applied it.</summary>
    public string Threads { get => _threads; private set => Set(ref _threads, value); }

    public bool IsSubscribed { get => _isSubscribed; private set => Set(ref _isSubscribed, value); }

    public void Subscribe()
    {
        if (_subscription is not null) return;
        _subscription = _core.SubscribeValue(new Forwarder(this, _ui));
        IsSubscribed = true;
    }

    /// <summary>What a page does when it goes away: cancel, then release the handle now rather than at GC.</summary>
    public void Unsubscribe()
    {
        if (_subscription is null) return;
        _subscription.Cancel();
        _subscription.Dispose();
        _subscription = null;
        IsSubscribed = false;
    }

    public void Increment() => _core.Increment();
    public void StartTicker() => _core.StartTicker(200);
    public void StopTicker() => _core.StopTicker();

    public void Dispose() => Unsubscribe();

    private void Apply(Snapshot value, string callbackThread)
    {
        Debug.Assert(_ui.HasThreadAccess, "applied off the UI thread");
        Count = value.Count;
        Threads = $"callback on {callbackThread}; applied on managed thread {Environment.CurrentManagedThreadId} " +
                  $"(UI access: {_ui.HasThreadAccess}); last change made on {value.ChangedOn}";
    }

    /// <summary>
    /// Called by the core on one of its worker threads. Must never throw: UniFFI turns an exception in a callback
    /// that returns nothing into a Rust panic, which ends the subscription. Captures no Core: that would let the
    /// core's own task keep the core alive.
    /// </summary>
    private sealed class Forwarder(ValueViewModel owner, DispatcherQueue ui) : ValueListener
    {
        private readonly WeakReference<ValueViewModel> _owner = new(owner);

        public void OnValue(Snapshot value)
        {
            try
            {
                var thread = $"{BridgeSpikeMethods.CurrentThreadName()} (managed {Environment.CurrentManagedThreadId}, " +
                             $"UI access: {ui.HasThreadAccess})";
                // Callbacks are queued in order; TryEnqueue returns false only once the UI thread is shutting down.
                ui.TryEnqueue(() =>
                {
                    if (_owner.TryGetTarget(out var vm)) vm.Apply(value, thread);
                });
            }
            catch (Exception e)
            {
                Debug.WriteLine($"value listener failed: {e}");
            }
        }
    }
}
