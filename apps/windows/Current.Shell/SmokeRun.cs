using System.Diagnostics;
using Current.Bindings;
using Current.Shell.Browsing;
using Current.Shell.Gateway;
using Current.Shell.Pages;
using Current.Shell.Player;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.ApplicationModel.DataTransfer;

namespace Current.Shell;

/// <summary>
/// <c>Current.exe --smoke</c>: a headless start and exit for CI that runs the bridge checks the D1 spike left to a person
/// (docs/notes/windows-shell.md). It starts XAML and the core, builds the main window without showing it, and checks that the first
/// capabilities delivery arrives on the UI thread and that disposing the subscription releases the core's listener. It prints one line
/// per check and exits 0 only if every check passed.
/// </summary>
internal static class SmokeRun
{
    public const string Argument = "--smoke";

    private static readonly TimeSpan Timeout = TimeSpan.FromSeconds(10);

    public static async Task RunAndExitAsync(DispatcherQueue ui)
    {
        var failures = 0;
        void Check(bool passed, string what)
        {
            Console.WriteLine($"{(passed ? "ok  " : "FAIL")} {what}");
            if (!passed)
            {
                failures++;
            }
        }

        try
        {
            using var core = CoreHost.Create();
            var version = core.AppInfo.Version;
            Check(version.Length > 0, $"core version '{version}'");
            // The app ships the libmpv engine (ADR 0019 task 5), never the test engine (build.ps1 checks its marker too); the libmpv DLLs
            // loaded, or the core would not have started.
            Check(core.PlayerEngine() == PlayerEngineKind.Libmpv, $"the libmpv player engine ({core.PlayerEngine()})");
            Console.WriteLine($"     data folder {AppIdentity.DataFolder}");

            var baseline = core.LiveListeners;
            var firstDelivery = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
            var subscription = core.ObserveCapabilities(new DispatcherQueueUi(ui));
            // Attached before this method yields, so the first delivery (queued on this thread) cannot be missed.
            subscription.PropertyChanged += (_, _) => firstDelivery.TrySetResult(ui.HasThreadAccess);
            Check(core.LiveListeners == baseline + 1, $"the core holds the listener ({core.LiveListeners}, before {baseline})");

            var delivered = await Task.WhenAny(firstDelivery.Task, Task.Delay(Timeout)) == firstDelivery.Task;
            Check(delivered, "the first capabilities value arrived");
            Check(delivered && firstDelivery.Task.Result, "it arrived on the UI thread");
            Check(subscription.Value.Servers.Count == 0, "no servers yet");

            subscription.Dispose();
            var released = await WaitUntilAsync(() => core.LiveListeners == baseline);
            Check(released, $"disposing the subscription released the listener ({core.LiveListeners}, before {baseline})");

            var diagnostics = core.DiagnosticsText();
            Check(diagnostics is not null, "diagnostics text");

            // The same copy as "Copy diagnostics", read back.
            var marker = $"Current smoke run {Guid.NewGuid()}";
            var copied = await ClipboardText.TryCopyAsync(marker);
            var pasted = copied ? await Clipboard.GetContent().GetTextAsync() : null;
            Check(copied && pasted == marker, "the clipboard holds the copied text");

            await StorageChecksAsync(ui, Check);

            // The pages' XAML loads; Settings uses the Community Toolkit's SettingsCard, so this also shows the toolkit loads.
            _ = new HomePage();
            _ = new AddServerPage();
            _ = new SignInPage();
            _ = new SettingsPage();
            _ = new SupportedFormatsPage();
            _ = new ExtraHeadersEditor();
            Check(true, "home, add-server, sign-in, settings and supported formats pages and the extra headers editor built");

            // The network observer reads the facts without asking for anything; the facts themselves are not printed.
            var facts = NetworkObserver.Read();
            Check(true, $"network facts read (connectivity {facts.Connectivity})");
            Check(DeviceName.Get().Length > 0, "device name read");
            // Listed as each search does; neither the count nor the addresses are printed, since a runner may have none.
            _ = LocalInterfaces.Read();
            Check(true, "local interfaces read");

            // Last: closing the only window ends XAML's message loop, so nothing may be awaited after it (seen in CI: the run stopped at
            // the next await and exited without a result). The window is shown so the navigation view's template applies, and the
            // content layer is read from the template part itself: Home and Settings show the same visible background.
            var window = new MainWindow();
            window.Activate();
            var shown = await Task.WhenAny(window.Shown, Task.Delay(Timeout)) == window.Shown;
            Check(shown, "main window shown");
            var homeLayer = window.ContentLayerBackground;
            Check(Alpha(homeLayer) > 0, $"home shows the content layer (alpha {Alpha(homeLayer)})");
            window.ShowSettings();
            var settingsLayer = window.ContentLayerBackground;
            Check(
                Alpha(settingsLayer) > 0 && ReferenceEquals(settingsLayer, homeLayer),
                $"settings shows the same content layer (alpha {Alpha(settingsLayer)})");
            // The app theme reaches the window and its caption buttons without a restart (core that stores nothing: only published).
            window.FollowAppTheme(core);
            var themes = new[]
            {
                (AppTheme.Dark, ElementTheme.Dark, Microsoft.UI.Colors.White),
                (AppTheme.Light, ElementTheme.Light, Microsoft.UI.Colors.Black),
            };
            foreach (var (theme, element, glyph) in themes)
            {
                await core.SetAppThemeAsync(theme);
                var applied = await WaitUntilAsync(() =>
                    window.ActualTheme == element && window.CaptionButtonForeground is { } color && color.Equals(glyph));
                Check(applied, $"the {theme} app theme reaches the window and its caption buttons ({window.ActualTheme})");
            }
            await core.SetAppThemeAsync(AppTheme.System);
            await SupportedFormatsChecksAsync(window, core, Check);
            // While the main window is open: closing the sign-in window must not be the last window closing.
            await BrowseChecksAsync(window, ui, Check);
            await SignInWindowChecksAsync(Check);
            window.Close();
            Check(true, "main window built");
        }
        catch (Exception error)
        {
            Check(false, $"unexpected exception: {error}");
        }

        Console.WriteLine(failures == 0 ? "smoke run passed" : $"smoke run failed: {failures} check(s)");
        Console.Out.Flush();
        Program.SetExitCode(failures == 0 ? 0 : 1);
        Application.Current.Exit();
    }

