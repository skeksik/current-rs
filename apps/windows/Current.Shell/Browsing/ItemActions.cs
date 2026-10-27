using System.Diagnostics;
using Current.Bindings;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Browsing;

/// <summary>
/// What every list of tiles does: Enter, a click or a tap opens the item; its context menu marks it played or unplayed and adds it
/// to or removes it from favourites (ADR 0018, section 5). The change shows at once in every list (the core's ItemStates), and a
/// refusal rolls back with a notice (BrowseHub).
/// </summary>
/// <remarks>
/// <para>The menu is set on each item's container, the element that has focus, so the keyboard opens it too: "Keyboard | Shift+F10,
/// Menu button" are context actions, and on one "the MenuFlyout or CommandBarFlyout corresponding to the item will be displayed"
/// ("Contextual commanding for collections and lists", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-25); "When you set the
/// ContextFlyout property, the context menu is shown and hidden automatically" (UIElement.ContextFlyout, learn.microsoft.com,
/// updated 2026-07-28, read 2026-10-25).</para>
/// <para>Items open on ItemClick with SelectionMode None and IsItemClickEnabled, so a list has no selection to keep, and Enter on a
/// focused item invokes it (not verified in a Microsoft source; checked on the owner's run).</para>
/// </remarks>
internal static class ItemActions
{
    /// <summary>Opens items of <paramref name="list"/> with <paramref name="open"/> and gives each its context menu.</summary>
    public static void Attach(ListViewBase list, Action<ItemSummary> open)
    {
        list.SelectionMode = ListViewSelectionMode.None;
        list.IsItemClickEnabled = true;
        list.ItemClick += (_, args) =>
        {
            if (SummaryOf(args.ClickedItem) is { } item)
            {
                open(item);
            }
        };
        var menu = new MenuFlyout();
        // Play or Resume first, for an item that plays by itself (the core's PlayTarget; a series plays from its page). E768 "Play"
        // ("Segoe Fluent Icons font", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-25).
        var play = new MenuFlyoutItem { Icon = new FontIcon { Glyph = "\uE768" } };
        var playSeparator = new MenuFlyoutSeparator();
        var played = new MenuFlyoutItem { Icon = new FontIcon { Glyph = "\uE73E" } };
        var favourite = new MenuFlyoutItem();
        menu.Items.Add(play);
        menu.Items.Add(playSeparator);
        menu.Items.Add(played);
        menu.Items.Add(favourite);
        ItemSummary? target = null;
        PlayTarget? playTarget = null;
        menu.Opening += (_, _) =>
        {
            target = menu.Target is DependencyObject container ? SummaryOf(list.ItemFromContainer(container)) : null;
            if (target is null || App.Browsing?.Find(target.Item.Server) is not { } server)
            {
                menu.Hide();
                return;
            }
            var state = server.Session.GetUserState(target.Item) ?? target.User;
            playTarget = PlayTarget.ForItem(target with { User = state });
            play.Visibility = playSeparator.Visibility = playTarget is null ? Visibility.Collapsed : Visibility.Visible;
            play.Text = playTarget is { Resumes: true } ? "Resume" : "Play";
            played.Text = state.Played ? "Mark as unplayed" : "Mark as played";
            favourite.Text = state.Favourite ? "Remove from favourites" : "Add to favourites";
            favourite.Icon = new FontIcon { Glyph = state.Favourite ? "" : "" };
        };
        played.Click += (_, _) => Toggle(target, played: true);
        play.Click += (_, _) =>
        {
            if (playTarget is { } chosen)
            {
                App.Play(chosen, fromStart: false);
            }
        };
        favourite.Click += (_, _) => Toggle(target, played: false);
        list.ContainerContentChanging += (_, args) =>
        {
            if (!args.InRecycleQueue)
            {
                // A person has nothing to play or mark played: no menu (a search's People).
                args.ItemContainer.ContextFlyout = SummaryOf(args.Item) is { Kind: not "person" } ? menu : null;
            }
        };
    }

    public static ItemSummary? SummaryOf(object? item) => item switch
    {
        GridSlot slot => slot.Summary,
        ItemSummary summary => summary,
        _ => null,
    };

    /// <summary>The item's marks as every list shows them now, read from the core at once: a list's subscription takes a change a
    /// moment later, through the UI's queue.</summary>
    public static UserState? StateOf(ItemSummary? item)
    {
        if (item is null || App.Browsing?.Find(item.Item.Server) is not { } server)
        {
            return item?.User;
        }
        try
        {
            return server.Session.GetUserState(item.Item) ?? item.User;
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: reading an item's marks failed: {error.GetType().Name}");
            return item.User;
        }
    }

    /// <summary>Flips the item's played (or favourite) mark, from the state every list shows now.</summary>
    public static void Toggle(ItemSummary? item, bool played)
    {
        if (item is null || App.Browsing?.Find(item.Item.Server) is not { } server)
        {
            return;
        }
        try
        {
            var state = server.Session.GetUserState(item.Item) ?? item.User;
            if (played)
            {
                server.Session.SetPlayed(item.Item, !state.Played);
            }
            else
            {
                server.Session.SetFavourite(item.Item, !state.Favourite);
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: changing an item's marks failed: {error.GetType().Name}");
        }
    }
}
