using System.ComponentModel;
using System.Globalization;
using Current.Bindings;
using Current.Shell.Browsing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Current.Shell.Pages;

/// <summary>
/// An item's detail page (ADR 0018): backdrop, poster, title, the facts the server gives (year, runtime, ratings, genres), the
/// overview, the played and favourite marks, and the cast; a series adds its seasons and the chosen season's episodes; every item
/// its similar items. Everything is the core's DetailsView, kept for the page's history entry (ServerBrowse.Details): Back binds a
/// new page to the view it had, with the season chosen in it, and scrolls to where the page was. Play, Resume and "Play from the
/// beginning" come first among the actions, from the core's PlayTarget (ADR 0019, task 5); the player opens over the page.
/// </summary>
public sealed partial class DetailsPage : Page
{
    /// <summary>The backdrop's height at most, in view pixels: provisional, as the tile sizes are (App.xaml).</summary>
    private const double MaxBackdropHeight = 360;

    private readonly List<IDisposable> _subscriptions = [];
    private readonly TileImage _backdrop = new(App.Core!);
    private readonly TileImage _poster = new(App.Core!);
    private readonly TileImage _posterPlaceholder = new(App.Core!);
    private DetailsView? _view;
    private KeptDetails? _kept;

    /// <summary>Reached by Back or Forward: scroll to where the page and its rows were once they are laid out.</summary>
    private bool _restore;
    private ValueSubscription<ItemDetails?>? _details;
    private ValueSubscription<PlayTarget?>? _play;

    /// <summary>What the Play buttons show now: rebuilt only when it changes, so focus on them stays.</summary>
    private (ItemRef Item, TimeSpan Start, bool Resumes)? _shownPlay;
    private ValueSubscription<LoadState>? _state;
    private ListSubscription<ItemSummary>? _seasons;
    private ImageRef? _shownBackdrop;
    private ImageRef? _shownPoster;
    private bool _showingSeasons;

    public DetailsPage()
    {
        InitializeComponent();
        void Open(ItemSummary item) => Frame.Navigate(typeof(DetailsPage), new DetailsArgs(item.Item));
        ItemActions.Attach(Episodes, Open);
        ItemActions.Attach(Similar, Open);
        BackdropFrame.SizeChanged += (_, _) => SizeBackdrop();
        // Released once the page has left the window: during its exit transition it still shows its lists.
        Loaded += (_, _) => _wasLoaded = true;
        Unloaded += (_, _) => Release();
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        if (e.Parameter is not DetailsArgs args || App.Browsing?.Find(args.Item.Server) is not { } server)
        {
            TitleText.Text = "This item isn't available.";
            return;
        }
        var ui = new DispatcherQueueUi(DispatcherQueue);
        _kept = server.Details(args, args.Item);
        _view = _kept.View!;
        _restore = e.NavigationMode is NavigationMode.Back or NavigationMode.Forward;
        _details = _view.ObserveDetails(ui);
        _details.PropertyChanged += OnDetailsChanged;
        _state = _view.ObserveState(ui);
        _state.PropertyChanged += OnStateChanged;
        _play = _view.ObservePlay(ui);
        _play.PropertyChanged += (_, _) => ShowPlay();
        _subscriptions.Add(_play);
        _seasons = _view.ObserveSeasons(ui);
        _seasons.Items.CollectionChanged += (_, _) => ShowSeasons();
        var episodes = _view.ObserveEpisodes(ui);
        var similar = _view.ObserveSimilar(ui);
        similar.Items.CollectionChanged += (_, _) =>
            SimilarPart.Visibility = similar.Items.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
        _subscriptions.AddRange([_details, _state, _seasons, episodes, similar]);
        _episodeTiles = new TileList(episodes.Items);
        _similarTiles = new TileList(similar.Items);
        Episodes.ItemsSource = _episodeTiles;
        Similar.ItemsSource = _similarTiles;
        ShowDetails();
        ShowState();
        ShowPlay();
        if (_restore)
        {
            _ = ScrollWhenLaidOutAsync(Scroller, _kept.Offset, horizontal: false);
            _ = ScrollWhenLaidOutAsync(SeasonsScroller, _kept.SeasonsOffset, horizontal: true);
            // The rows' own scroll viewers exist once their templates are applied.
            People.Loaded += (_, _) => RestoreRow(People, _kept?.PeopleOffset ?? 0);
            Similar.Loaded += (_, _) => RestoreRow(Similar, _kept?.SimilarOffset ?? 0);
            RestoreRow(People, _kept.PeopleOffset);
            RestoreRow(Similar, _kept.SimilarOffset);
        }
    }

