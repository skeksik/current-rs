using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell;

/// <summary>
/// One rule for the page title, the notices and every page column (docs/notes/windows-shell.md, "Page layout"), the same in every
/// display mode of the navigation view, measured from the content's left edge (the pane, the icon rail or the window's edge), and
/// one of two by the page:
/// <list type="bullet">
/// <item>Pages without the content layer (Settings) follow Windows Settings: 24 px at both sides, the title's glyphs 6 px below the
/// title bar (owner's run, 2026-10-24).</item>
/// <item>Pages with the layer (Home, Add a server, Sign in) follow Task Manager's Users page inside the layer: the title's glyphs
/// about 17 px from the layer's inner left edge and 21 px below its inner top, and the same 17 px at the right and bottom.</item>
/// </list>
/// The navigation view's own header margins are 0 (MainWindow.xaml) and its header is a TextBlock this class places. Measured on the
/// owner's run, the header's 28 px glyphs start about 13 px below the top of its box, which is at least 36 px high (the template's
/// MinHeight, PaneToggleButtonHeight); the offsets below are worked from that. The content keeps the distance from the title's
/// glyphs to the first item that the owner's run matched with Windows Settings (24 px between the boxes at a header margin of 0).
/// A page marks its column Grid with <c>shell:PageLayout.IsColumn="True"</c>; that Grid has one star column with the page's
/// MaxWidth, so the column fills the width up to that maximum and stays at the left: a StackPanel with HorizontalAlignment="Left"
/// would size to its content instead, and one stretched with a MaxWidth centres. MainWindow calls <see cref="SetContentLayer"/> on
/// every navigation.
/// </summary>
public static class PageLayout
{
    /// <summary>Without the layer: the header's box 8 px above the content's top, so its glyphs sit 6 px below the title bar (the
    /// layer's 1 px top border, still there but transparent, plus 13 - 8). Its 36 px box then ends 7 px higher than at margin 0, and
    /// the column's top padding is 7 px less, 17, so the distance from the glyphs to the content stays.</summary>
    private static readonly Thickness PlainHeader = new(24, -8, 24, 0);

    private static readonly Thickness PlainColumn = new(24, 17, 24, 24);

    /// <summary>With the layer: the header's box 8 px below the layer's inner top, its glyphs 21 px below it; the box grows with the
    /// margin, so the column's top padding stays 24.</summary>
    private static readonly Thickness LayerHeader = new(17, 8, 17, 0);

    private static readonly Thickness LayerColumn = new(17, 24, 17, 17);

    /// <summary>
    /// Whether a page shows the navigation view's content layer (its background, border and rounded corner): content pages do, as
    /// Task Manager's Users page does; pages made of settings cards set it to false and show the window's Mica, as the pane does
    /// (owner, 2026-10-24).
    /// </summary>
    public static readonly DependencyProperty HasContentLayerProperty = DependencyProperty.RegisterAttached(
        "HasContentLayer", typeof(bool), typeof(PageLayout), new PropertyMetadata(true));

    public static readonly DependencyProperty IsColumnProperty = DependencyProperty.RegisterAttached(
        "IsColumn", typeof(bool), typeof(PageLayout), new PropertyMetadata(false, OnIsColumnChanged));

    public static readonly DependencyProperty IsHeaderProperty = DependencyProperty.RegisterAttached(
        "IsHeader", typeof(bool), typeof(PageLayout), new PropertyMetadata(false, OnIsHeaderChanged));

    /// <summary>The columns and headers still alive; weak, so a page left behind is not kept by them.</summary>
    private static readonly List<WeakReference<FrameworkElement>> Placed = [];

    private static bool _contentLayer = true;

    public static bool GetHasContentLayer(Page element) => (bool)element.GetValue(HasContentLayerProperty);

    public static void SetHasContentLayer(Page element, bool value) => element.SetValue(HasContentLayerProperty, value);

    public static bool GetIsColumn(Grid element) => (bool)element.GetValue(IsColumnProperty);

    public static void SetIsColumn(Grid element, bool value) => element.SetValue(IsColumnProperty, value);

    public static bool GetIsHeader(TextBlock element) => (bool)element.GetValue(IsHeaderProperty);

    public static void SetIsHeader(TextBlock element, bool value) => element.SetValue(IsHeaderProperty, value);

    internal static Thickness NoticesMargin
    {
        get
        {
            var column = _contentLayer ? LayerColumn : PlainColumn;
            return new Thickness(column.Left, 0, column.Right, 0);
        }
    }

    /// <summary>Places the header and the columns for the page now shown.</summary>
    internal static void SetContentLayer(bool contentLayer)
    {
        if (_contentLayer == contentLayer)
        {
            return;
        }
        _contentLayer = contentLayer;
        Prune();
        foreach (var reference in Placed)
        {
            if (reference.TryGetTarget(out var element))
            {
                Place(element);
            }
        }
    }

    private static void OnIsColumnChanged(DependencyObject sender, DependencyPropertyChangedEventArgs args) => Register(sender, args);

    private static void OnIsHeaderChanged(DependencyObject sender, DependencyPropertyChangedEventArgs args) => Register(sender, args);

    private static void Register(DependencyObject sender, DependencyPropertyChangedEventArgs args)
    {
        if (sender is FrameworkElement element && args.NewValue is true)
        {
            Prune();
            Placed.Add(new WeakReference<FrameworkElement>(element));
            Place(element);
        }
    }

    private static void Prune() => Placed.RemoveAll(reference => !reference.TryGetTarget(out _));

    private static void Place(FrameworkElement element)
    {
        switch (element)
        {
            case Grid column:
                column.Padding = _contentLayer ? LayerColumn : PlainColumn;
                break;
            case TextBlock header:
                header.Margin = _contentLayer ? LayerHeader : PlainHeader;
                break;
        }
    }
}
