using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell;

/// <summary>
/// One rule for the page title, the notices and every page column, the same in every display mode of the navigation view and on
/// every page, all of which show the navigation view's content layer: inside the layer, the title's glyphs about 17 px from its inner
/// left edge and 21 px below its inner top, and the same 17 px at the right and bottom. A page marks its column Grid with
/// <c>shell:PageLayout.IsColumn="True"</c>; the header is a TextBlock in the navigation view's HeaderTemplate.
/// </summary>
public static class PageLayout
{
    /// <summary>With the layer: the header's box 8 px below the layer's inner top, its glyphs 21 px below it; the box grows with the
    /// margin, so the column's top padding stays 24.</summary>
    private static readonly Thickness LayerHeader = new(17, 8, 17, 0);

    private static readonly Thickness LayerColumn = new(17, 24, 17, 17);

    public static readonly DependencyProperty IsColumnProperty = DependencyProperty.RegisterAttached(
        "IsColumn", typeof(bool), typeof(PageLayout), new PropertyMetadata(false, OnIsColumnChanged));

    public static readonly DependencyProperty IsHeaderProperty = DependencyProperty.RegisterAttached(
        "IsHeader", typeof(bool), typeof(PageLayout), new PropertyMetadata(false, OnIsHeaderChanged));

    public static bool GetIsColumn(Grid element) => (bool)element.GetValue(IsColumnProperty);

    public static void SetIsColumn(Grid element, bool value) => element.SetValue(IsColumnProperty, value);

    public static bool GetIsHeader(TextBlock element) => (bool)element.GetValue(IsHeaderProperty);

    public static void SetIsHeader(TextBlock element, bool value) => element.SetValue(IsHeaderProperty, value);

    internal static Thickness NoticesMargin => new(LayerColumn.Left, 0, LayerColumn.Right, 0);

    private static void OnIsColumnChanged(DependencyObject sender, DependencyPropertyChangedEventArgs args) => Register(sender, args);

    private static void OnIsHeaderChanged(DependencyObject sender, DependencyPropertyChangedEventArgs args) => Register(sender, args);

    private static void Register(DependencyObject sender, DependencyPropertyChangedEventArgs args)
    {
        if (sender is FrameworkElement element && args.NewValue is true)
        {
            Place(element);
        }
    }

    private static void Place(FrameworkElement element)
    {
        switch (element)
        {
            case Grid column:
                column.Padding = LayerColumn;
                break;
            case TextBlock header:
                header.Margin = LayerHeader;
                break;
        }
    }
}
