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
public sealed class DiffObservableCollection<T> : ObservableCollection<T>
{
    /// <summary>Applies a batch in order.</summary>
    /// <exception cref="ArgumentOutOfRangeException">A diff's index does not exist: the mirror is out of step with the core.</exception>
    public void Apply(IEnumerable<ListDiff<T>> batch)
    {
        foreach (var diff in batch)
        {
            Apply(diff);
        }
    }

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
}
