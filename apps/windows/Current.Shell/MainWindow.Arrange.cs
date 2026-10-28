using Current.Bindings;
using Current.Shell.Browsing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;

namespace Current.Shell;

/// <summary>
/// The pane's edit mode (ADR 0024; ADR 0022, departures 8 and 9, owner, 2026-10-28): the pencil above Settings swaps the pane's items
/// for one reorderable list per group of each signed-in server (Movies and Series, Favourites and Recently added, the libraries),
/// each item moved by dragging or its arrows and shown or hidden by its eye. Home shows above the lists and Settings in the footer,
/// both dimmed and fixed. Every change is saved at once by the core, which publishes the arrangement again; the lists follow it.
/// Done, Escape, choosing a page (Back, a link), or closing the pane leaves it.
/// </summary>
public sealed partial class MainWindow
{
    private bool _editing;

    /// <summary>The lists of edit mode, by server and group, kept while it is open so a change keeps focus on its row.</summary>
    private readonly Dictionary<(ServerRef Server, NavGroup Group), ArrangeList> _editLists = [];

    /// <summary>
    /// The pencil shows while the pane is open (full, or over the page) and a server is signed in; in edit mode the pane's items give
    /// way to the lists. Called after every change of the pane's items (ShowLibraries) and of the pane's state.
    /// </summary>
    private void ShowEditNavigation()
    {
        var hub = App.Browsing;
        if (_editing && (hub is null || !hub.IsAvailable || !NavView.IsPaneOpen))
        {
            LeaveEditMode();
            return;
        }
        EditNavItem.Visibility = !_editing && hub is { IsAvailable: true } && NavView.IsPaneOpen
            ? Visibility.Visible
            : Visibility.Collapsed;
        if (!_editing || hub is null)
        {
            return;
        }
        foreach (var item in NavView.MenuItems.OfType<FrameworkElement>())
        {
            item.Visibility = Visibility.Collapsed;
        }
        EditScroll.MaxHeight = EditListsHeight();
        ShowEditLists(hub);
    }

    private void EnterEditMode()
    {
        if (App.Browsing is not { IsAvailable: true })
        {
            return;
        }
        _editing = true;
        // What shows now, given back on leaving: ShowLibraries sets the items it manages again, but not Home.
        _shownBeforeEdit.Clear();
        foreach (var item in NavView.MenuItems.OfType<FrameworkElement>())
        {
            _shownBeforeEdit[item] = item.Visibility;
        }
        EditPane.Visibility = Visibility.Visible;
        // Fixed and dimmed while the lists show (the mock); Done, Escape or closing the pane leaves.
        SetSettingsItemEnabled(false);
        ShowEditNavigation();
        EditDoneButton.Focus(FocusState.Programmatic);
    }

    /// <summary>The top-level items' visibility when edit mode opened.</summary>
    private readonly Dictionary<FrameworkElement, Visibility> _shownBeforeEdit = [];

    /// <summary>The one way out of edit mode: every top-level item as it was, then the pane's items as the arrangement now is.</summary>
    private void LeaveEditMode()
    {
        if (!_editing)
        {
            return;
        }
        _editing = false;
        _editLists.Clear();
        EditGroups.Children.Clear();
        EditPane.Visibility = Visibility.Collapsed;
        SetSettingsItemEnabled(true);
        foreach (var (item, visibility) in _shownBeforeEdit)
        {
            item.Visibility = visibility;
        }
        _shownBeforeEdit.Clear();
        if (App.Browsing is { } hub)
        {
            ShowLibraries(hub);
        }
        else
        {
            ShowEditNavigation();
        }
    }

    /// <summary>The navigation view's Settings item, disabled (so dimmed) in edit mode. Null before its template is applied.</summary>
    private void SetSettingsItemEnabled(bool enabled)
    {
        if (NavView.SettingsItem is Control settings)
        {
            settings.IsEnabled = enabled;
        }
    }

    /// <summary>For the smoke run: enters or leaves edit mode as the pencil and Done do.</summary>
    internal void EditNavigation(bool on)
    {
        if (on)
        {
            EnterEditMode();
        }
        else
        {
            LeaveEditMode();
        }
    }

