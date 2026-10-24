using Microsoft.Web.WebView2.Core;

namespace Current.Shell.Gateway;

/// <summary>
/// Whether the WebView2 Runtime the sign-in window needs is on this PC. Windows 11 includes it; "the vast majority of Windows 10
/// devices have the WebView2 Runtime installed already … A small number of Windows 10 devices don't", and Microsoft suggests
/// deploying it with the app or sending users to its download page ("Distribute your app and the WebView2 Runtime",
/// learn.microsoft.com, updated 2026-10-02, read 2026-10-24). Until the installer does the first (docs/notes/windows-shell.md), the app
/// does the second, in the window and on the page that would open it.
/// </summary>
internal static class WebViewRuntime
{
    /// <summary>Microsoft's page for end users, from the same distribution page.</summary>
    public const string DownloadPage = "https://developer.microsoft.com/microsoft-edge/webview2/consumer/";

    public const string Missing =
        "Signing in here needs the Microsoft Edge WebView2 Runtime, which isn't installed on this PC. Install it from Microsoft, then try again.";

    /// <summary>The installed runtime's version, or null when there is none (GetAvailableBrowserVersionString).</summary>
    public static string? Version()
    {
        try
        {
            return CoreWebView2Environment.GetAvailableBrowserVersionString() is { Length: > 0 } version ? version : null;
        }
        catch (Exception)
        {
            // The runtime is missing (WebView2's not-found error) or the loader failed: either way there is none to use.
            return null;
        }
    }

    public static bool IsInstalled() => Version() is not null;
}
