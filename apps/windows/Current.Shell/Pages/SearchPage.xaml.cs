using System.Collections.ObjectModel;
using Current.Shell.Browsing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Navigation;

namespace Current.Shell.Pages;

/// <summary>
/// The results of a search from the navigation's search box: every signed-in server's best matches, grouped by kind (ADR 0018). The
/// searches are the core's, kept with the page's history entry (BrowseHub.Search): Back shows the results the page had.
/// </summary>
public sealed partial class SearchPage : Page
{
    private readonly ObservableCollection<SearchSection> _sections = [];

    public SearchPage()
    {
        InitializeComponent();
        Sections.ItemsSource = _sections;
        // Released once the page has left the window: during its exit transition it still shows its rows.
        Loaded += (_, _) => _wasLoaded = true;
        Unloaded += (_, _) => Release();
    }

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        var term = (e.Parameter as SearchArgs)?.Term ?? "";
        TitleText.Text = $"Results for “{term}”";
        if (App.Browsing is not { } hub || term.Length == 0)
        {
            return;
        }
        foreach (var section in hub.Search(e.Parameter!, term))
        {
            section.PropertyChanged += OnSectionChanged;
            section.Rows.CollectionChanged += OnRowsChanged;
            _sections.Add(section);
        }
        ShowStatus();
    }

    private void OnSectionChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e) => ShowStatus();

    private void OnRowsChanged(object? sender, System.Collections.Specialized.NotifyCollectionChangedEventArgs e) => ShowStatus();

    /// <summary>The page reached the window: its lists go on Unloaded. One navigated from before it loaded has no Unloaded.</summary>
    private bool _wasLoaded;

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        if (!_wasLoaded)
        {
            // Left before it ever loaded (navigated past at once): no Unloaded will come.
            DispatcherQueue.TryEnqueue(Release);
        }
    }

    /// <summary>Lets go of the sections; they stay with the history entry (BrowseHub.Search), which disposes them.</summary>
    private void Release()
    {
        foreach (var section in _sections)
        {
            section.PropertyChanged -= OnSectionChanged;
            section.Rows.CollectionChanged -= OnRowsChanged;
        }
        _sections.Clear();
    }

    /// <summary>"Searching…", then nothing once there are results, "No results", or why the search failed.</summary>
    private void ShowStatus()
    {
        var loading = _sections.Any(section => section.State.Loading);
        var found = _sections.Sum(section => section.Rows.Count);
        var failed = _sections.Select(section => section.State.Failed).FirstOrDefault(problem => problem is not null);
        LoadingRing.IsActive = loading && found == 0;
        var text = found > 0 ? ""
            : loading ? "Searching…"
            : failed is not null ? $"Couldn't search. {BrowseText.Problem(failed)}"
            : "No results";
        if (StatusText.Text != text)
        {
            StatusText.Text = text;
            StatusText.Visibility = text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
            // A live region is read only when its peer raises LiveRegionChanged (Announce.Shown): "No results" was not heard
            // (the owner's run, 2026-10-25).
            if (text.Length > 0)
            {
                Announce.Shown(StatusText);
            }
        }
    }

    /// <summary>F5 and Ctrl+R: every server's search runs again, keeping its results until the answers arrive.</summary>
    private void OnRefreshAccelerator(Microsoft.UI.Xaml.Input.KeyboardAccelerator sender,
        Microsoft.UI.Xaml.Input.KeyboardAcceleratorInvokedEventArgs args)
    {
        foreach (var section in _sections)
        {
            section.Refresh();
        }
        args.Handled = _sections.Count > 0;
    }

    private void OnRowLoaded(object sender, RoutedEventArgs e)
    {
        if (sender is ListViewBase list)
        {
            HomeSection.AttachOnce(list, item => Frame.Navigate(typeof(DetailsPage), new DetailsArgs(item.Item)));
        }
    }
}
