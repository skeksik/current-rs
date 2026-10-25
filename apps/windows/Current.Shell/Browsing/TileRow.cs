using System.Collections.ObjectModel;
using System.ComponentModel;
using Current.Bindings;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Browsing;

/// <summary>
/// One titled row of tiles that scrolls sideways: a Home row or a search group. Its items are the core's list, mirrored on the UI
/// thread. Public, because x:DataType needs a public type.
/// </summary>
public sealed class TileRow(string key, string title, ListSubscription<ItemSummary> items, bool wide) : IDisposable
{
    public string Key { get; } = key;

    public string Title { get; } = title;

    /// <summary>The row's items as its tiles bind to them: a change to an item updates its card (TileList).</summary>
    public TileList Items { get; } = new(items.Items);

    internal ListSubscription<ItemSummary> Subscription { get; } = items;

    /// <summary>Wide tiles (16:9) for rows of episodes, posters otherwise.</summary>
    public DataTemplate ItemTemplate { get; } =
        (DataTemplate)Application.Current.Resources[wide ? "WideTileTemplate" : "PosterTileTemplate"];

    public void Dispose()
    {
        Items.Dispose();
        Subscription.Dispose();
    }
}

/// <summary>
/// The tiles of one signed-in server on Home: its HomeFeed's rows in the core's order, each shown once it has items (ADR 0018,
/// section 6), and one message for a failed or offline load. Public for x:DataType.
/// </summary>
public sealed class HomeSection : INotifyPropertyChanged, IDisposable
{
    private readonly HomeFeed _feed;
    private readonly IUiDispatcher _ui;
    private readonly ListSubscription<HomeRow> _rows;
    private readonly ValueSubscription<HomeState> _state;

    internal HomeSection(string header, HomeFeed feed, IUiDispatcher ui)
    {
        Header = header;
        _feed = feed;
        _ui = ui;
        _rows = feed.ObserveRows(ui);
        _rows.Items.CollectionChanged += (_, _) => SyncRows();
        _state = feed.ObserveState(ui);
        _state.PropertyChanged += (_, _) => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(State)));
        SyncRows();
    }

    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>The server's name above its rows when more than one server is signed in; empty otherwise.</summary>
    public string Header { get; }

    public Visibility HeaderVisibility => Header.Length > 0 ? Visibility.Visible : Visibility.Collapsed;

    public ObservableCollection<TileRow> Rows { get; } = [];

    public HomeState State => _state.Value;

    internal HomeFeed Feed => _feed;

    /// <summary>Keeps the row objects of rows that stay, so their lists keep their scroll position and focus.</summary>
    private void SyncRows()
    {
        var wanted = _rows.Items.ToList();
        for (var i = Rows.Count - 1; i >= 0; i--)
        {
            if (!wanted.Any(row => row.Key == Rows[i].Key))
            {
                Rows[i].Dispose();
                Rows.RemoveAt(i);
            }
        }
        var position = 0;
        foreach (var row in wanted)
        {
            var existing = Rows.FirstOrDefault(shown => shown.Key == row.Key);
            if (existing is null)
            {
                if (_feed.ObserveRow(row.Key, _ui) is not { } items)
                {
                    continue;
                }
                existing = new TileRow(row.Key, BrowseText.RowTitle(row), items, row.Kind != HomeRowKind.Latest);
                Rows.Insert(position, existing);
            }
            else if (Rows.IndexOf(existing) != position)
            {
                Rows.Move(Rows.IndexOf(existing), position);
            }
            position++;
        }
    }

    public void Dispose()
    {
        _rows.Dispose();
        _state.Dispose();
        foreach (var row in Rows)
        {
            row.Dispose();
        }
        Rows.Clear();
    }

    /// <summary>Gives a row's list the open action and context menu once, when it is first shown.</summary>
    internal static void AttachOnce(ListViewBase list, Action<ItemSummary> open)
    {
        if (list.Tag is not "attached")
        {
            list.Tag = "attached";
            ItemActions.Attach(list, open);
        }
    }
}
