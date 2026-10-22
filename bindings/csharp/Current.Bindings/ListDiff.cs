using System.Collections.ObjectModel;

namespace Current.Bindings;

/// <summary>
/// One change to a list the core owns (ADR 0004). Indices refer to the list after the diffs before it in the same batch. Each
/// exported list maps its generated diff type to this one.
/// </summary>
public abstract record ListDiff<T>
{
    private ListDiff() { }

    public sealed record Append(IReadOnlyList<T> Values) : ListDiff<T>;
    public sealed record Insert(int Index, T Value) : ListDiff<T>;
    public sealed record Set(int Index, T Value) : ListDiff<T>;
    public sealed record Remove(int Index) : ListDiff<T>;
    public sealed record Reset(IReadOnlyList<T> Values) : ListDiff<T>;
}

/// <summary>
/// An <see cref="ObservableCollection{T}"/> that applies the core's diffs one by one, so a list view animates single changes instead
/// of redrawing. Use it on the UI thread only.
/// </summary>
/// <remarks>
/// An out-of-range diff means the mirror is out of step with the core, which never sends one on its own. The owner should then
/// re-subscribe: a new subscription starts with a fresh Reset (the core sends no unsolicited Reset for this).
/// </remarks>
public sealed class DiffObservableCollection<T> : ObservableCollection<T>
{
    /// <summary>Applies a batch in order, all or nothing: the batch is checked against a copy first.</summary>
    /// <exception cref="ArgumentOutOfRangeException">A diff's index does not exist; the collection is unchanged.</exception>
    public void Apply(IReadOnlyList<ListDiff<T>> batch)
    {
        var copy = new List<T>(Items);
        foreach (var diff in batch)
        {
            ApplyTo(copy, diff);
        }
        foreach (var diff in batch)
        {
            Apply(diff);
        }
    }

    /// <summary>Applies one diff.</summary>
    /// <exception cref="ArgumentOutOfRangeException">Its index does not exist; the collection is unchanged.</exception>
    public void Apply(ListDiff<T> diff)
    {
        switch (diff)
        {
            case ListDiff<T>.Append append:
                foreach (var value in append.Values)
                {
                    Add(value);
                }
                break;
            case ListDiff<T>.Insert insert:
                ArgumentOutOfRangeException.ThrowIfGreaterThan((uint)insert.Index, (uint)Count, "index");
                InsertItem(insert.Index, insert.Value);
                break;
            case ListDiff<T>.Set set:
                ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual((uint)set.Index, (uint)Count, "index");
                SetItem(set.Index, set.Value);
                break;
            case ListDiff<T>.Remove remove:
                ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual((uint)remove.Index, (uint)Count, "index");
                RemoveItem(remove.Index);
                break;
            case ListDiff<T>.Reset reset:
                // One Reset notification, not one per item.
                Items.Clear();
                foreach (var value in reset.Values)
                {
                    Items.Add(value);
                }
                OnPropertyChanged(new System.ComponentModel.PropertyChangedEventArgs(nameof(Count)));
                OnPropertyChanged(new System.ComponentModel.PropertyChangedEventArgs("Item[]"));
                OnCollectionChanged(new System.Collections.Specialized.NotifyCollectionChangedEventArgs(
                    System.Collections.Specialized.NotifyCollectionChangedAction.Reset));
                break;
            default:
                throw new ArgumentException($"unknown diff {diff.GetType().Name}", nameof(diff));
        }
    }

    /// <summary>Applies a diff to a plain list, with the same checks and no notifications.</summary>
    private static void ApplyTo(List<T> list, ListDiff<T> diff)
    {
        switch (diff)
        {
            case ListDiff<T>.Append append:
                list.AddRange(append.Values);
                break;
            case ListDiff<T>.Insert insert:
                ArgumentOutOfRangeException.ThrowIfGreaterThan((uint)insert.Index, (uint)list.Count, "index");
                list.Insert(insert.Index, insert.Value);
                break;
            case ListDiff<T>.Set set:
                ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual((uint)set.Index, (uint)list.Count, "index");
                list[set.Index] = set.Value;
                break;
            case ListDiff<T>.Remove remove:
                ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual((uint)remove.Index, (uint)list.Count, "index");
                list.RemoveAt(remove.Index);
                break;
            case ListDiff<T>.Reset reset:
                list.Clear();
                list.AddRange(reset.Values);
                break;
            default:
                throw new ArgumentException($"unknown diff {diff.GetType().Name}", nameof(diff));
        }
    }
}
