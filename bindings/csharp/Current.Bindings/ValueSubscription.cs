using System.ComponentModel;
using System.Diagnostics;

namespace Current.Bindings;

/// <summary>
/// A value the core owns, mirrored on the UI thread (ADR 0004). <see cref="Value"/> changes, and <see cref="PropertyChanged"/> fires,
/// only on the UI thread. The owner (a page or its view model) disposes it when the page unloads; the finaliser of the core handle is
/// only the backstop.
/// </summary>
public sealed class ValueSubscription<T> : INotifyPropertyChanged, IDisposable
{
    private uniffi.current_ffi.TaskHandle? _handle;

    internal ValueSubscription(T initial)
    {
        Value = initial;
    }

    public T Value { get; private set; }

    public event PropertyChangedEventHandler? PropertyChanged;

    public bool IsDisposed { get; private set; }

    internal void Attach(uniffi.current_ffi.TaskHandle handle) => _handle = handle;

    /// <summary>Runs on the UI thread. A delivery that arrives after <see cref="Dispose"/> is dropped.</summary>
    internal void Set(T value)
    {
        if (IsDisposed)
        {
            return;
        }
        Value = value;
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(Value)));
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
/// The listener the core holds for a value. It keeps only a weak reference to its subscription and never the core, so the core cannot
/// keep a page alive. It runs on a core worker thread, maps the value there, hops to the UI thread and returns. It catches every
/// exception: one escaping into the core would end the subscription (ADR 0004).
/// </summary>
internal class ValueForwarder<TRaw, T>(ValueSubscription<T> target, IUiDispatcher ui, Func<TRaw, T> map)
{
    private readonly WeakReference<ValueSubscription<T>> _target = new(target);

    protected void Forward(TRaw raw)
    {
        try
        {
            if (!_target.TryGetTarget(out var target) || target.IsDisposed)
            {
                return;
            }
            var value = map(raw);
            ui.TryEnqueue(() =>
            {
                try
                {
                    target.Set(value);
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
        Trace.TraceError($"Current: a core listener failed: {error}");
}

internal sealed class AppThemeForwarder(ValueSubscription<AppTheme> target, IUiDispatcher ui)
    : ValueForwarder<uniffi.current_ffi.AppTheme, AppTheme>(target, ui, Map.From),
      uniffi.current_ffi.AppThemeListener
{
    public void OnValue(uniffi.current_ffi.AppTheme value) => Forward(value);
}

internal sealed class CapabilitiesForwarder(ValueSubscription<Capabilities> target, IUiDispatcher ui)
    : ValueForwarder<uniffi.current_ffi.CapabilitiesSnapshot, Capabilities>(target, ui, Map.From),
      uniffi.current_ffi.CapabilitiesListener
{
    public void OnValue(uniffi.current_ffi.CapabilitiesSnapshot value) => Forward(value);
}
