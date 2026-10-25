using System.Collections.ObjectModel;
using System.Collections.Specialized;
using Current.Bindings;

namespace Current.Shell.Browsing;

/// <summary>
/// A short list of items (a Home row, a search group, a season's episodes, similar items) as its tiles bind to it: one
/// <see cref="GridSlot"/> per item, kept while the item is shown, so a change to the item (its marks, its resume point) updates the
/// card it is on rather than replacing it, as a library grid does (SlotCollection.SetItem). It follows the core's list, mirrored on
/// the UI thread, until disposed.
/// </summary>
public sealed class TileList : ObservableCollection<GridSlot>, IDisposable
{
    private readonly ObservableCollection<ItemSummary> _source;

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

    private void OnSourceChanged(object? sender, NotifyCollectionChangedEventArgs e)
    {
        switch (e.Action)
        {
            case NotifyCollectionChangedAction.Add when e.NewItems is { } added:
                for (var i = 0; i < added.Count; i++)
                {
                    Insert(e.NewStartingIndex + i, Tile((ItemSummary)added[i]!));
                }
                break;
            case NotifyCollectionChangedAction.Remove when e.OldItems is { } removed:
                for (var i = 0; i < removed.Count; i++)
                {
                    RemoveAt(e.OldStartingIndex);
                }
                break;
            case NotifyCollectionChangedAction.Replace when e.NewItems is { } replaced:
                for (var i = 0; i < replaced.Count; i++)
                {
                    var index = e.NewStartingIndex + i;
                    var item = (ItemSummary)replaced[i]!;
                    if (!this[index].TryUpdate(new Slot.Item(item)))
                    {
                        this[index] = Tile(item);
                    }
                }
                break;
            case NotifyCollectionChangedAction.Move:
                Move(e.OldStartingIndex, e.NewStartingIndex);
                break;
            default:
                // A reset: the list again, keeping the objects of items still shown at the same place.
                var kept = this.ToList();
                Clear();
                for (var i = 0; i < _source.Count; i++)
                {
                    var item = _source[i];
                    Add(i < kept.Count && kept[i].TryUpdate(new Slot.Item(item)) ? kept[i] : Tile(item));
                }
                break;
        }
    }

    public void Dispose() => _source.CollectionChanged -= OnSourceChanged;
}
