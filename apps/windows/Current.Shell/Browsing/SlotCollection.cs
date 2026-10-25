using System.Collections.Specialized;
using System.ComponentModel;
using Current.Bindings;
using Microsoft.UI.Xaml.Data;

namespace Current.Shell.Browsing;

/// <summary>
/// The collection a library grid binds to (ADR 0018, section 7): the core's slots, with WinUI's view ranges forwarded to the core so it
/// loads the pages the grid shows. Every item is a <see cref="GridSlot"/> around the core's <see cref="Slot"/> record, never null:
/// the tile template shows a placeholder for <see cref="Slot.Loading"/> and a retry glyph for <see cref="Slot.Failed"/>.
/// </summary>
/// <remarks>
/// <para>It lives here, not in Current.Bindings, because IItemsRangeInfo is a Windows App SDK type (Microsoft.UI.Xaml.Data), and the
/// bindings' console test runner must stay free of the Windows App SDK runtime. Not generic and partial, so the WinRT projection can
/// expose it to XAML.</para>
/// <para>"ListView and GridView data virtualization" (learn.microsoft.com, ms.date 2026-03-16, read 2026-10-25): random-access data
/// virtualization needs IList, INotifyCollectionChanged and "(Optionally) IItemsRangeInfo", which "provides information about which
/// items the control is actively using. The items control calls this method whenever its view is changing"; asked for an item it does
/// not have, a source may "return either `null` or a placeholder item". We return a placeholder item: a template then always has a
/// slot to bind to. That WinUI 3 fails on null placeholders is not verified.</para>
/// <para>IItemsRangeInfo (learn.microsoft.com, updated 2026-07-28, read 2026-10-25): <c>public interface IItemsRangeInfo :
/// System.IDisposable</c>, with <c>RangesChanged(ItemIndexRange visibleRange, IReadOnlyList&lt;ItemIndexRange&gt; trackedItems)</c>;
/// "IItemsRangeInfo is designed to be used by only a single instance of an items control", so open one per grid.</para>
/// </remarks>
public sealed partial class SlotCollection : DiffObservableCollection<GridSlot>, IItemsRangeInfo
{
    private PagedCollection<GridSlot>? _paged;

    private SlotCollection() { }

    /// <summary>The slots of <paramref name="items"/>, mirrored on the UI thread. Dispose it when the grid goes away.</summary>
    public static SlotCollection Open(PagedItems items, IUiDispatcher ui)
    {
        var collection = new SlotCollection();
        collection._paged = items.Collect(ui, slot => new GridSlot(slot), collection);
        return collection;
    }

    /// <summary>The mirror's subscription: its recoveries and whether it faulted.</summary>
    public ListSubscription<GridSlot>? Subscription => _paged?.Subscription;

    /// <summary>Called by the grid on the UI thread whenever its view changes: only the visible range goes to the core, which keeps a
    /// page either side itself.</summary>
    public void RangesChanged(ItemIndexRange visibleRange, IReadOnlyList<ItemIndexRange> trackedItems)
    {
        if (visibleRange.Length == 0)
        {
            return;
        }
        _paged?.SetVisibleRange(visibleRange.FirstIndex, visibleRange.LastIndex);
    }

    /// <summary>
    /// A slot set again to the same item (its marks or resume point changed, a refresh read it again) updates the place's object,
    /// which its tile follows: the grid sees no replacement, so the card stays and only what changed on it is drawn again (the
    /// owner's run, 2026-10-25). Any other set replaces the place.
    /// </summary>
    protected override void SetItem(int index, GridSlot item)
    {
        if (!this[index].TryUpdate(item.Slot))
        {
            base.SetItem(index, item);
        }
    }

    /// <summary>Stops the mirror; the grid may also call it (IItemsRangeInfo is IClosable). Its PagedItems stays with its owner.</summary>
    public void Dispose()
    {
        _paged?.Dispose();
        _paged = null;
    }
}

/// <summary>
/// One place in the grid. A class, so each place is its own object: the core's slot records are compared by value, and every
/// <see cref="Slot.Loading"/> equals every other, so a list control that finds an item by IndexOf (to scroll to it, to keep focus on
/// it) would find the first placeholder instead. ToString is what Narrator falls back to: the tile's name. While it shows the same
/// item its object stays and its slot changes (<see cref="TryUpdate"/>), which the tile follows through PropertyChanged.
/// </summary>
public sealed class GridSlot(Slot slot) : INotifyPropertyChanged
{
    public Slot Slot { get; private set; } = slot;

    public ItemSummary? Summary => (Slot as Slot.Item)?.Summary;

    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>Takes <paramref name="newer"/> if it is the same item as now; false (nothing changed) otherwise. UI thread only.</summary>
    public bool TryUpdate(Slot newer)
    {
        if (Slot is not Slot.Item now || newer is not Slot.Item next || now.Summary.Item != next.Summary.Item)
        {
            return false;
        }
        if (now.Summary != next.Summary)
        {
            Slot = newer;
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(Slot)));
        }
        return true;
    }

    public override string ToString() => Slot switch
    {
        Slot.Item item => BrowseText.TileName(item.Summary),
        Slot.Failed => BrowseText.FailedTileName,
        _ => BrowseText.LoadingTileName,
    };
}
