using System.Diagnostics;

namespace Current.Shell.Gateway;

/// <summary>
/// The sign-in window's web profile: a new, empty WebView2 user data folder for each window, deleted after it closes, so nothing is
/// shared with the user's browser, between servers or between sign-ins (ADR 0017, "Its own profile"). Chosen over an InPrivate
/// controller because Microsoft documents what a user data folder holds and how to delete it ("WebView2 apps use user data folders
/// to store browser data, such as cookies, permissions, and cached resources"; "Each WebView2 session has exactly one UDF"; "If your
/// app doesn't reuse user data from app sessions, you can delete the UDF"; "wait for the browser process and all child processes to
/// exit before deleting the UDF": "Manage user data folders", learn.microsoft.com, ms.date 2026-06-26, read 2026-10-24), while the
/// InPrivate option's reference says only "Manage the controller's InPrivate mode" (CoreWebView2ControllerOptions.IsInPrivateModeEnabled,
/// learn.microsoft.com, read 2026-10-24). The folders live under the data folder, never next to the program, which an installed app
/// cannot write ("If distributing as unpackaged, in scenarios where the install directory is protected, you must specify a custom
/// UDF location", same page).
/// </summary>
internal static class SignInProfiles
{
    private static readonly TimeSpan ExitWait = TimeSpan.FromSeconds(10);

    private static string Root => Path.Combine(AppIdentity.DataFolder, "SignIn");

    /// <summary>The folders of windows still open (UI thread only): never deleted as leftovers.</summary>
    private static readonly HashSet<string> InUse = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>A new, empty folder for one sign-in window. Folders left by an earlier run are deleted first.</summary>
    public static string Create()
    {
        DeleteLeftovers();
        var folder = Path.Combine(Root, Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(folder);
        InUse.Add(folder);
        return folder;
    }

    /// <summary>Deletes a window's folder once its browser process has exited (Microsoft's order, see above).</summary>
    public static async Task DeleteAsync(string folder, uint browserProcess)
    {
        InUse.Remove(folder);
        try
        {
            if (browserProcess != 0)
            {
                using var process = Process.GetProcessById((int)browserProcess);
                using var wait = new CancellationTokenSource(ExitWait);
                await process.WaitForExitAsync(wait.Token);
            }
        }
        catch (Exception error) when (error is ArgumentException or InvalidOperationException or OperationCanceledException)
        {
            // Exited already, or still running after the wait: delete what can be deleted; the rest goes at the next sign-in.
        }
        TryDelete(folder);
    }

    /// <summary>Deletes the folders of earlier windows (an app that ended before it could). One still in use stays.</summary>
    public static void DeleteLeftovers()
    {
        try
        {
            if (!Directory.Exists(Root))
            {
                return;
            }
            foreach (var folder in Directory.EnumerateDirectories(Root).Where(folder => !InUse.Contains(folder)))
            {
                TryDelete(folder);
            }
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        {
            Trace.TraceError($"Current: listing old sign-in profiles failed: {error.GetType().Name}");
        }
    }

    private static void TryDelete(string folder)
    {
        try
        {
            Directory.Delete(folder, true);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        {
            // In use by a sign-in window that is still open, or its browser process: the next sign-in deletes it.
        }
    }
}
