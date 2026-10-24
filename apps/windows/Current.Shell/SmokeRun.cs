using Current.Bindings;
using Current.Shell.Pages;
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
            Check(true, "home, add-server, sign-in and settings pages built");

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
            using (var first = await CoreHost.OpenAsync(folder, "smoke", SecretStore.Memory))
            {
                check(first.LoadReport is LoadReport.Created, $"a new data folder is created ({first.LoadReport?.GetType().Name})");
            }
            using var core = await CoreHost.OpenAsync(folder, "smoke", SecretStore.Memory);
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

    private static async Task<bool> WaitUntilAsync(Func<bool> condition)
    {
        var deadline = DateTime.UtcNow + Timeout;
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