    private void RestoreRow(ListViewBase row, double offset)
    {
        if (_restore && ScrollerOf(row) is { } scroller)
        {
            _ = ScrollWhenLaidOutAsync(scroller, offset, horizontal: true);
        }
    }

    /// <summary>A list's own ScrollViewer, the template part it scrolls with; null before its template is applied.</summary>
    private static ScrollViewer? ScrollerOf(DependencyObject parent)
    {
        for (var i = 0; i < Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChildrenCount(parent); i++)
        {
            var child = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChild(parent, i);
            if (child is ScrollViewer scroller)
            {
                return scroller;
            }
            if (ScrollerOf(child) is { } found)
            {
                return found;
            }
        }
        return null;
    }

    /// <summary>Before the exit transition, while the page is still in place: where it was scrolled to.</summary>
    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        if (!_wasLoaded)
        {
            // Left before it ever loaded (navigated past at once): no Unloaded will come.
            DispatcherQueue.TryEnqueue(Release);
        }
        if (_kept is not null)
        {
            _kept.Offset = Scroller.VerticalOffset;
            _kept.SeasonsOffset = SeasonsScroller.HorizontalOffset;
            _kept.PeopleOffset = ScrollerOf(People)?.HorizontalOffset ?? 0;
            _kept.SimilarOffset = ScrollerOf(Similar)?.HorizontalOffset ?? 0;
        }
    }

    /// <summary>
    /// Scrolls <paramref name="scroller"/> to <paramref name="offset"/> once its content reaches it: the lists arrive and lay out a
    /// moment after the page is shown, so it tries again each frame, for at most a second. Without animation, as a return to a place
    /// is. Nothing to do for an offset of 0.
    /// </summary>
    private async Task ScrollWhenLaidOutAsync(ScrollViewer scroller, double offset, bool horizontal)
    {
        if (offset <= 0)
        {
            return;
        }
        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(1);
        while (_view is not null && DateTime.UtcNow < deadline)
        {
            var reach = horizontal ? scroller.ScrollableWidth : scroller.ScrollableHeight;
            if (reach >= offset - 0.5)
            {
                if (horizontal)
                {
                    scroller.ChangeView(offset, null, null, true);
                }
                else
                {
                    scroller.ChangeView(null, offset, null, true);
                }
                return;
            }
            await Task.Delay(16);
        }
    }

    /// <summary>The episodes and similar items as their tiles bind to them (TileList).</summary>
    private TileList? _episodeTiles;
    private TileList? _similarTiles;

    /// <summary>The page reached the window: its lists go on Unloaded. One navigated from before it loaded has no Unloaded.</summary>
    private bool _wasLoaded;

    private void Release()
    {
        _episodeTiles?.Dispose();
        _similarTiles?.Dispose();
        _backdrop.Recycle();
        _poster.Recycle();
        _posterPlaceholder.Recycle();
        Episodes.ItemsSource = null;
        Similar.ItemsSource = null;
        People.ItemsSource = null;
        foreach (var subscription in _subscriptions)
        {
            subscription.Dispose();
        }
        _subscriptions.Clear();
        // The view stays with its history entry (ServerBrowse.Details); it goes when the entry leaves the history.
        _view = null;
    }

    private void OnDetailsChanged(object? sender, PropertyChangedEventArgs e) => ShowDetails();

    private void OnStateChanged(object? sender, PropertyChangedEventArgs e) => ShowState();

    private void ShowState()
    {
        if (_state?.Value is not { } state)
        {
            return;
        }
        LoadingRing.IsActive = state.Loading && _details?.Value is null;
        if (state.Failed is { } failed)
        {
            var title = _details?.Value is null ? "Couldn't load this item." : "Couldn't refresh this item.";
            var message = BrowseText.Problem(failed);
            if (!StateInfo.IsOpen || StateInfo.Title != title || StateInfo.Message != message)
            {
                StateInfo.IsOpen = false;
                StateInfo.Severity = InfoBarSeverity.Error;
                StateInfo.Title = title;
                StateInfo.Message = message;
                StateInfo.IsOpen = true;
            }
        }
        else
        {
            StateInfo.IsOpen = false;
        }
    }

    private void ShowDetails()
    {
        if (_details?.Value is not { } details)
        {
            Actions.Visibility = Visibility.Collapsed;
            return;
        }
        var item = details.Summary;
        Actions.Visibility = Visibility.Visible;
        TitleText.Text = item.Kind == "episode" ? EpisodeTitle(item) : item.Name;
        FactsText.Text = Facts(details);
        GenresText.Text = string.Join(", ", details.Genres);
        GenresText.Visibility = details.Genres.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
        TaglineText.Text = details.Taglines.FirstOrDefault() ?? "";
        TaglineText.Visibility = TaglineText.Text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        OverviewText.Text = details.Overview ?? "";
        PlayedButton.IsChecked = item.User.Played;
        FavouriteButton.IsChecked = item.User.Favourite;
        FavouriteGlyph.Glyph = item.User.Favourite ? "" : "";
        // The same people again (a played or favourite change, a refresh): the cards stay, with their photos.
        if (People.ItemsSource is not IReadOnlyList<Person> shown || !shown.SequenceEqual(details.People))
        {
            People.ItemsSource = details.People;
        }
        PeoplePart.Visibility = details.People.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
        SeriesPart.Visibility = item.Kind == "series" ? Visibility.Visible : Visibility.Collapsed;
        ShowPictures(details);
    }

    /// <summary>
    /// The Play buttons from the core's PlayTarget (what plays and from where are its rule): "Play" or "Resume" first, in the accent
    /// style as the page's main action (no Microsoft guideline found for that: the Buttons page, learn.microsoft.com, ms.date
    /// 2026-10-13, read 2026-10-25, says nothing on accent buttons), and "Play from the beginning" beside "Resume". On a series' page
    /// the buttons name the episode they play; Narrator hears its name too.
    /// </summary>
    private void ShowPlay()
    {
        var target = _play?.Value;
        var shown = target is null ? ((ItemRef, TimeSpan, bool)?)null : (target.Item.Item, target.Start, target.Resumes);
        if (shown == _shownPlay)
        {
            return;
        }
        _shownPlay = shown;
        PlayActions.Children.Clear();
        if (target is null)
        {
            return;
        }
        var number = _details?.Value?.Summary.Kind == "series" ? BrowseText.EpisodeNumber(target.Item) : "";
        var verb = target.Resumes ? "Resume" : "Play";
        var label = number.Length > 0 ? $"{verb} {number}" : verb;
        var main = new Button
        {
            Style = (Style)Application.Current.Resources["AccentButtonStyle"],
            Content = IconLabel("", label),
        };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(main, number.Length > 0 ? $"{label} {target.Item.Name}" : label);
        main.Click += (_, _) => App.Play(target, fromStart: false);
        PlayActions.Children.Add(main);
        if (target.Resumes)
        {
            var again = new Button { Content = "Play from the beginning" };
            again.Click += (_, _) => App.Play(target, fromStart: true);
            PlayActions.Children.Add(again);
        }
    }

    /// <summary>A glyph and a label, as the Played and Favourite buttons show theirs (DetailsPage.xaml). E768: "Play" ("Segoe Fluent
    /// Icons font", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-25).</summary>
    private static StackPanel IconLabel(string glyph, string label)
    {
        var content = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8 };
        content.Children.Add(new FontIcon { Glyph = glyph, FontSize = 16 });
        content.Children.Add(new TextBlock { Text = label });
        return content;
    }

    /// <summary>"Series name: S1:E2 Episode name" on an episode's own page.</summary>
    private static string EpisodeTitle(ItemSummary item)
    {
        var number = BrowseText.EpisodeNumber(item);
        var name = number.Length > 0 ? $"{number} {item.Name}" : item.Name;
        return item.Episode?.SeriesName is { Length: > 0 } series ? $"{series}: {name}" : name;
    }

    /// <summary>The year (or a series' run), runtime, age rating and community rating, as the server gave them.</summary>
    private static string Facts(ItemDetails details)
    {
        var facts = new List<string>();
        var item = details.Summary;
        // A series' run as the core reads it from its dates and status (ItemDetails.Run); only the formatting is here.
        if (details.Run is { } run)
        {
            facts.Add(run.EndYear is { } endYear ? $"{run.StartYear}\u2013{endYear}"
                : run.Continuing ? $"{run.StartYear}\u2013"
                : run.StartYear.ToString(CultureInfo.CurrentCulture));
        }
        else if (item.Year is { } year)
        {
            facts.Add(year.ToString(CultureInfo.CurrentCulture));
        }
        if (item.Runtime is { } runtime && runtime > TimeSpan.Zero)
        {
            facts.Add(BrowseText.Runtime(runtime));
        }
        if (details.OfficialRating is { Length: > 0 } rating)
        {
            facts.Add(rating);
        }
        if (details.CommunityRating is { } community)
        {
            facts.Add($"{community.ToString("0.0", CultureInfo.CurrentCulture)}/10");
        }
        return string.Join(" · ", facts);
    }

    private void ShowPictures(ItemDetails details)
    {
        var item = details.Summary;
        var wide = item.Kind is "episode" or "video";
        var posterWidth = (double)Application.Current.Resources[wide ? "WideTileWidth" : "PosterTileWidth"];
        PosterFrame.Width = posterWidth;
        PosterFrame.Height = Math.Round(wide ? posterWidth * 9 / 16 : posterWidth * 3 / 2);
        var poster = ItemTile.Pick(item, wide ? TileShape.Wide : TileShape.Poster);
        PosterFrame.Visibility = poster is null ? Visibility.Collapsed : Visibility.Visible;
        var scale = XamlRoot?.RasterizationScale ?? 1;
        if (poster is not null && poster.Image != _shownPoster)
        {
            _shownPoster = poster.Image;
            var width = (int)Math.Ceiling(PosterFrame.Width * scale);
            var height = (int)Math.Ceiling(PosterFrame.Height * scale);
            if (poster.BlurHash is { } hash)
            {
                _ = Load(_posterPlaceholder, PosterPlaceholder, new ImageRequest(new ImageSource.BlurHash(hash), width, height));
            }
            _ = Load(_poster, Poster, new ImageRequest(new ImageSource.Server(poster.Image), width, height));
        }
        var backdrop = details.Backdrops.FirstOrDefault();
        BackdropFrame.Visibility = backdrop is null ? Visibility.Collapsed : Visibility.Visible;
        if (backdrop is not null && backdrop.Image != _shownBackdrop && BackdropFrame.ActualWidth > 0)
        {
            _shownBackdrop = backdrop.Image;
            var width = (int)Math.Ceiling(BackdropFrame.ActualWidth * scale);
            _ = Load(_backdrop, Backdrop, new ImageRequest(new ImageSource.Server(backdrop.Image), width, (int)Math.Ceiling(width * 9.0 / 16)));
        }
    }

    /// <summary>16:9 of the page's width, at most <see cref="MaxBackdropHeight"/>; its image loads once the width is known.</summary>
    private void SizeBackdrop()
    {
        var height = Math.Min(Math.Round(BackdropFrame.ActualWidth * 9 / 16), MaxBackdropHeight);
        if (Math.Abs(BackdropFrame.Height - height) > 0.5 || double.IsNaN(BackdropFrame.Height))
        {
            BackdropFrame.Height = height;
        }
        if (_shownBackdrop is null && _details?.Value is { } details)
        {
            ShowPictures(details);
        }
    }

    private static async Task Load(TileImage loader, Image target, ImageRequest request)
    {
        try
        {
            if (await loader.LoadAsync(request) is { } bitmap)
            {
                target.Source = bitmap;
            }
        }
        catch (ImageLoadException)
        {
            // The placeholder stays.
        }
        catch (Exception error)
        {
            System.Diagnostics.Trace.TraceError($"Current: a detail page's image failed: {error.GetType().Name}");
        }
    }

    /// <summary>The seasons as the selector's items, the chosen one kept (the core shows the first until one is chosen).</summary>
    private void ShowSeasons()
    {
        if (_seasons is null)
        {
            return;
        }
        var chosen = (Seasons.SelectedItem?.Tag as ItemSummary)?.Item ?? _kept?.Season;
        _showingSeasons = true;
        Seasons.Items.Clear();
        foreach (var season in _seasons.Items)
        {
            var entry = new SelectorBarItem { Text = season.Name, Tag = season };
            Seasons.Items.Add(entry);
            if (season.Item == chosen)
            {
                Seasons.SelectedItem = entry;
            }
        }
        if (Seasons.SelectedItem is null && Seasons.Items.Count > 0)
        {
            Seasons.SelectedItem = Seasons.Items[0];
        }
        _showingSeasons = false;
        Seasons.Visibility = Seasons.Items.Count > 1 ? Visibility.Visible : Visibility.Collapsed;
    }

    private void OnSeasonChanged(SelectorBar sender, SelectorBarSelectionChangedEventArgs args)
    {
        if (!_showingSeasons && sender.SelectedItem?.Tag is ItemSummary season)
        {
            _view?.SelectSeason(season.Item);
            if (_kept is not null)
            {
                _kept.Season = season.Item;
            }
        }
    }

    // The button flips its own IsChecked at once; the core's change (or its rollback) sets it again through ShowDetails.
    private void OnPlayed(object sender, RoutedEventArgs e) => ItemActions.Toggle(_details?.Value?.Summary, played: true);

    private void OnFavourite(object sender, RoutedEventArgs e) => ItemActions.Toggle(_details?.Value?.Summary, played: false);

    private void OnRetry(object sender, RoutedEventArgs e) => _view?.Refresh();

    /// <summary>F5 and Ctrl+R: the page loads again, keeping what it shows until the answers arrive.</summary>
    private void OnRefreshAccelerator(Microsoft.UI.Xaml.Input.KeyboardAccelerator sender,
        Microsoft.UI.Xaml.Input.KeyboardAcceleratorInvokedEventArgs args)
    {
        _view?.Refresh();
        args.Handled = _view is not null;
    }
}
