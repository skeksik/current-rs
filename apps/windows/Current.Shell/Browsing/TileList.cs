using System.Collections.ObjectModel;
using System.Collections.Specialized;
using Current.Bindings;
using Microsoft.UI.Dispatching;

namespace Current.Shell.Browsing;

/// <summary>
/// A short list of items (a Home row, a search group, a season's episodes, similar items) as its tiles bind to it: one
/// <see cref="GridSlot"/> per item, kept while the item is in the list, so a change to the item (its marks, its resume point) updates
/// the card it is on, and an item that moved keeps its card. It follows the core's list, mirrored on the UI thread, until disposed.
/// </summary>
/// <remarks>
/// The core refreshes a list place by place (<c>sync_list</c>: set, append, remove). When the server answers in another order (a
/// search's ranking, a new item at the front of a row), every place gets another item, and replacing each card built every tile
/// again without its picture: the whole row went blank for a frame (the owner's run, 2026-10-25). So a batch of changes is applied
/// once it is complete, by item: items still there keep their cards, new ones get new cards, gone ones lose theirs.
/// </remarks>
public sealed class TileList : ObservableCollection<GridSlot>, IDisposable
{
    private readonly ObservableCollection<ItemSummary> _source;
    private readonly DispatcherQueue? _queue = DispatcherQueue.GetForCurrentThread();
    private bool _pending;
    private bool _disposed;

    public TileList(ObservableCollection<ItemSummary> source)
    {
        _source = source;
        foreach (var item in source)
        {
            Add(Tile(item));
        }
        source.CollectionChanged += OnSourceChanged;
    }

    private static GridSlot Tile(ItemSummary item) => new(new Slot.Item(item));

    private static bool Shows(GridSlot tile, ItemSummary item) => tile.Summary?.Item == item.Item;

    /// <summary>A batch of the core's diffs arrives as several changes in one go: follow it once, when the batch is in.</summary>
    private void OnSourceChanged(object? sender, NotifyCollectionChangedEventArgs e)
    {
        if (_pending)
        {
            return;
        }
        _pending = true;
        if (_queue is null || !_queue.TryEnqueue(DispatcherQueuePriority.High, Follow))
        {
            Follow();
        }
    }

    /// <summary>Makes the tiles match the source in its order, keeping the tile of every item still in it.</summary>
    private void Follow()
    {
        _pending = false;
        if (_disposed)
        {
            return;
        }
        var target = _source.ToList();
        for (var i = 0; i < target.Count; i++)
        {
            var item = target[i];
            if (i < Count && Shows(this[i], item))
            {
                this[i].TryUpdate(new Slot.Item(item));
                continue;
            }
            var later = -1;
            for (var j = i + 1; j < Count; j++)
            {
                if (Shows(this[j], item))
                {
                    later = j;
                    break;
                }
            }
            if (later >= 0)
            {
                Move(later, i);
                this[i].TryUpdate(new Slot.Item(item));
            }
            else if (i < Count && !target.Skip(i + 1).Any(next => Shows(this[i], next)))
            {
                // The tile here shows an item that left the list: it makes way for the new one.
                this[i] = Tile(item);
            }
            else
            {
                Insert(i, Tile(item));
            }
        }
        while (Count > target.Count)
        {
            RemoveAt(Count - 1);
        }
    }

    public void Dispose()
    {
        _disposed = true;
        _source.CollectionChanged -= OnSourceChanged;
    }
}
