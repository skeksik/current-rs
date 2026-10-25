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
/// its similar items. Everything is the core's DetailsView, opened when the page is shown and closed when it is left. No Play yet:
/// the player adds it (ADR 0019, task 5), in PlayActions.
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
    private ValueSubscription<ItemDetails?>? _details;
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
        _view = server.Session.Details(args.Item);
        _details = _view.ObserveDetails(ui);
        _details.PropertyChanged += OnDetailsChanged;
        _state = _view.ObserveState(ui);
        _state.PropertyChanged += OnStateChanged;
        _seasons = _view.ObserveSeasons(ui);
        _seasons.Items.CollectionChanged += (_, _) => ShowSeasons();
        var episodes = _view.ObserveEpisodes(ui);
        var similar = _view.ObserveSimilar(ui);
        similar.Items.CollectionChanged += (_, _) =>
            SimilarPart.Visibility = similar.Items.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
        _subscriptions.AddRange([_details, _state, _seasons, episodes, similar]);
        Episodes.ItemsSource = episodes.Items;
        Similar.ItemsSource = similar.Items;
        ShowDetails();
        ShowState();
    }

    private void Release()
    {
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
        _view?.Dispose();
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
        People.ItemsSource = details.People;
        PeoplePart.Visibility = details.People.Count > 0 ? Visibility.Visible : Visibility.Collapsed;
        SeriesPart.Visibility = item.Kind == "series" ? Visibility.Visible : Visibility.Collapsed;
        ShowPictures(details);
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
        if (item.Kind == "series" && details.Premiere is { } start)
        {
            facts.Add(details.End is { } end && end.Year != start.Year
                ? $"{start.Year}–{end.Year}"
                : details.Status == "Continuing" ? $"{start.Year}–" : start.Year.ToString(CultureInfo.CurrentCulture));
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
        var chosen = (Seasons.SelectedItem?.Tag as ItemSummary)?.Item;
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
        }
    }

    // The button flips its own IsChecked at once; the core's change (or its rollback) sets it again through ShowDetails.
    private void OnPlayed(object sender, RoutedEventArgs e) => ItemActions.Toggle(_details?.Value?.Summary, played: true);

    private void OnFavourite(object sender, RoutedEventArgs e) => ItemActions.Toggle(_details?.Value?.Summary, played: false);

    private void OnRetry(object sender, RoutedEventArgs e) => _view?.Refresh();
}
