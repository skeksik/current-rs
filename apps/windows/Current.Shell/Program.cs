using System.Diagnostics;
using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;

namespace Current.Shell;

/// <summary>
/// The entry point, replacing the one XAML generates (DISABLE_XAML_GENERATED_MAIN). It names the process for the shell, keeps the app to
/// one instance and starts XAML. The single-instance code follows "How to create a single-instanced WinUI app with C#"
/// (learn.microsoft.com, ms.date 2026-09-10, read 2026-10-22).
/// </summary>
public static class Program
{
    private static int _exitCode;

    [STAThread]
    private static int Main(string[] args)
    {
        // Before any UI: the taskbar, Start and the media flyout group the process under this ID (docs/notes/windows-shell.md). A
        // packaged build gets its identity from the package instead.
        if (!AppIdentity.IsPackaged)
        {
            AppIdentity.ApplyToProcess();
        }

        WinRT.ComWrappersSupport.InitializeComWrappers();

        var smoke = args.Contains(SmokeRun.Argument);
        // A smoke run never takes the single-instance key, so it can run next to the app and is never redirected.
        if (!smoke && RedirectedToFirstInstance())
        {
            return 0;
        }
        if (!smoke)
        {
            AppIdentity.MarkRunning();
        }

        Application.Start(p =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            _ = new App(smoke);
        });
        return _exitCode;
    }

    /// <summary>What <see cref="Main"/> returns once XAML has stopped. A smoke run sets it.</summary>
    internal static void SetExitCode(int code) => _exitCode = code;

    /// <summary>
    /// True when another instance holds the key: this launch's activation went to it and this process should exit. Otherwise this
    /// process now holds the key and later launches are redirected here.
    /// </summary>
    private static bool RedirectedToFirstInstance()
    {
        var activation = AppInstance.GetCurrent().GetActivatedEventArgs();
        var keyInstance = AppInstance.FindOrRegisterForKey(AppIdentity.InstanceKey);
        if (keyInstance.IsCurrent)
        {
            // Raised on a background thread when a later launch redirects here.
            keyInstance.Activated += (_, _) => App.BringToFront();
            return false;
        }
        RedirectActivationTo(activation, keyInstance);
        return true;
    }

    // Redirect on another thread and wait without blocking the STA's message pumping, as the page above does.
    private static void RedirectActivationTo(AppActivationArguments activation, AppInstance keyInstance)
    {
        var redirected = CreateEvent(IntPtr.Zero, true, false, null);
        Task.Run(() =>
        {
            keyInstance.RedirectActivationToAsync(activation).AsTask().Wait();
            SetEvent(redirected);
        });
        const uint CwmoDefault = 0;
        const uint Infinite = 0xFFFFFFFF;
        _ = CoWaitForMultipleObjects(CwmoDefault, Infinite, 1, [redirected], out _);

        // This process was just launched by the user, so it may hand the foreground to the first instance's window.
        var first = Process.GetProcessById((int)keyInstance.ProcessId);
        SetForegroundWindow(first.MainWindowHandle);
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr CreateEvent(IntPtr lpEventAttributes, bool bManualReset, bool bInitialState, string? lpName);

    [DllImport("kernel32.dll")]
    private static extern bool SetEvent(IntPtr hEvent);

    [DllImport("ole32.dll")]
    private static extern uint CoWaitForMultipleObjects(
        uint dwFlags, uint dwMilliseconds, ulong nHandles, IntPtr[] pHandles, out uint dwIndex);

    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(IntPtr hWnd);
}
