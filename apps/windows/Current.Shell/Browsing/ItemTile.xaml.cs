using System.Diagnostics;
using Current.Bindings;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Browsing;

/// <summary>How a tile's picture is shaped: a poster (2:3) for films and series, wide (16:9) for episodes and videos.</summary>
public enum TileShape
{
    Poster,
    Wide,
}

/// <summary>
/// One item as a tile, bound to a grid's <see cref="GridSlot"/> or a row's <see cref="ItemSummary"/> through its DataContext. A list
/// recycles its tiles: a new DataContext stops the old picture's load and starts the new one, the BlurHash first, both decoded at the
/// box's physical width (ADR 0018, section 4). The same picture under a changed item (a played or favourite mark) is kept.
/// </summary>
/// <remarks>
/// Narrator: "when a DataTemplate contains multiple controls … the default accessible name for screenreaders comes from .ToString()
/// on the item. As a convenience you can instead set the AutomationProperties.Name on the root element of the DataTemplate" ("Item
/// templates for grid view", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-25). This tile is that root, and names itself
/// with its lines and marks (BrowseText.TileName).
/// </remarks>
public sealed partial class ItemTile : UserControl
{
    private readonly TileImage _placeholder;
    private readonly TileImage _artwork;
    private ImageRef? _shown;

    public ItemTile()
    {
        InitializeComponent();
        _placeholder = new TileImage(App.Core!);
        _artwork = new TileImage(App.Core!);
        DataContextChanged += (_, _) => Follow();
        // Out of the window (scrolled past and recycled, or its page left): a load still under way stops and starts again when the
        // tile is back; a picture that arrived stays, so a kept page (Home) shows it again at once.
        Unloaded += (_, _) =>
        {
            if (_followed is not null)
            {
                _followed.PropertyChanged -= OnSlotChanged;
                _followed = null;
            }
            _placeholder.Recycle();
            _artwork.Recycle();
            if (Artwork.ImageSource is null)
            {
                _shown = null;
            }
        };
        Loaded += (_, _) => Follow();
    }

    /// <summary>Set before <see cref="TileWidth"/> or after: the picture's height follows either way.</summary>
    public TileShape Shape
    {
        get => _shape;
        set
        {
            _shape = value;
            Picture.Height = PictureHeight(Root.Width);
        }
    }

    private TileShape _shape = TileShape.Poster;

    /// <summary>The tile's width in view pixels; the picture's height follows from <see cref="Shape"/>.</summary>
    public double TileWidth
    {
        get => Root.Width;
        set
        {
            Root.Width = value;
            Picture.Height = PictureHeight(value);
        }
    }

    private double PictureHeight(double width) =>
        double.IsNaN(width) ? double.NaN : Math.Round(_shape == TileShape.Poster ? width * 3 / 2 : width * 9 / 16);

    /// <summary>False for the picture only, without the title and year under it (a library's View menu).</summary>
    public bool ShowsText
    {
        get => TextPart.Visibility == Visibility.Visible;
        set => TextPart.Visibility = value ? Visibility.Visible : Visibility.Collapsed;
    }

    /// <summary>False on a series' own page: an episode's tile names the episode, not its series again.</summary>
    public bool ShowsSeries { get; set; } = true;

    /// <summary>The item now shown, or null for a placeholder.</summary>
    public ItemSummary? Item { get; private set; }

    /// <summary>The place this tile shows, followed while it is the DataContext: a change to its item updates this card.</summary>
    private GridSlot? _followed;

    private void Follow()
    {
        if (!ReferenceEquals(_followed, DataContext))
        {
            if (_followed is not null)
            {
                _followed.PropertyChanged -= OnSlotChanged;
            }
            _followed = DataContext as GridSlot;
            if (_followed is not null)
            {
                _followed.PropertyChanged += OnSlotChanged;
            }
        }
        Bind();
    }

