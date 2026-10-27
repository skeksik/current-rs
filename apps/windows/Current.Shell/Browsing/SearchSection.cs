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

    /// <param name="link">The view a row's header opens with the row's kind (the Favourites tab's rows); null for a search.</param>
    /// <param name="filter">A search's filters, for the line under a full group and whether one type wraps; null for the Favourites
    /// tab.</param>
    internal SearchSection(string header, SearchResults results, IUiDispatcher ui, string? link = null, SearchFilter? filter = null)
    {
        Header = header;
        _link = link;
        _filter = filter;
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

    private readonly string? _link;
    private readonly SearchFilter? _filter;

    /// <summary>The search page's View (owner, 2026-10-27, as a library grid's): wide pictures, and whether tiles show their titles.</summary>
    private bool _wide;
    private bool _titles = true;

    /// <summary>
    /// Shows every row with the View's tiles. A row keeps its list but not its cards, so its template changes: the rows are made again
    /// from the core's groups, without searching again.
    /// </summary>
    internal void SetView(bool wide, bool titles)
    {
        if (wide == _wide && titles == _titles)
        {
            return;
        }
        (_wide, _titles) = (wide, titles);
        foreach (var row in Rows)
        {
            row.Dispose();
        }
        Rows.Clear();
        SyncGroups();
    }

    /// <summary>
    /// The tiles of a row of <paramref name="kind"/>: people as cast cards, whatever the view; episodes and videos wide, as before;
    /// the rest posters or wide as the View says, with or without their titles.
    /// </summary>
    private string TemplateFor(string kind) =>
        kind == "person" ? "PersonTileTemplate"
        : (_wide || kind is "episode" or "video") ? (_titles ? "WideTileTemplate" : "WidePictureTemplate")
        : _titles ? "PosterTileTemplate" : "PosterPictureTemplate";

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
                existing = new TileRow(kind, BrowseText.KindGroup(kind), items, kind is "episode" or "video")
                {
                    View = _link,
                    Kind = _link is null ? null : kind,
                    HeaderLinkTip = _link is null ? "" : BrowseText.ViewLinkTip(_link, kind),
                    ItemTemplate = (DataTemplate)Application.Current.Resources[TemplateFor(kind)],
                    Wraps = _filter?.Kind is not null,
                    NoteFor = _filter is { } filter ? count => filter.Note(count) : null,
                };
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