    /// <summary>For the smoke run: whether edit mode is open, and whether Home's item shows.</summary>
    internal bool IsEditingNavigation => _editing;

    internal bool HomeItemShown => HomeItem.Visibility == Visibility.Visible;

    private void OnEditDone(object sender, RoutedEventArgs e)
    {
        LeaveEditMode();
        EditNavItem.Focus(FocusState.Programmatic);
    }

    private void OnEditEscape(KeyboardAccelerator sender, KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        LeaveEditMode();
    }

    /// <summary>
    /// The lists' height: the pane's custom content takes the height it asks for, so the lists scroll within what is left above the
    /// footer (the title bar's 48, the heading, the note and Home, Settings' item); not verified on the owner's PC.
    /// </summary>
    private double EditListsHeight() => Math.Max(160, NavView.ActualHeight - 48 - 128 - 48 - 16);

    /// <summary>The lists in the order the pane shows its groups, a heading per server when more than one is signed in.</summary>
    private void ShowEditLists(BrowseHub hub)
    {
        var servers = hub.Servers;
        var children = new List<UIElement>();
        var kept = new HashSet<(ServerRef, NavGroup)>();
        foreach (var server in servers)
        {
            if (server.Pane is not { } pane)
            {
                continue;
            }
            if (servers.Count > 1)
            {
                children.Add(Heading(hub.ServerName(server.Server), Microsoft.UI.Xaml.Automation.Peers.AutomationHeadingLevel.Level3));
            }
            foreach (var (group, name, rows) in new[]
            {
                (NavGroup.Tabs, "Pages", PaneRows.Tabs(pane, compact: true)),
                (NavGroup.Views, "Views", PaneRows.ViewItems(pane, compact: true)),
                (NavGroup.Libraries, "Libraries", PaneRows.Libraries(pane, compact: true)),
            })
            {
                if (rows.Count == 0)
                {
                    continue;
                }
                var key = (server.Server, group);
                kept.Add(key);
                if (!_editLists.TryGetValue(key, out var list))
                {
                    var session = server.Session;
                    list = new ArrangeList
                    {
                        ListName = servers.Count > 1 ? $"{name}, {hub.ServerName(server.Server)}" : name,
                        Move = (item, after) => session.MoveAsync(group, item, after),
                        SetHidden = (item, hidden) => session.SetHiddenAsync(group, item, hidden),
                    };
                    _editLists[key] = list;
                }
                list.Show(rows);
                children.Add(Heading(name, Microsoft.UI.Xaml.Automation.Peers.AutomationHeadingLevel.Level4));
                children.Add(list);
            }
        }
        foreach (var gone in _editLists.Keys.Where(key => !kept.Contains(key)).ToList())
        {
            _editLists.Remove(gone);
        }
        // Rebuilt only when what it holds changed, so a list that has focus stays in the tree.
        if (!EditGroups.Children.SequenceEqual(children, new HeadingComparer()))
        {
            EditGroups.Children.Clear();
            foreach (var child in children)
            {
                EditGroups.Children.Add(child);
            }
        }
    }

    private static TextBlock Heading(string text, Microsoft.UI.Xaml.Automation.Peers.AutomationHeadingLevel level)
    {
        var heading = new TextBlock
        {
            Text = text,
            Margin = new Thickness(0, 8, 0, 0),
            Style = (Style)Application.Current.Resources["CaptionTextBlockStyle"],
        };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetHeadingLevel(heading, level);
        return heading;
    }

    /// <summary>The same children: the same lists, and headings with the same text.</summary>
    private sealed class HeadingComparer : IEqualityComparer<UIElement>
    {
        public bool Equals(UIElement? x, UIElement? y) =>
            ReferenceEquals(x, y) || (x is TextBlock a && y is TextBlock b && a.Text == b.Text);

        public int GetHashCode(UIElement obj) => obj is TextBlock text ? text.Text.GetHashCode(StringComparison.Ordinal) : obj.GetHashCode();
    }
}
