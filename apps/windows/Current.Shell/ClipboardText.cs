using System.Runtime.InteropServices;
using Windows.ApplicationModel.DataTransfer;

namespace Current.Shell;

/// <summary>
/// Puts text on the clipboard so that it stays there after the app exits. The clipboard is shared: while another process (clipboard
/// history, for one) has it open, Windows refuses it with CLIPBRD_E_CANT_OPEN ("OpenClipboard fails if another window has the
/// clipboard open", OpenClipboard, learn.microsoft.com, read 2026-10-22). So a refused attempt is retried a few times before giving
/// up. Microsoft's Clipboard.Flush page gives no retry guidance; the attempts and delay here are our own choice (not verified).
/// </summary>
internal static class ClipboardText
{
    private const int ClipboardCantOpen = unchecked((int)0x800401D0);
    private const int Attempts = 5;
    private static readonly TimeSpan RetryDelay = TimeSpan.FromMilliseconds(100);

    /// <summary>True when the text is on the clipboard; false when the clipboard stayed busy. Call on the UI thread.</summary>
    public static async Task<bool> TryCopyAsync(string text)
    {
        for (var attempt = 1; attempt <= Attempts; attempt++)
        {
            try
            {
                var package = new DataPackage();
                package.SetText(text);
                Clipboard.SetContent(package);
                // Renders the content now, so it survives the app.
                Clipboard.Flush();
                return true;
            }
            catch (COMException error) when (error.HResult == ClipboardCantOpen)
            {
                if (attempt < Attempts)
                {
                    await Task.Delay(RetryDelay);
                }
            }
        }
        return false;
    }
}
