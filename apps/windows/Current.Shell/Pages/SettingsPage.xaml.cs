using System.Diagnostics;
using Current.Bindings;
using Current.Shell.Gateway;
using Current.Shell.ViewModels;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Pages;

/// <summary>
/// Settings: the app theme under Personalization, the saved servers under Accounts (sign out, sign in again, sign in to the gateway
/// again, extra headers, remove), skipping intros and credits and the supported formats page under Playback, the image cache under Storage, diagnostics, and About. It waits for the core to open (App.CoreReady); the server subscriptions live from Loaded to Unloaded (ADR 0004).
/// </summary>
public sealed partial class SettingsPage : Page
{
    private bool _loaded;

    /// <summary>The theme combo is being set from the core: its SelectionChanged is not a choice of the user's.</summary>
    private bool _showingTheme;

    /// <summary>A Remove confirmation is open: a second one is not started.</summary>
    private bool _confirmingRemoval;

    /// <summary>A gateway sign-in is running (its dialog or window is open): a second one is not started.</summary>
    private bool _gatewayRunning;

    public SettingsPage()
    {
        InitializeComponent();
        Loaded += OnLoaded;
        Unloaded += OnUnloaded;
    }

    public ServersViewModel? Servers { get; private set; }

    public string VersionText { get; private set; } = "";

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
            VersionText = $"Version {core.AppInfo.Version}";
            ShowTheme(core.AppTheme);
            ThemeBox.IsEnabled = true;
            ShowAutoSkip(core.PlaybackSettings().SkipsIntrosAndCredits);
            AutoSkipSwitch.IsEnabled = true;
            ShowQualityResolution(core.PlaybackSettings().ShowsQualityResolution);
            QualityResolutionSwitch.IsEnabled = true;
            ShowAllQualities(core.PlaybackSettings().ShowsAllQualities);
            AllQualitiesSwitch.IsEnabled = true;
            ShowPreferredQuality(core);
            PreferredQualityBox.IsEnabled = true;
            ShowLibraryMemory(core.LibraryMemory);
            RememberSortSwitch.IsEnabled = RememberFilterSwitch.IsEnabled = RememberViewSwitch.IsEnabled = true;
            Servers ??= new ServersViewModel(core, new DispatcherQueueUi(DispatcherQueue));
            Servers.Start();
            Bindings.Update();
            await ShowCacheSizeAsync(core);
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the settings page failed to load: {error.GetType().Name}");
        }
    }

    private void OnUnloaded(object sender, RoutedEventArgs e)
    {
        _loaded = false;
        Servers?.Stop();
    }

    /// <summary>The combo's items in order: Light, Dark, Windows default (SettingsPage.xaml).</summary>
    private static readonly AppTheme[] ThemeItems = [AppTheme.Light, AppTheme.Dark, AppTheme.System];

    private void ShowTheme(AppTheme theme)
    {
        _showingTheme = true;
        ThemeBox.SelectedIndex = Array.IndexOf(ThemeItems, theme);
        _showingTheme = false;
    }

    /// <summary>
    /// Applies the chosen theme at once: the core saves it and the window follows (MainWindow.FollowAppTheme). If it cannot be saved,
    /// the combo goes back to the theme the core still has, and the notice says why. async void, as event handlers are: it catches
    /// everything itself.
    /// </summary>
    private async void OnThemeChanged(object sender, SelectionChangedEventArgs e)
    {
        var index = ThemeBox.SelectedIndex;
        if (_showingTheme || App.Core is not { } core || index < 0 || index >= ThemeItems.Length)
        {
            return;
        }
        ThemeInfo.IsOpen = false;
        try
        {
            await core.SetAppThemeAsync(ThemeItems[index]);
        }
        catch (CoreStoreException error)
        {
            ShowTheme(core.AppTheme);
            ShowThemeInfo(ProblemText.Store(error.Problem));
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: changing the app theme failed: {error.GetType().Name}");
            ShowTheme(core.AppTheme);
            ShowThemeInfo(ProblemText.Unexpected);
        }
    }

    private void ShowThemeInfo(string message)
    {
        ThemeInfo.Severity = InfoBarSeverity.Error;
        ThemeInfo.Title = "Couldn't change the app theme.";
        ThemeInfo.Message = message;
        ThemeInfo.IsOpen = true;
    }

    /// <summary>The auto-skip switch is being set from the core: its Toggled is not a choice of the user's.</summary>
    private bool _showingAutoSkip;

    private void ShowAutoSkip(bool skip)
    {
        _showingAutoSkip = true;
        AutoSkipSwitch.IsOn = skip;
        _showingAutoSkip = false;
    }

    /// <summary>
    /// Saves the auto-skip switch at once (the core; open players follow). If it cannot be saved it still stands for this run (the
    /// core's rule), and the notice says so. async void, as event handlers are: it catches everything itself.
    /// </summary>
    private async void OnAutoSkipToggled(object sender, RoutedEventArgs e)
    {
        if (_showingAutoSkip || App.Core is not { } core)
        {
            return;
        }
        PlaybackInfo.IsOpen = false;
        try
        {
            await core.SetSkipIntrosAndCreditsAsync(AutoSkipSwitch.IsOn);
        }
        catch (CoreStoreException error)
        {
            ShowPlaybackInfo($"It applies until Current closes. {ProblemText.Store(error.Problem)}");
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: changing auto-skip failed: {error.GetType().Name}");
            ShowAutoSkip(core.PlaybackSettings().SkipsIntrosAndCredits);
            ShowPlaybackInfo(ProblemText.Unexpected);
        }
    }

    /// <summary>The remember switches are being set from the core: their Toggled is not a choice of the user's.</summary>
    private bool _showingLibraryMemory;

    private void ShowLibraryMemory(LibraryMemorySettings memory)
    {
        _showingLibraryMemory = true;
        RememberSortSwitch.IsOn = memory.RememberSort;
        RememberFilterSwitch.IsOn = memory.RememberFilter;
        RememberViewSwitch.IsOn = memory.RememberView;
        _showingLibraryMemory = false;
    }

    /// <summary>
    /// Saves a remember switch at once (the core; off forgets that choice in every library). If it cannot be saved nothing changed, so
    /// the switches show the core's again. async void, as event handlers are: it catches everything itself.
    /// </summary>
    private async void OnRememberToggled(object sender, RoutedEventArgs e)
    {
        if (_showingLibraryMemory || App.Core is not { } core)
        {
            return;
        }
        LibrariesInfo.IsOpen = false;
        try
        {
            var on = ((ToggleSwitch)sender).IsOn;
            if (ReferenceEquals(sender, RememberSortSwitch))
            {
                await core.SetRememberSortAsync(on);
            }
            else if (ReferenceEquals(sender, RememberFilterSwitch))
            {
                await core.SetRememberFilterAsync(on);
            }
            else
            {
                await core.SetRememberViewAsync(on);
            }
        }
        catch (CoreStoreException error)
        {
            ShowLibraryMemory(core.LibraryMemory);
            ShowLibrariesInfo(ProblemText.Store(error.Problem));
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: changing a remember switch failed: {error.GetType().Name}");
            ShowLibraryMemory(core.LibraryMemory);
            ShowLibrariesInfo(ProblemText.Unexpected);
        }
    }

    private void ShowLibrariesInfo(string message)
    {
        LibrariesInfo.Severity = InfoBarSeverity.Error;
        LibrariesInfo.Title = "Couldn't save this setting.";
        LibrariesInfo.Message = message;
        LibrariesInfo.IsOpen = true;
    }

    /// <summary>The quality resolution switch is being set from the core: its Toggled is not a choice of the user's.</summary>
    private bool _showingQualityResolution;

    private void ShowQualityResolution(bool on)
    {
        _showingQualityResolution = true;
        QualityResolutionSwitch.IsOn = on;
        _showingQualityResolution = false;
    }

    /// <summary>Saves "Show resolution in quality menu" at once; if it cannot be saved it still stands for this run (the core's rule).</summary>
    private async void OnQualityResolutionToggled(object sender, RoutedEventArgs e)
    {
        if (_showingQualityResolution || App.Core is not { } core)
        {
            return;
        }
        PlaybackInfo.IsOpen = false;
        try
        {
            await core.SetShowQualityResolutionAsync(QualityResolutionSwitch.IsOn);
        }
        catch (CoreStoreException error)
        {
            ShowPlaybackInfo($"It applies until Current closes. {ProblemText.Store(error.Problem)}");
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: changing the quality menu's resolutions failed: {error.GetType().Name}");
            ShowQualityResolution(core.PlaybackSettings().ShowsQualityResolution);
            ShowPlaybackInfo(ProblemText.Unexpected);
        }
        // Its entries follow the switch: which steps it lists, and how they are named.
        ShowPreferredQuality(core);
    }

    /// <summary>The preferred quality's list is being filled from the core: its SelectionChanged is not a choice of the user's.</summary>
    private bool _showingPreferredQuality;

    /// <summary>The entries of the preferred quality's list, as the core lists them (index for index).</summary>
    private IReadOnlyList<QualityOption> _preferredOptions = [];

    /// <summary>Fills the preferred quality's list as the core lists it, named as the two switches say, and selects the preferred one.</summary>
    private void ShowPreferredQuality(CoreHost core)
    {
        var settings = core.PlaybackSettings();
        _showingPreferredQuality = true;
        _preferredOptions = core.PreferredQualityOptions();
        PreferredQualityBox.Items.Clear();
        foreach (var option in _preferredOptions)
        {
            PreferredQualityBox.Items.Add(core.QualityLabel(option, settings.ShowsQualityResolution));
        }
        PreferredQualityBox.SelectedIndex = _preferredOptions
            .Select((option, index) => (option, index))
            .FirstOrDefault(entry => entry.option.Quality == settings.PreferredQuality, (null!, -1)).index;
        _showingPreferredQuality = false;
    }

    /// <summary>Saves "Preferred quality" at once; if it cannot be saved it still stands for this run (the core's rule).</summary>
    private async void OnPreferredQualityChanged(object sender, SelectionChangedEventArgs e)
    {
        var index = PreferredQualityBox.SelectedIndex;
        if (_showingPreferredQuality || App.Core is not { } core || index < 0 || index >= _preferredOptions.Count)
        {
            return;
        }
        PlaybackInfo.IsOpen = false;
        try
        {
            await core.SetPreferredQualityAsync(_preferredOptions[index].Quality);
        }
        catch (CoreStoreException error)
        {
            ShowPlaybackInfo($"It applies until Current closes. {ProblemText.Store(error.Problem)}");
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: changing the preferred quality failed: {error.GetType().Name}");
            ShowPreferredQuality(core);
            ShowPlaybackInfo(ProblemText.Unexpected);
        }
    }

    /// <summary>The all-qualities switch is being set from the core: its Toggled is not a choice of the user's.</summary>
    private bool _showingAllQualities;

    private void ShowAllQualities(bool all)
    {
        _showingAllQualities = true;
        AllQualitiesSwitch.IsOn = all;
        _showingAllQualities = false;
    }

    /// <summary>Saves "Show all quality options" at once; if it cannot be saved it still stands for this run (the core's rule).</summary>
    private async void OnAllQualitiesToggled(object sender, RoutedEventArgs e)
    {
        if (_showingAllQualities || App.Core is not { } core)
        {
            return;
        }
        PlaybackInfo.IsOpen = false;
        try
        {
            await core.SetShowAllQualitiesAsync(AllQualitiesSwitch.IsOn);
        }
        catch (CoreStoreException error)
        {
            ShowPlaybackInfo($"It applies until Current closes. {ProblemText.Store(error.Problem)}");
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: changing the quality menu's list failed: {error.GetType().Name}");
            ShowAllQualities(core.PlaybackSettings().ShowsAllQualities);
            ShowPlaybackInfo(ProblemText.Unexpected);
        }
        // Its entries follow the switch: which steps it lists, and how they are named.
        ShowPreferredQuality(core);
    }

    private void ShowPlaybackInfo(string message)
    {
        PlaybackInfo.Severity = InfoBarSeverity.Error;
        PlaybackInfo.Title = "Couldn't save this setting.";
        PlaybackInfo.Message = message;
        PlaybackInfo.IsOpen = true;
    }

    private void OnAddServer(object sender, RoutedEventArgs e) => Frame.Navigate(typeof(AddServerPage));

    private void OnSupportedFormats(object sender, RoutedEventArgs e) => Frame.Navigate(typeof(SupportedFormatsPage));

    private void OnReload(object sender, RoutedEventArgs e) => Servers?.Reload();

    private void OnSignInAgain(object sender, RoutedEventArgs e)
    {
        if (App.Core is { } core && (sender as FrameworkElement)?.DataContext is ServerRow row)
        {
            SignInRequest.SignInAgain(Frame, core, row);
        }
    }

    /// <summary>"Sign in to the gateway": the session was kept and resumes once the gateway lets it through (ADR 0017).</summary>
    private void OnSignInToGateway(object sender, RoutedEventArgs e)
    {
        if (sender is Button { DataContext: ServerRow row } button)
        {
            RunGatewaySignIn(row, button, false);
        }
    }

    /// <summary>"Advanced: extra headers": the headers editor whenever the gateway asks for anything (ADR 0017, "Mechanism 3").</summary>
    private void OnEditHeaders(object sender, RoutedEventArgs e)
    {
        if (sender is Button { Tag: ServerRow row } button)
        {
            RunGatewaySignIn(row, button, true);
        }
    }

    // async void, as event handlers are: it catches everything itself.
    private async void RunGatewaySignIn(ServerRow row, Button origin, bool headersFirst)
    {
        if (App.Core is not { } core || _gatewayRunning || XamlRoot is null)
        {
            return;
        }
        _gatewayRunning = true;
        AccountsInfo.IsOpen = false;
        try
        {
            var outcome = await new GatewaySignInRunner(XamlRoot, new DispatcherQueueUi(DispatcherQueue))
                .RunAsync(core, row.Server, headersFirst);
            if (outcome.Message is { } message)
            {
                ShowAccountsInfo(outcome.Severity, row.Name, message);
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: signing in to the gateway failed: {error.GetType().Name}");
            ShowAccountsInfo(InfoBarSeverity.Error, row.Name, ProblemText.Unexpected);
        }
        finally
        {
            _gatewayRunning = false;
            // The dialog or window that had focus is gone: back to the button that started it, while it is shown.
            origin.Focus(FocusState.Programmatic);
        }
    }

    /// <summary>
    /// Signs out at once, without asking (manager, 2026-10-23): signing in again undoes it. This PC forgets the session either way;
    /// the notice says when the server could not be told (ADR 0015, "Sign out"). async void, as event handlers are: it catches
    /// everything itself.
    /// </summary>
    private async void OnSignOut(object sender, RoutedEventArgs e)
    {
        if (App.Core is not { } core || sender is not Button { DataContext: ServerRow row } button)
        {
            return;
        }
        button.IsEnabled = false;
        AccountsInfo.IsOpen = false;
        try
        {
            var result = await core.SignOutAsync(row.Server);
            if (!result.ServerTold)
            {
                ShowAccountsInfo(
                    InfoBarSeverity.Informational,
                    $"Signed out of {row.Name} on this PC.",
                    "The server couldn't be told, so the session may still show in its dashboard.");
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: signing out failed: {error.GetType().Name}");
            ShowAccountsInfo(InfoBarSeverity.Error, $"Couldn't sign out of {row.Name}.", ProblemText.Unexpected);
        }
        finally
        {
            button.IsEnabled = true;
        }
    }

    /// <summary>
    /// Removes a server after a confirmation (Dialogs page, learn.microsoft.com, ms.date 2026-07-15, read 2026-10-23): the close
    /// button is the safe action, the default, and answers Esc, as the plain-http dialog. Unlike sign-out, removing cannot be undone
    /// by signing in again: the server would have to be added again. Home and this list update from the saved-server list. async
    /// void, as event handlers are: it catches everything itself.
    /// </summary>
    private async void OnRemove(object sender, RoutedEventArgs e)
    {
        if (App.Core is not { } core || sender is not Button { Tag: ServerRow row } button || row.Removing || _confirmingRemoval
            || XamlRoot is null)
        {
            return;
        }
        AccountsInfo.IsOpen = false;
        // Only one dialog at a time, and nothing is disabled while it is open: a disabled Remove button would lose focus, and
        // Cancel would leave it elsewhere (the owner's run, 2026-10-23).
        _confirmingRemoval = true;
        ContentDialogResult answer;
        try
        {
            var dialog = new ContentDialog
            {
                XamlRoot = XamlRoot,
                Title = $"Remove {row.Name}?",
                Content = "This signs out every user on this PC and forgets the server's addresses. You can add it again later.",
                PrimaryButtonText = "Remove",
                CloseButtonText = "Cancel",
                DefaultButton = ContentDialogButton.Close,
            };
            answer = await dialog.ShowAsync();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the remove confirmation failed: {error.GetType().Name}");
            ShowAccountsInfo(InfoBarSeverity.Error, $"Couldn't remove {row.Name}.", ProblemText.Unexpected);
            return;
        }
        finally
        {
            _confirmingRemoval = false;
        }
        if (answer != ContentDialogResult.Primary)
        {
            // Whether ContentDialog itself gives focus back to the element that opened it is not verified (no Microsoft source
            // found, 2026-10-23), so it is given back here.
            button.Focus(FocusState.Programmatic);
            return;
        }
        row.Removing = true;
        try
        {
            var result = await core.RemoveServerAsync(row.Server);
            FocusAfterRemoval(row);
            if (result.CredentialsLeft)
            {
                ShowAccountsInfo(
                    InfoBarSeverity.Warning,
                    $"Removed {row.Name}, but its saved sign-in may still be in Credential Manager.",
                    "Current no longer uses it. You can delete the entries ending in \".Current\" under Windows Credentials.");
            }
        }
        catch (CoreStoreException error)
        {
            ShowAccountsInfo(InfoBarSeverity.Error, $"Couldn't remove {row.Name}.", ProblemText.Store(error.Problem));
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: removing a server failed: {error.GetType().Name}");
            ShowAccountsInfo(InfoBarSeverity.Error, $"Couldn't remove {row.Name}.", ProblemText.Unexpected);
        }
        finally
        {
            // A row that stays (the removal failed) works again; a removed one is gone with its buttons.
            row.Removing = false;
        }
    }

    /// <summary>
    /// The removed row took the focused button with it: once the row has left the list, focus goes to "Add another server", or to
    /// "Add a server" when no server is left. Queued, so the list's visibility has changed first.
    /// </summary>
    private void FocusAfterRemoval(ServerRow row)
    {
        if (Servers is not { } servers)
        {
            return;
        }
        void MoveFocus() => DispatcherQueue.TryEnqueue(() =>
            (servers.HasServers ? AddAnotherServerButton : AddFirstServerButton).Focus(FocusState.Programmatic));
        if (!servers.Rows.Contains(row))
        {
            MoveFocus();
            return;
        }
        void OnRowsChanged(object? sender, System.Collections.Specialized.NotifyCollectionChangedEventArgs e)
        {
            if (!servers.Rows.Contains(row))
            {
                servers.Rows.CollectionChanged -= OnRowsChanged;
                MoveFocus();
            }
        }
        servers.Rows.CollectionChanged += OnRowsChanged;
    }

    /// <summary>The image cache's size on disk, read when the page is shown and after Clear.</summary>
    private async Task ShowCacheSizeAsync(CoreHost core)
    {
        try
        {
            var size = await core.ImageCacheSizeAsync();
            if (_loaded)
            {
                ImageCacheCard.Description = size == 0 ? "Empty" : $"{Browsing.BrowseText.Bytes(size)} of images";
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: reading the image cache's size failed: {error.GetType().Name}");
            ImageCacheCard.Description = "The size couldn't be read.";
        }
    }

    /// <summary>
    /// Clears at once, without asking: the images load again from the servers, so nothing is lost. The button stays enabled, so focus
    /// stays on it; a second click while one runs does nothing. async void, as event handlers are: it catches everything itself.
    /// </summary>
    private async void OnClearCache(object sender, RoutedEventArgs e)
    {
        if (App.Core is not { } core || _clearing)
        {
            return;
        }
        _clearing = true;
        StorageInfo.IsOpen = false;
        try
        {
            await core.ClearImageCacheAsync();
            Announce.Say(ImageCacheCard, "Image cache cleared", "image-cache");
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: clearing the image cache failed: {error.GetType().Name}");
            StorageInfo.Severity = InfoBarSeverity.Error;
            StorageInfo.Title = "Couldn't clear the image cache.";
            StorageInfo.Message = ProblemText.Unexpected;
            StorageInfo.IsOpen = true;
        }
        await ShowCacheSizeAsync(core);
        _clearing = false;
    }

    private bool _clearing;

    private void ShowAccountsInfo(InfoBarSeverity severity, string title, string message)
    {
        AccountsInfo.Severity = severity;
        AccountsInfo.Title = title;
        AccountsInfo.Message = message;
        AccountsInfo.IsOpen = true;
    }

    // async void, as event handlers are: it catches everything itself, because an exception escaping it would end the process.
    private async void OnCopyDiagnostics(object sender, RoutedEventArgs e)
    {
        bool copied;
        try
        {
            var text = App.Core is { } core ? await core.ExportDiagnosticsAsync() : "";
            copied = await ClipboardText.TryCopyAsync(text);
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: copying diagnostics failed: {error.GetType().Name}");
            copied = false;
        }
        CopiedInfo.IsOpen = false;
        CopiedInfo.Severity = copied ? InfoBarSeverity.Success : InfoBarSeverity.Error;
        CopiedInfo.Message = copied
            ? "Diagnostics copied to the clipboard."
            : "The clipboard is in use by another app. Try again.";
        CopiedInfo.IsOpen = true;
    }
}
