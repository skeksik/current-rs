using Microsoft.UI.Dispatching;
using uniffi.bridge_spike;

namespace BridgeSpike.ViewModels;

/// <summary>The async call: run, cancel, run with bad input, and abandon (stop awaiting without cancelling).</summary>
public sealed class OperationViewModel : Observable, IDisposable
{
    private const ulong DelayMs = 3000;

    private readonly Core _core;
    private readonly DispatcherQueueTimer _poll;
    private CancellationTokenSource? _cts;
    private string _input = "hello bridge";
    private string _result = "";
    private bool _isRunning;
    private string _coreState = "";

    internal OperationViewModel(Core core, DispatcherQueue ui)
    {
        _core = core;
        // Polled, not observed: these counters exist only to show what the core still holds.
        _poll = ui.CreateTimer();
        _poll.Interval = TimeSpan.FromMilliseconds(250);
        _poll.Tick += (_, _) => CoreState =
            $"listeners held by the core: {_core.LiveListeners()}; calls running in the core: {_core.InFlight()}";
        _poll.Start();
    }

    public string Input { get => _input; set => Set(ref _input, value); }
    public string Result { get => _result; private set => Set(ref _result, value); }
    public bool IsRunning { get => _isRunning; private set => Set(ref _isRunning, value); }
    public string CoreState { get => _coreState; private set => Set(ref _coreState, value); }

    public async void Run() => await RunAsync(Input);

    public async void RunEmpty() => await RunAsync("");

    public void Cancel() => _cts?.Cancel();

    /// <summary>
    /// What happens when C# code stops awaiting without the CancelToken: the Task is abandoned, the Rust work runs on
    /// for its full delay (the in-flight count shows it).
    /// </summary>
    public async void RunAndAbandon()
    {
        using var stopWaiting = new CancellationTokenSource(TimeSpan.FromMilliseconds(300));
        var token = new CancelToken(); // never cancelled; released by its finaliser
        try
        {
            Result = "abandoning after 300 ms…";
            var r = await _core.SlowReverse(Input, DelayMs, token).WaitAsync(stopWaiting.Token);
            Result = r;
        }
        catch (OperationCanceledException)
        {
            Result = "stopped waiting; the core keeps working until its delay ends";
        }
        catch (Exception e)
        {
            Result = $"unexpected: {e.Message}";
        }
    }

    private async Task RunAsync(string text)
    {
        if (IsRunning) return;
        IsRunning = true;
        _cts = new CancellationTokenSource();
        Result = "running…";
        try
        {
            // Resumes on the UI thread: await captures the window's SynchronizationContext.
            Result = await _core.SlowReverseAsync(text, DelayMs, _cts.Token);
        }
        catch (OperationCanceledException)
        {
            Result = "cancelled";
        }
        catch (BridgeException.InvalidInput e)
        {
            Result = $"BridgeException.InvalidInput: {e.@reason}";
        }
        catch (BridgeException e)
        {
            Result = $"{e.GetType().Name}: {e.Message}";
        }
        finally
        {
            _cts.Dispose();
            _cts = null;
            IsRunning = false;
        }
    }

    public void Dispose()
    {
        _poll.Stop();
        _cts?.Cancel();
    }
}
