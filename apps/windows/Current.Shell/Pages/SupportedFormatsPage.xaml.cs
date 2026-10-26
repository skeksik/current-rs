using System.Diagnostics;
using Current.Bindings;
using Current.Shell.Player;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Pages;

/// <summary>One row as the list shows it: the core's name and notes, and the support as the column's word.</summary>
public sealed record FormatRowView(string Name, string Support, string Notes)
{
    /// <summary>What Narrator reads for the row: name, support, notes.</summary>
    public string Spoken => Notes.Length > 0 ? $"{Name}, {Support}, {Notes}" : $"{Name}, {Support}";
}

/// <summary>
/// Settings > Supported formats (ADR 0021): what this PC plays, as the core builds it from the engine's capabilities, the graphics
/// card's decoder profiles (DecoderTest) and the facts of the display the window is on. This page runs the platform's two tests and
/// renders the result; it decides nothing. It waits for the core to open (App.CoreReady).
/// </summary>
public sealed partial class SupportedFormatsPage : Page
{
    private bool _loaded;
    private CancellationTokenSource? _load;
    private SupportedFormats? _formats;

    public SupportedFormatsPage()
    {
        InitializeComponent();
        Breadcrumb.ItemsSource = new[] { "Settings", "Supported formats" };
        Loaded += OnLoaded;
        Unloaded += OnUnloaded;
    }

    /// <summary>The words of the "On this PC" column.</summary>
    internal static string SupportText(FormatSupport support) => support switch
    {
        FormatSupport.Hardware => "Hardware",
        FormatSupport.Software => "Software",
        FormatSupport.Plays => "Plays",
        FormatSupport.ServerRepackages => "Server repackages",
        FormatSupport.ServerConverts => "Server converts",
        FormatSupport.Hdr => "HDR",
        FormatSupport.ToneMapped => "Tone-mapped",
        _ => "Not supported",
    };

    // async void, as event handlers are: it catches everything itself.
    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        _loaded = true;
        try
        {
            var core = await App.CoreReady;
            if (!_loaded || core is null)
            {
                return;
            }
            await LoadAsync(core);
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the supported formats page failed to load: {error.GetType().Name}");
        }
    }

    private void OnUnloaded(object sender, RoutedEventArgs e)
    {
        _loaded = false;
        _load?.Cancel();
    }

    /// <summary>
    /// Tests the graphics card (once per run, again if the adapters changed) and reads the window's display, both off the UI thread or
    /// cheap, then asks the core for the page and shows it. The smoke run calls it with its own core.
    /// </summary>
    internal async Task LoadAsync(CoreHost core)
    {
        _load?.Cancel();
        var load = new CancellationTokenSource();
        _load = load;
        LoadInfo.IsOpen = false;
        Loading.IsActive = _formats is null;
        try
        {
            var hardware = await DecoderTest.GetAsync();
            var display = XamlRoot?.ContentIslandEnvironment is { } island ? DisplayWatcher.ReadOnce(island.AppWindowId) : null;
            var formats = await core.SupportedFormatsAsync(hardware, display, load.Token);
            if (!load.IsCancellationRequested)
            {
                Show(formats);
            }
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: reading the supported formats failed: {error.GetType().Name}");
            LoadInfo.Severity = InfoBarSeverity.Error;
            LoadInfo.Title = "Couldn't find out what this PC plays.";
            LoadInfo.Message = ProblemText.Unexpected;
            LoadInfo.IsOpen = true;
        }
        finally
        {
            if (_load == load)
            {
                Loading.IsActive = false;
            }
        }
    }

    private void Show(SupportedFormats formats)
    {
        _formats = formats;
        DescriptionText.Text = formats.Description;
        HardwareText.Text = formats.HardwareNote;
        HardwareText.Visibility = formats.HardwareNote.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        if (Tabs.SelectedItem is null)
        {
            // Raises SelectionChanged, which shows the tab.
            Tabs.SelectedItem = Tabs.Items[0];
        }
        else
        {
            ShowTab();
        }
    }

    private void OnTabChanged(SelectorBar sender, SelectorBarSelectionChangedEventArgs args) => ShowTab();

    private void ShowTab()
    {
        if (_formats is null || Tabs.SelectedItem?.Tag is not string tag || !Enum.TryParse<FormatTabKind>(tag, out var kind))
        {
            return;
        }
        var tab = _formats.Tabs.FirstOrDefault(t => t.Kind == kind);
        NameHeader.Text = tab?.NameHeader ?? "";
        RowsList.ItemsSource = tab?.Rows.Select(row => new FormatRowView(row.Name, SupportText(row.Support), row.Notes)).ToList()
            ?? new List<FormatRowView>();
    }

    /// <summary>Names each row for Narrator, which would otherwise read the item's type.</summary>
    private void OnRowContainerChanging(ListViewBase sender, ContainerContentChangingEventArgs args)
    {
        if (args.Item is FormatRowView row)
        {
            AutomationProperties.SetName(args.ItemContainer, row.Spoken);
        }
    }

    /// <summary>"Settings" in the breadcrumb goes back to Settings, as Back does; the current page's item does nothing ("you typically
    /// don't want to perform any navigation if the user clicks the current item", Breadcrumb Bar, learn.microsoft.com, read
    /// 2026-10-26).</summary>
    private void OnBreadcrumbClicked(BreadcrumbBar sender, BreadcrumbBarItemClickedEventArgs args)
    {
        if (args.Index != 0)
        {
            return;
        }
        if (Frame.CanGoBack && Frame.BackStack[Frame.BackStack.Count - 1].SourcePageType == typeof(SettingsPage))
        {
            Frame.GoBack();
        }
        else
        {
            Frame.Navigate(typeof(SettingsPage));
        }
    }

    /// <summary>For the smoke run: shows tab <paramref name="index"/> and returns its header and its number of rows (0 before a load).</summary>
    internal (string Header, int Rows) ShowTabForTest(int index)
    {
        Tabs.SelectedItem = Tabs.Items[index];
        ShowTab();
        return (NameHeader.Text, RowsList.Items.Count);
    }

    internal int TabCount => Tabs.Items.Count;
}