    /// <summary>Its item changed: the lines, marks and name again; the picture stays unless the image itself changed.</summary>
    private void OnSlotChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e) => Bind();

    private void Bind()
    {
        var slot = DataContext switch
        {
            GridSlot grid => grid.Slot,
            ItemSummary summary => new Slot.Item(summary),
            _ => new Slot.Loading(),
        };
        Item = (slot as Slot.Item)?.Summary;
        FailedGlyph.Visibility = slot is Slot.Failed ? Visibility.Visible : Visibility.Collapsed;
        if (Item is not { } item)
        {
            TitleText.Text = "";
            SubtitleText.Text = "";
            ShowMarks(null, null);
            ShowPicture(null);
            AutomationProperties.SetName(this, slot is Slot.Failed ? BrowseText.FailedTileName : BrowseText.LoadingTileName);
            return;
        }
        TitleText.Text = BrowseText.Title(item, ShowsSeries);
        SubtitleText.Text = BrowseText.Subtitle(item, ShowsSeries);
        ShowMarks(item.User, item.TimeLeft);
        ShowPicture(item);
        AutomationProperties.SetName(this, BrowseText.TileName(item, ShowsSeries));
    }

    private void ShowMarks(UserState? user, TimeSpan? timeLeft)
    {
        FavouriteBadge.Visibility = user?.Favourite == true ? Visibility.Visible : Visibility.Collapsed;
        PlayedBadge.Visibility = user?.Played == true ? Visibility.Visible : Visibility.Collapsed;
        var unplayed = user is { Played: false, UnplayedCount: > 0 } ? user.UnplayedCount : null;
        UnplayedBadge.Visibility = unplayed is null ? Visibility.Collapsed : Visibility.Visible;
        UnplayedCount.Text = unplayed?.ToString(System.Globalization.CultureInfo.CurrentCulture) ?? "";
        var progress = user is { Played: false, Progress: > 0 and < 1 } ? user.Progress : null;
        ProgressPart.Visibility = progress is null ? Visibility.Collapsed : Visibility.Visible;
        Progress.Value = progress ?? 0;
        TimeLeftText.Text = timeLeft is { } left ? BrowseLabels.TimeLeftShort(left) : "";
    }

    /// <summary>The artwork for the tile's shape, as the core picks it (ItemSummary.Poster and Wide).</summary>
    internal static Artwork? Pick(ItemSummary item, TileShape shape) => shape == TileShape.Poster ? item.Poster : item.Wide;

    private void ShowPicture(ItemSummary? item)
    {
        var art = item is null ? null : Pick(item, Shape);
        if (art is not null && art.Image == _shown)
        {
            return;
        }
        _shown = null;
        _placeholder.Recycle();
        _artwork.Recycle();
        Placeholder.ImageSource = null;
        Artwork.ImageSource = null;
        // Before the tile is in a window there is no scale to size the request by: Loaded binds again.
        if (art is null || XamlRoot is null || double.IsNaN(Root.Width))
        {
            return;
        }
        _shown = art.Image;
        var scale = XamlRoot.RasterizationScale;
        var width = (int)Math.Ceiling(Root.Width * scale);
        var height = (int)Math.Ceiling(Picture.Height * scale);
        if (art.BlurHash is { } hash)
        {
            _ = LoadAsync(_placeholder, Placeholder, new ImageRequest(new ImageSource.BlurHash(hash), width, height), art.Image);
        }
        _ = LoadAsync(_artwork, Artwork, new ImageRequest(new ImageSource.Server(art.Image), width, height), art.Image);
    }

    private async Task LoadAsync(TileImage loader, Microsoft.UI.Xaml.Media.ImageBrush target, ImageRequest request, ImageRef image)
    {
        try
        {
            var bitmap = await loader.LoadAsync(request);
            if (bitmap is not null && _shown == image)
            {
                target.ImageSource = bitmap;
            }
        }
        catch (ImageLoadException)
        {
            // Offline with nothing cached, or the server has no such image: the placeholder stays.
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: a tile's image failed: {error.GetType().Name}");
        }
    }
}
