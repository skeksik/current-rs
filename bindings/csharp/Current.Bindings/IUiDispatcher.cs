namespace Current.Bindings;

/// <summary>
/// The UI thread's queue. Core callbacks arrive on a core thread and must hop here before touching UI objects (ADR 0004).
/// The WinUI shell implements it over <c>DispatcherQueue.TryEnqueue</c>; tests use a queue they drain by hand.
/// </summary>
public interface IUiDispatcher
{
    /// <summary>Queues <paramref name="work"/> to run on the UI thread, in order. False when the queue is shutting down.</summary>
    bool TryEnqueue(Action work);
}
