using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell;

/// <summary>
/// One rule for the page title, the notices and every page column, in both of the navigation view's display modes
/// (docs/notes/windows-shell.md, "Page layout"). They start at the same left inset: 56 px with the pane shown (Expanded), where the
/// navigation view puts its header (NavigationViewHeaderMargin 56,44,0,0), and 24 px with the pane hidden (Minimal), where
/// MainWindow.xaml moves the header (NavigationViewMinimalHeaderMargin). The right inset is 24 px in both, so a narrow window has
/// equal margins at both edges. The content starts 24 px below the title. A page marks its column Grid with
/// <c>shell:PageLayout.IsColumn="True"</c>; MainWindow calls <see cref="SetMinimal"/> when the display mode changes. That Grid has
/// one star column with the page's MaxWidth, so the column fills the width up to that maximum and stays at the left: a StackPanel
/// with HorizontalAlignment="Left" would size to its content instead, and one stretched with a MaxWidth centres.
/// </summary>
public static class PageLayout
{
    private const double ExpandedLeft = 56;
    private const double MinimalLeft = 24;
    private const double Right = 24;
    private const double TitleGap = 24;
    private const double Bottom = 24;

    public static readonly DependencyProperty IsColumnProperty = DependencyProperty.RegisterAttached(
        "IsColumn", typeof(bool), typeof(PageLayout), new PropertyMetadata(false, OnIsColumnChanged));

    /// <summary>The columns of pages still alive; weak, so a page left behind is not kept by its column.</summary>
    private static readonly List<WeakReference<Grid>> Columns = [];

    private static bool _minimal;

    public static bool GetIsColumn(Grid element) => (bool)element.GetValue(IsColumnProperty);

    public static void SetIsColumn(Grid element, bool value) => element.SetValue(IsColumnProperty, value);

    internal static Thickness NoticesMargin => new(Left, 0, Right, 0);

    private static Thickness ColumnPadding => new(Left, TitleGap, Right, Bottom);

    private static double Left => _minimal ? MinimalLeft : ExpandedLeft;

    internal static void SetMinimal(bool minimal)
    {
        if (_minimal == minimal)
        {
            return;
        }
        _minimal = minimal;
        Columns.RemoveAll(column => !column.TryGetTarget(out _));
        foreach (var column in Columns)
        {
            if (column.TryGetTarget(out var grid))
            {
                grid.Padding = ColumnPadding;
            }
        }
    }

    private static void OnIsColumnChanged(DependencyObject sender, DependencyPropertyChangedEventArgs args)
    {
        if (sender is Grid grid && args.NewValue is true)
        {
            Columns.Add(new WeakReference<Grid>(grid));
            grid.Padding = ColumnPadding;
        }
    }
}