    /// <summary>
    /// The sign-in window against the Jellyfin stand-in's portal on 127.0.0.1 (ADR 0017), when <c>CURRENT_STAND_IN</c> names the built
    /// stand-in (smoke.ps1 -StandIn, as CI runs it): setup finds the web sign-in; the window opens its page with the title and the
    /// origin bar the state names (plain http, so the warning, not the lock); following the portal's link sets its HttpOnly cookie, the
    /// window hands it over and setup reaches Ready; the throw-away profile is deleted after the window closes. Skipped, visibly,
    /// without the stand-in.
    /// </summary>
    private static async Task SignInWindowChecksAsync(Action<bool, string> check)
    {
        var path = Environment.GetEnvironmentVariable("CURRENT_STAND_IN");
        if (string.IsNullOrEmpty(path) || !File.Exists(path))
        {
            Console.WriteLine("skip the sign-in window against the stand-in's portal: CURRENT_STAND_IN does not name the stand-in");
            return;
        }
        var runtime = WebViewRuntime.Version();
        check(runtime is not null, $"the WebView2 Runtime is installed ({runtime ?? "none"})");
        if (runtime is null)
        {
            return;
        }
        using var standIn = Process.Start(new ProcessStartInfo(path)
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        }) ?? throw new InvalidOperationException("the stand-in did not start");
        var folder = Path.Combine(Path.GetTempPath(), $"current-smoke-{Guid.NewGuid():N}");
        try
        {
            var line = await standIn.StandardOutput.ReadLineAsync().WaitAsync(Timeout);
            var address = line is { } listening && listening.StartsWith("listening ", StringComparison.Ordinal)
                ? listening["listening ".Length..]
                : throw new InvalidOperationException("the stand-in did not report its address");
            await standIn.StandardInput.WriteLineAsync("gate portal");
            await standIn.StandardInput.FlushAsync();

            using var core = await CoreHost.OpenAsync(folder, Path.Combine(folder, "Cache"), "smoke", SecretStore.Memory);
            using var flow = core.AddServer();
            await flow.ProbeAsync(address);
            check(flow.State is AddServerState.GatewaySignIn, $"setup finds the portal's web sign-in ({flow.State.GetType().Name})");
            if (flow.State is not AddServerState.GatewaySignIn web)
            {
                return;
            }
            var window = new SignInWindow(web.Url, web.Origin, cookies => flow.SubmitCookiesAsync(cookies, false));
            window.Activate();
            var opened = await Task.WhenAny(window.FirstPage, Task.Delay(Timeout * 3)) == window.FirstPage && window.FirstPage.Result;
            check(opened, "the sign-in window opens the portal's page");
            var host = web.Origin["http://".Length..];
            check(window.Title == $"Sign in to {host}", "its title names the server's origin");
            var (origin, locked) = window.OriginShown;
            check(origin == web.Origin && !locked, "its origin bar shows the origin, with the warning for plain http");
            window.NavigateForTest($"{web.Url}/gate/sign-in");
            var ready = await WaitUntilAsync(() => flow.State is AddServerState.Ready, Timeout * 3);
            check(ready, $"after the portal's sign-in, its cookie lets the server through ({flow.State.GetType().Name})");
            window.CloseFromApp();
            var profiles = Path.Combine(AppIdentity.DataFolder, "SignIn");
            var deleted = await WaitUntilAsync(
                () => !Directory.Exists(profiles) || !Directory.EnumerateDirectories(profiles).Any(), Timeout * 2);
            check(deleted, "the window's throw-away profile is deleted after it closes");
        }
        catch (Exception error)
        {
            check(false, $"the sign-in window check failed: {error.GetType().Name}");
        }
        finally
        {
            // It exits when its stdin closes.
            standIn.StandardInput.Close();
            if (!standIn.WaitForExit(5000))
            {
                standIn.Kill();
            }
            try
            {
                Directory.Delete(folder, true);
            }
            catch (Exception error) when (error is IOException or UnauthorizedAccessException)
            {
            }
        }
    }

    /// <summary>The stand-in's library of many items (tools/jellyfin-stand-in, paging.rs), which its UserViews does not list.</summary>
    private const string BigLibraryId = "57a2d1e0000000000000000000006000";

    private const int BigLibrarySize = 450;

    /// <summary>The stand-in's films library (library.rs), and one the smoke run adds.</summary>
    private const string FilmsLibraryId = "57a2d1e0000000000000000000001001";

    private const string AddedLibraryId = "57a2d1e0000000000000000000001003";

    /// <summary>
    /// Sends the stand-in a command (its main.rs), then a count, which it answers only after every earlier line: once this returns,
    /// the command has taken effect.
    /// </summary>
    private static async Task SendAsync(Process standIn, string command)
    {
        await standIn.StandardInput.WriteLineAsync(command);
        await standIn.StandardInput.WriteLineAsync("count /");
        await standIn.StandardInput.FlushAsync();
        await standIn.StandardOutput.ReadLineAsync().WaitAsync(Timeout);
    }

    /// <summary>
    /// Browsing in the main window against the Jellyfin stand-in (ADR 0018, task 5), when <c>CURRENT_STAND_IN</c> names it: signed in
    /// with its password, the window's navigation lists its two libraries; the library grid (LibraryPage) is sized to the big
    /// library's total with its first page loaded, and scrolled to its end, the grid's visible range reaches the core (SlotCollection's
    /// IItemsRangeInfo) and the last slots load. Skipped, visibly, without the stand-in.
    /// </summary>
    private static async Task BrowseChecksAsync(MainWindow window, DispatcherQueue ui, Action<bool, string> check)
    {
        var path = Environment.GetEnvironmentVariable("CURRENT_STAND_IN");
        if (string.IsNullOrEmpty(path) || !File.Exists(path))
        {
            Console.WriteLine("skip browsing against the stand-in: CURRENT_STAND_IN does not name the stand-in");
            return;
        }
        using var standIn = Process.Start(new ProcessStartInfo(path)
        {
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            UseShellExecute = false,
            CreateNoWindow = true,
        }) ?? throw new InvalidOperationException("the stand-in did not start");
        BrowseHub? hub = null;
        var core = CoreHost.Create();
        try
        {
            var line = await standIn.StandardOutput.ReadLineAsync().WaitAsync(Timeout);
            var address = line is { } listening && listening.StartsWith("listening ", StringComparison.Ordinal)
                ? listening["listening ".Length..]
                : throw new InvalidOperationException("the stand-in did not report its address");
            using (var add = core.AddServer())
            {
                await add.ProbeAsync(address);
                using var signIn = add.SignIn();
                await signIn.WithPasswordAsync("stand-in-user", "not-a-real-password");
                check(signIn.State is SignInState.SignedIn, $"signed in to the stand-in ({signIn.State.GetType().Name})");
                if (signIn.State is not SignInState.SignedIn)
                {
                    return;
                }
            }

            // The tiles load their images through App.Core.
            App.Core = core;
            hub = new BrowseHub(core, new DispatcherQueueUi(ui));
            App.Browsing = hub;
            window.FollowBrowsing(hub);
            var listed = await WaitUntilAsync(() => window.LibraryItemCount == 2);
            check(listed, $"the navigation lists the server's libraries ({window.LibraryItemCount})");
            if (hub.Servers.FirstOrDefault() is not { } server)
            {
                check(false, "a browse session for the stand-in");
                return;
            }

            window.ShowLibrary(new Library(new ItemRef(server.Server, BigLibraryId), "Example Big Library", "movies", []));
            var page = window.ShownPage as LibraryPage;
            var sized = await WaitUntilAsync(() => page?.Slots is { Count: BigLibrarySize } slots && slots[0].Slot is Slot.Item);
            check(sized, $"the library grid is sized to its total and shows its first page ({page?.Slots?.Count ?? 0})");
            page?.ScrollTo(BigLibrarySize - 1);
            var loaded = await WaitUntilAsync(() => page?.Slots is { Count: BigLibrarySize } slots && slots[^1].Slot is Slot.Item);
            check(loaded, "scrolled to its end, the grid's visible range loads the last slots");

            // The library's own search: Sort and Filter go off while it has text, and come back with the query before it.
            page?.SearchFor("Example");
            check(page is { SortAndFilterAvailable: false }, "a library search turns Sort and Filter off");
            page?.SearchFor("");
            check(page is { SortAndFilterAvailable: true }, "clearing the search turns them on again");

            // The Favourites tab, a row per kind (the stand-in counts every item a favourite), and the Recently added grid.
            window.ShowView(new ViewArgs(server.Server, ViewArgs.Favourites));
            var rows = await WaitUntilAsync(() => window.ShownPage is SearchPage { RowCount: > 0 });
            check(rows, $"the Favourites tab shows its rows ({(window.ShownPage as SearchPage)?.RowCount ?? 0})");
            window.ShowView(new ViewArgs(server.Server, ViewArgs.RecentlyAdded));
            var recent = await WaitUntilAsync(() => window.ShownPage is LibraryPage { Slots.Count: > 0 } recentPage
                && recentPage.Slots[0].Slot is Slot.Item);
            check(recent, "the Recently added tab shows its grid");

            // Libraries that change on the server: one added shows in the pane after a refresh; the films library removed while its
            // page shows sends the page back to Home with a notice, and the user stays signed in.
            await SendAsync(standIn, $"library add {AddedLibraryId} examplekind Example Added");
            server.Session.Refresh();
            var added = await WaitUntilAsync(() => window.LibraryItemCount == 3);
            check(added, $"a library added on the server joins the navigation ({window.LibraryItemCount})");
            if (hub.FindLibrary(new ItemRef(server.Server, FilmsLibraryId)) is { } films)
            {
                window.ShowLibrary(films);
                await SendAsync(standIn, $"library remove {FilmsLibraryId}");
                server.Session.Refresh();
                var home = await WaitUntilAsync(() => window.ShownPage is HomePage && window.LibraryItemCount == 2);
                check(home, $"a removed library's page goes back to Home ({window.ShownPage?.GetType().Name}, {window.LibraryItemCount})");
                check(window.NoticeTitles.Count(title => title == BrowseText.LibraryGoneTitle) == 1,
                    "one notice says the library is no longer available");
                check(hub.Find(server.Server) is not null, "still signed in");
                // Back from Home never reopens the gone library: the big library's page, opened before it, shows instead.
                window.GoBack();
                var back = await WaitUntilAsync(() => window.ShownPage is LibraryPage);
                check(back && window.ShownParameter is LibraryArgs { Library.Item.Id: BigLibraryId },
                    $"Back from Home skips the gone library ({window.ShownPage?.GetType().Name})");
            }
            else
            {
                check(false, "the navigation lists the films library");
            }
            // Off the grid before its session goes: the page lets go of its slots.
            window.ShowSettings();
        }
        catch (Exception error)
        {
            check(false, $"the browse check failed: {error.GetType().Name}");
        }
        finally
        {
            App.Browsing = null;
            hub?.Dispose();
            App.Core = null;
            core.Dispose();
            standIn.StandardInput.Close();
            if (!standIn.WaitForExit(5000))
            {
                standIn.Kill();
            }
        }
    }

    /// <summary>
    /// Settings > Supported formats (ADR 0021): the core reads what this build's libmpv plays from a headless engine (no window, no audio
    /// device), the page opens inside Settings, and every tab has rows after it loads with this runner's graphics card test and display.
    /// The test's result is printed as a count only, never the adapter's name.
    /// </summary>
    private static async Task SupportedFormatsChecksAsync(MainWindow window, CoreHost core, Action<bool, string> check)
    {
        try
        {
            var formats = await core.SupportedFormatsAsync(null, null).WaitAsync(Timeout * 3);
            check(formats.Tabs.Count == 5 && formats.Tabs.All(tab => tab.Rows.Count > 0), "the core's supported formats have five tabs with rows");
            var video = formats.Tabs.FirstOrDefault(tab => tab.Kind == FormatTabKind.Video)?.Rows ?? Array.Empty<FormatRow>();
            check(video.Any(row => row.Support == FormatSupport.Plays),
                $"a headless libmpv reports the codecs it decodes ({video.Count(row => row.Support == FormatSupport.Plays)} play)");
            var hardware = await DecoderTest.GetAsync().WaitAsync(Timeout);
            Console.WriteLine(hardware is null
                ? "     the graphics card test did not run"
                : $"     the graphics card test found {hardware.Profiles.Count} decoder profile(s)");

            window.ShowSupportedFormats();
            check(window.ShownPage is SupportedFormatsPage, $"the supported formats page opens ({window.ShownPage?.GetType().Name})");
            if (window.ShownPage is not SupportedFormatsPage page)
            {
                return;
            }
            await page.LoadAsync(core).WaitAsync(Timeout * 3);
            check(page.TabCount == 5, $"it has five tabs ({page.TabCount})");
            for (var index = 0; index < page.TabCount; index++)
            {
                var (header, rows) = page.ShowTabForTest(index);
                check(rows > 0 && header.Length > 0, $"tab {index + 1} has rows under \"{header}\" ({rows})");
            }
            window.ShowSettings();
        }
        catch (Exception error)
        {
            check(false, $"the supported formats check failed: {error.GetType().Name}");
        }
    }

    /// <summary>The opacity of a solid brush; -1 for no brush or another kind.</summary>
    private static int Alpha(Brush? brush) => brush is SolidColorBrush solid ? solid.Color.A : -1;

    /// <summary>
    /// The core on a data folder of its own, with secrets in memory and no network: a first open creates the settings, a second loads
    /// them, the saved servers arrive on the UI thread, and an empty address fails as the core says. The folder is deleted after.
    /// </summary>
    private static async Task StorageChecksAsync(DispatcherQueue ui, Action<bool, string> check)
    {
        var folder = Path.Combine(Path.GetTempPath(), $"current-smoke-{Guid.NewGuid():N}");
        try
        {
            using (var first = await CoreHost.OpenAsync(folder, Path.Combine(folder, "Cache"), "smoke", SecretStore.Memory))
            {
                check(first.LoadReport is LoadReport.Created, $"a new data folder is created ({first.LoadReport?.GetType().Name})");
            }
            using var core = await CoreHost.OpenAsync(folder, Path.Combine(folder, "Cache"), "smoke", SecretStore.Memory);
            check(core.LoadReport is LoadReport.Loaded, $"it is loaded again ({core.LoadReport?.GetType().Name})");

            var listed = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
            using (var servers = core.ObserveServers(new DispatcherQueueUi(ui)))
            {
                // The first delivery is a Reset, even of an empty list.
                servers.Items.CollectionChanged += (_, _) => listed.TrySetResult(ui.HasThreadAccess);
                var arrived = await Task.WhenAny(listed.Task, Task.Delay(Timeout)) == listed.Task;
                check(arrived && listed.Task.Result, "the saved servers arrived on the UI thread");
                check(servers.Items.Count == 0, "no saved servers");
            }

            using var flow = core.AddServer();
            await flow.ProbeAsync("");
            check(
                flow.State is AddServerState.Failed { Problem: ProbeProblem.InvalidAddress { Reason: AddressProblem.Empty } },
                "an empty address fails without a request");
        }
        finally
        {
            try
            {
                Directory.Delete(folder, true);
            }
            catch (Exception error) when (error is IOException or UnauthorizedAccessException)
            {
                // A file the core still closes; the temp folder is the system's to clean.
            }
        }
    }

    private static Task<bool> WaitUntilAsync(Func<bool> condition) => WaitUntilAsync(condition, Timeout);

    private static async Task<bool> WaitUntilAsync(Func<bool> condition, TimeSpan timeout)
    {
        var deadline = DateTime.UtcNow + timeout;
        while (!condition())
        {
            if (DateTime.UtcNow > deadline)
            {
                return false;
            }
            await Task.Delay(50);
        }
        return true;
    }
}
