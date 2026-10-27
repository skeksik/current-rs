using System.Collections.ObjectModel;
using System.ComponentModel;
using Current.Bindings;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Browsing;

/// <summary>
/// One titled row of tiles that scrolls sideways: a Home row or a search group, as the core shows it (<see cref="RowDisplay"/>: its
/// title, its tiles' shape and where its header goes). Its items are the core's list, mirrored on the UI thread. Public, because
/// x:DataType needs a public type.
/// </summary>
public sealed class TileRow(string key, RowDisplay display, ListSubscription<ItemSummary> items) : IDisposable, INotifyPropertyChanged
{
    public string Key { get; } = key;

    public string Title { get; } = display.Title;

    /// <summary>The library a Latest row shows, whose grid its header opens; null for a row without a page of its own.</summary>
    public ItemRef? Library { get; } = (display.Link?.Target as RowTarget.OpenLibrary)?.Library;

    /// <summary>The view a row across every library opens from its header (<see cref="Pages.ViewArgs"/>); null for none.</summary>
    public BrowseView? View { get; } = (display.Link?.Target as RowTarget.OpenView)?.View;

    /// <summary>The media kind of that view the header opens (a Favourites row's), or null for the whole view.</summary>
    public string? Kind { get; } = (display.Link?.Target as RowTarget.OpenView)?.Kind;

    /// <summary>Whether that view opens with its favourites tick on (a Favourites row's).</summary>
    public bool Favourite { get; } = (display.Link?.Target as RowTarget.OpenView)?.Favourite ?? false;

    /// <summary>Where the header link goes, as the core words it ("Open Movies, newest first"); empty for a row without a link.</summary>
    public string HeaderLinkTip { get; } = display.Link?.Tooltip ?? "";

    public Visibility HeaderLinkVisibility => Library is null && View is null ? Visibility.Collapsed : Visibility.Visible;

    public Visibility HeaderTextVisibility => Library is null && View is null ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>The row's items as its tiles bind to them: a change to an item updates its card (TileList).</summary>
    public TileList Items { get; } = new(items.Items);

    internal ListSubscription<ItemSummary> Subscription { get; } = items;

    /// <summary>Cast cards, wide tiles (16:9) or posters, with or without their titles, as the core shapes the row.</summary>
    public DataTemplate ItemTemplate { get; } = (DataTemplate)Application.Current.Resources[
        display.Cards ? "PersonTileTemplate"
        : display.Wide ? (display.ShowTitles ? "WideTileTemplate" : "WidePictureTemplate")
        : display.ShowTitles ? "PosterTileTemplate" : "PosterPictureTemplate"];

    /// <summary>Whether the tiles wrap into a grid rather than scroll sideways (the core's: a search narrowed to one type).</summary>
    public bool Wraps { get; } = display.Wraps;

    /// <summary>What opening an item shows: its details, or a person's titles.</summary>
    public ItemOpens Opens { get; } = display.Opens;

    /// <summary>Whether its items have the item menu (Play, Mark as played, Favourite); a person has none.</summary>
    public bool ItemMenu { get; } = display.ItemMenu;

    /// <summary>The line under the header once the row is full (the core's note: only the best matches show); empty until then.</summary>
    public string Note => _note is { } note && Items.Count >= note.FullAt ? note.Text : "";

    // Kept in a field, not read from the constructor's parameter later: one parameter both initialising members and captured draws
    // CS9124, an error here.
    private readonly RowNote? _note = display.Note;

    public Visibility NoteVisibility => Note.Length > 0 ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>The note's binding listens here: from then on a row with a note follows its count.</summary>
    public event PropertyChangedEventHandler? PropertyChanged
    {
        add
        {
            if (_propertyChanged is null && _note is not null)
            {
                Items.CollectionChanged += OnItemsChanged;
            }
            _propertyChanged += value;
        }
        remove => _propertyChanged -= value;
    }

    private PropertyChangedEventHandler? _propertyChanged;

    private void OnItemsChanged(object? sender, System.Collections.Specialized.NotifyCollectionChangedEventArgs e)
    {
        _propertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(Note)));
        _propertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(NoteVisibility)));
    }

    public void Dispose()
    {
        Items.CollectionChanged -= OnItemsChanged;
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

    internal HomeSection(string header, ServerRef server, HomeFeed feed, IUiDispatcher ui)
    {
        Header = header;
        Server = server;
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

    /// <summary>The server whose rows these are: a view row's header opens that server's tab.</summary>
    public ServerRef Server { get; }

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
                existing = new TileRow(row.Key, Views.ForHomeRow(row), items);
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
    internal static void AttachOnce(ListViewBase list, Action<ItemSummary> open, bool itemMenu = true)
    {
        if (list.Tag is not "attached")
        {
            list.Tag = "attached";
            ItemActions.Attach(list, open, itemMenu);
        }
    }
}
