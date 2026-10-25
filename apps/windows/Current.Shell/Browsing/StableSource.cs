using System.Runtime.CompilerServices;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Browsing;

/// <summary>
/// Gives a list inside a data template its items once per data object, instead of through x:Bind. The generated x:Bind code sets a
/// template's values again whenever WinUI raises the template's DataContextChanged, and WinUI raised it with nothing changed (the same
/// row object, after a refresh or with the pointer over the window). Each time the list took the same collection as a new source and
/// built every card again: the row went blank for a frame (the owner's run and its log, 2026-10-25). Why WinUI raises it then is not
/// verified. Use it on the UI thread.
/// </summary>
internal static class StableSource
{
    /// <summary>The data object each list shows now.</summary>
    private static readonly ConditionalWeakTable<ItemsControl, object> Shown = new();

    /// <summary>The lists followed already.</summary>
    private static readonly ConditionalWeakTable<ItemsControl, object> Followed = new();

    /// <summary>
    /// Follows <paramref name="list"/>'s DataContext from now on: <paramref name="source"/> gives its items (and template) for a data
    /// object, or null when it is not one this list shows. Call it once per list, when it loads.
    /// </summary>
    public static void Follow<T>(ItemsControl list, Func<T, (object Items, DataTemplate? Template)> source)
        where T : class
    {
        if (Followed.TryGetValue(list, out _))
        {
            Show(list, source);
            return;
        }
        Followed.Add(list, list);
        list.DataContextChanged += (_, _) => Show(list, source);
        Show(list, source);
    }

    private static void Show<T>(ItemsControl list, Func<T, (object Items, DataTemplate? Template)> source)
        where T : class
    {
        if (list.DataContext is not T data || (Shown.TryGetValue(list, out var shown) && ReferenceEquals(shown, data)))
        {
            return;
        }
        Shown.AddOrUpdate(list, data);
        var (items, template) = source(data);
        if (template is not null)
        {
            list.ItemTemplate = template;
        }
        list.ItemsSource = items;
    }
}
