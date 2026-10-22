using Current.Bindings;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

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

            Check(core.DiagnosticsText() is not null, "diagnostics text");

            // The window's XAML, Mica and title bar load without being shown.
            var window = new MainWindow();
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
