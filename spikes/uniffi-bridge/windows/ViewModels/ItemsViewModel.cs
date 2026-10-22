using System.Collections.ObjectModel;
using System.Diagnostics;
using Microsoft.UI.Dispatching;
using uniffi.bridge_spike;

namespace BridgeSpike.ViewModels;

public sealed record ItemRow(ulong Id, string Title);

/// <summary>
/// Mirrors the core's list in an ObservableCollection by applying its diffs in order on the UI thread, so the
/// ListView animates single changes instead of rebuilding.
/// </summary>
public sealed class ItemsViewModel : Observable, IDisposable
{
    private readonly Core _core;
    private readonly DispatcherQueue _ui;
    private TaskHandle? _subscription;
    private int _batches;

    internal ItemsViewModel(Core core, DispatcherQueue ui)
    {
        _core = core;
        _ui = ui;
        _subscription = _core.SubscribeItems(new Forwarder(this, ui));
    }

    public ObservableCollection<ItemRow> Rows { get; } = new();

    public int Batches { get => _batches; private set => Set(ref _batches, value); }

    public void Step() => _core.FeedStep();
    public void StartFeed() => _core.StartFeed(300);
    public void StopFeed() => _core.StopFeed();

    public void Dispose()
    {
        _subscription?.Cancel();
        _subscription?.Dispose();
        _subscription = null;
    }

    private void Apply(ItemDiff[] diffs)
    {
        Debug.Assert(_ui.HasThreadAccess, "applied off the UI thread");
        foreach (var diff in diffs)
        {
            switch (diff)
            {
                case ItemDiff.Append d:
                    foreach (var item in d.Values) Rows.Add(Row(item));
                    break;
                case ItemDiff.Insert d:
                    Rows.Insert(checked((int)d.Index), Row(d.Value));
                    break;
                case ItemDiff.Set d:
                    Rows[checked((int)d.Index)] = Row(d.Value);
                    break;
                case ItemDiff.Remove d:
                    Rows.RemoveAt(checked((int)d.Index));
                    break;
                case ItemDiff.Reset d:
                    Rows.Clear();
                    foreach (var item in d.Values) Rows.Add(Row(item));
                    break;
            }
        }
        Batches++;
    }

    private static ItemRow Row(Item item) => new(item.Id, item.Title);

    private sealed class Forwarder(ItemsViewModel owner, DispatcherQueue ui) : ItemsListener
    {
        private readonly WeakReference<ItemsViewModel> _owner = new(owner);

        public void OnDiffs(ItemDiff[] diffs)
        {
            try
            {
                ui.TryEnqueue(() =>
                {
                    if (_owner.TryGetTarget(out var vm)) vm.Apply(diffs);
                });
            }
            catch (Exception e)
            {
                Debug.WriteLine($"items listener failed: {e}");
            }
        }
    }
}
