// Tests for the adapters, against the real core. A plain runner: no test framework package. Prints one line per test and
// "<n> passed, <m> failed"; exits non-zero on any failure.

using System.Collections.Specialized;
using System.Runtime.CompilerServices;
using Current.Bindings;

var tests = new (string Name, Func<Task> Run)[]
{
    ("a diff batch applies in order", Tests.DiffBatchAppliesInOrder),
    ("a reset raises one Reset event", Tests.ResetRaisesOneEvent),
    ("a bad index throws and changes nothing", Tests.BadIndexChangesNothing),
    ("a batch with a bad index changes nothing", Tests.BadBatchChangesNothing),
    ("values arrive through the dispatcher", Tests.ValuesArriveThroughTheDispatcher),
    ("disposing releases the listener", Tests.DisposingReleasesTheListener),
    ("a delivery after dispose is dropped", Tests.DeliveryAfterDisposeIsDropped),
    ("a forgotten subscription is released by the GC", Tests.ForgottenSubscriptionIsReleased),
    ("a failing dispatcher does not end the subscription", Tests.FailingDispatcherDoesNotEndTheSubscription),
    ("export diagnostics completes", Tests.ExportDiagnosticsCompletes),
    ("a cancelled token cancels the call", Tests.CancelledTokenCancelsTheCall),
    ("a cancel the caller did not ask for is not an OperationCanceledException", Tests.UnrequestedCancelIsItsOwnException),
};

var failed = 0;
foreach (var (name, run) in tests)
{
    try
    {
        await run();
        Console.WriteLine($"PASS {name}");
    }
    catch (Exception error)
    {
        failed++;
        Console.WriteLine($"FAIL {name}: {error}");
    }
}
Console.WriteLine($"{tests.Length - failed} passed, {failed} failed");
return failed == 0 ? 0 : 1;

/// <summary>A UI queue the test drains by hand, so it can see that nothing reaches the model without the hop.</summary>
internal sealed class ManualDispatcher : IUiDispatcher
{
    private readonly Queue<Action> _queue = new();

    public bool TryEnqueue(Action work)
    {
        lock (_queue)
        {
            _queue.Enqueue(work);
        }
        return true;
    }

    public int Pending
    {
        get
        {
            lock (_queue)
            {
                return _queue.Count;
            }
        }
    }

    public void Drain()
    {
        while (true)
        {
            Action? work;
            lock (_queue)
            {
                if (!_queue.TryDequeue(out work))
                {
                    return;
                }
            }
            work();
        }
    }
}

internal sealed class ThrowingDispatcher : IUiDispatcher
{
    public int Calls;

    public bool TryEnqueue(Action work)
    {
        Interlocked.Increment(ref Calls);
        throw new InvalidOperationException("the UI is gone");
    }
}

internal static class Tests
{
    private static readonly TimeSpan Wait = TimeSpan.FromSeconds(5);

    private static void Check(bool condition, string what)
    {
        if (!condition)
        {
            throw new Exception($"expected: {what}");
        }
    }

    private static async Task Eventually(string what, Func<bool> done)
    {
        var start = DateTime.UtcNow;
        while (!done())
        {
            Check(DateTime.UtcNow - start < Wait, what);
            await Task.Delay(10);
        }
    }

    public static Task DiffBatchAppliesInOrder()
    {
        var list = new DiffObservableCollection<string>();
        var events = new List<NotifyCollectionChangedAction>();
        list.CollectionChanged += (_, e) => events.Add(e.Action);
        list.Apply([
            new ListDiff<string>.Reset(["a", "b"]),
            new ListDiff<string>.Insert(0, "z"),
            new ListDiff<string>.Remove(2),
            new ListDiff<string>.Set(1, "y"),
            new ListDiff<string>.Append(["c"]),
        ]);
        Check(list.SequenceEqual(new[] { "z", "y", "c" }), "z, y, c");
        Check(events.SequenceEqual(new[]
        {
            NotifyCollectionChangedAction.Reset,
            NotifyCollectionChangedAction.Add,
            NotifyCollectionChangedAction.Remove,
            NotifyCollectionChangedAction.Replace,
            NotifyCollectionChangedAction.Add,
        }), "one event per diff, in order");
        return Task.CompletedTask;
    }

    public static Task ResetRaisesOneEvent()
    {
        var list = new DiffObservableCollection<int>();
        var events = 0;
        list.CollectionChanged += (_, _) => events++;
        list.Apply(new ListDiff<int>.Reset([1, 2, 3]));
        Check(events == 1, "one event");
        Check(list.Count == 3, "three items");
        return Task.CompletedTask;
    }

    public static Task BadIndexChangesNothing()
    {
        var list = new DiffObservableCollection<string>();
        list.Apply(new ListDiff<string>.Append(["a"]));
        foreach (var diff in new ListDiff<string>[] { new ListDiff<string>.Insert(2, "b"), new ListDiff<string>.Set(1, "b"), new ListDiff<string>.Remove(-1) })
        {
            try
            {
                list.Apply(diff);
                Check(false, $"{diff} throws");
            }
            catch (ArgumentOutOfRangeException)
            {
            }
        }
        Check(list.SequenceEqual(new[] { "a" }), "unchanged");
        return Task.CompletedTask;
    }

