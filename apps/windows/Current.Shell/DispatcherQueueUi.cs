using Current.Bindings;
using Microsoft.UI.Dispatching;

namespace Current.Shell;

/// <summary>The UI thread's queue for core callbacks (ADR 0004): work enqueued with the same priority runs in order.</summary>
internal sealed class DispatcherQueueUi(DispatcherQueue queue) : IUiDispatcher
{
    public bool TryEnqueue(Action work) => queue.TryEnqueue(() => work());
}
