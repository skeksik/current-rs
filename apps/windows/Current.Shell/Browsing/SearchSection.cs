using System.Collections.ObjectModel;
using System.ComponentModel;
using Current.Bindings;
using Microsoft.UI.Xaml;

namespace Current.Shell.Browsing;

/// <summary>
/// One server's results for a search: a row per media kind with matches, in the core's order, each the best matches the server
/// offers (ADR 0018: search is not paged). Public for x:DataType.
/// </summary>
public sealed class SearchSection : INotifyPropertyChanged, IDisposable
{
    private readonly SearchResults _results;
    private readonly IUiDispatcher _ui;
    private readonly ListSubscription<string> _groups;
    private readonly ValueSubscription<LoadState> _state;

    internal SearchSection(string header, SearchResults results, IUiDispatcher ui)
    {
        Header = header;
        _results = results;
        _ui = ui;
        _groups = results.ObserveGroups(ui);
        _groups.Items.CollectionChanged += (_, _) => SyncGroups();
        _state = results.ObserveState(ui);
        _state.PropertyChanged += (_, _) => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(State)));
        SyncGroups();
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    public string Header { get; }

    public Visibility HeaderVisibility => Header.Length > 0 ? Visibility.Visible : Visibility.Collapsed;

    public ObservableCollection<TileRow> Rows { get; } = [];

    public LoadState State => _state.Value;

    private void SyncGroups()
    {
        var wanted = _groups.Items.ToList();
        for (var i = Rows.Count - 1; i >= 0; i--)
        {
            if (!wanted.Contains(Rows[i].Key))
            {
                Rows[i].Dispose();
                Rows.RemoveAt(i);
            }
        }
        var position = 0;
        foreach (var kind in wanted)
        {
            var existing = Rows.FirstOrDefault(row => row.Key == kind);
            if (existing is null)
            {
                if (_results.ObserveGroup(kind, _ui) is not { } items)
                {
                    continue;
                }
                existing = new TileRow(kind, BrowseText.KindGroup(kind), items, kind is "episode" or "video");
                Rows.Insert(position, existing);
            }
            else if (Rows.IndexOf(existing) != position)
            {
                Rows.Move(Rows.IndexOf(existing), position);
            }
            position++;
        }
    }

    /// <summary>Searches again, keeping the results until the answers arrive.</summary>
    internal void Refresh() => _results.Refresh();

    public void Dispose()
    {
        _groups.Dispose();
        _state.Dispose();
        foreach (var row in Rows)
        {
            row.Dispose();
        }
        Rows.Clear();
        _results.Dispose();
    }
}