    public static Task BadBatchChangesNothing()
    {
        var list = new DiffObservableCollection<string>();
        list.Apply(new ListDiff<string>.Reset(["a", "b"]));
        var events = 0;
        list.CollectionChanged += (_, _) => events++;
        try
        {
            list.Apply([new ListDiff<string>.Remove(0), new ListDiff<string>.Set(5, "x"), new ListDiff<string>.Append(["c"])]);
            Check(false, "the batch throws");
        }
        catch (ArgumentOutOfRangeException)
        {
        }
        Check(list.SequenceEqual(new[] { "a", "b" }), "unchanged");
        Check(events == 0, "no events");
        return Task.CompletedTask;
    }

    public static async Task ValuesArriveThroughTheDispatcher()
    {
        using var core = CoreHost.Create();
        var ui = new ManualDispatcher();
        using var capabilities = core.ObserveCapabilities(ui);
        var changes = 0;
        capabilities.PropertyChanged += (_, _) => changes++;

        await Eventually("the first value is queued", () => ui.Pending == 1);
        Check(changes == 0, "nothing changes before the UI thread runs the queue");
        ui.Drain();
        Check(changes == 1, "one change after the hop");
        Check(capabilities.Value.Servers.Count == 0, "no servers yet");
    }

    public static async Task DisposingReleasesTheListener()
    {
        using var core = CoreHost.Create();
        var ui = new ManualDispatcher();
        var capabilities = core.ObserveCapabilities(ui);
        Check(core.LiveListeners == 1, "one listener");
        capabilities.Dispose();
        await Eventually("the core drops the listener", () => core.LiveListeners == 0);
    }

    public static async Task DeliveryAfterDisposeIsDropped()
    {
        using var core = CoreHost.Create();
        var ui = new ManualDispatcher();
        var capabilities = core.ObserveCapabilities(ui);
        var changes = 0;
        capabilities.PropertyChanged += (_, _) => changes++;
        await Eventually("the first value is queued", () => ui.Pending == 1);
        capabilities.Dispose();
        ui.Drain();
        Check(changes == 0, "no change after dispose");
    }

    public static async Task ForgottenSubscriptionIsReleased()
    {
        using var core = CoreHost.Create();
        var ui = new ManualDispatcher();
        Subscribe(core, ui);
        Check(core.LiveListeners == 1, "one listener");
        await Eventually("the first value is queued", () => ui.Pending == 1);
        // The listener holds its subscription weakly, so nothing keeps it alive once the UI thread has run the work it queued (which
        // holds the subscription until then); the handle's finaliser then frees the core's task.
        await Eventually("the core drops the listener", () =>
        {
            ui.Drain();
            GC.Collect();
            GC.WaitForPendingFinalizers();
            return core.LiveListeners == 0;
        });
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private static void Subscribe(CoreHost core, IUiDispatcher ui) => core.ObserveCapabilities(ui);

    public static async Task FailingDispatcherDoesNotEndTheSubscription()
    {
        using var core = CoreHost.Create();
        var ui = new ThrowingDispatcher();
        using var capabilities = core.ObserveCapabilities(ui);
        await Eventually("the first value is delivered", () => Volatile.Read(ref ui.Calls) == 1);
        // An exception escaping into the core would end the task and drop the listener. The fixed wait can only make this pass
        // falsely on a very slow runner, never fail falsely; a second delivery cannot be forced from C# while capabilities have no
        // setter (the Rust test a_failing_listener_keeps_its_subscription covers the core side).
        await Task.Delay(100);
        Check(core.LiveListeners == 1, "the core still holds the listener");
    }

    public static async Task ExportDiagnosticsCompletes()
    {
        using var core = CoreHost.Create();
        var report = await core.ExportDiagnosticsAsync();
        Check(report.StartsWith($"Current {core.AppInfo.Version}", StringComparison.Ordinal), "the report starts with the version");
    }

    public static async Task UnrequestedCancelIsItsOwnException()
    {
        // The core's own token is cancelled, the caller's is not.
        using var core = new uniffi.current_ffi.Core();
        try
        {
            await Current.Bindings.CoreCall.RunAsync(token =>
            {
                token.Cancel();
                return core.ExportDiagnostics(token);
            }, CancellationToken.None);
            Check(false, "the call fails");
        }
        catch (CoreCancelledException)
        {
        }
    }

    public static async Task CancelledTokenCancelsTheCall()
    {
        using var core = CoreHost.Create();
        using var cts = new CancellationTokenSource();
        cts.Cancel();
        try
        {
            await core.ExportDiagnosticsAsync(cts.Token);
            Check(false, "the call is cancelled");
        }
        catch (OperationCanceledException error)
        {
            Check(error.CancellationToken == cts.Token, "the caller's token");
        }
    }
}
