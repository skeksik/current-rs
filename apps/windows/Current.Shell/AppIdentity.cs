using System.Runtime.InteropServices;

namespace Current.Shell;

/// <summary>How Windows identifies the app, and where its files go (docs/notes/windows-shell.md).</summary>
internal static class AppIdentity
{
    /// <summary>
    /// The explicit AppUserModelID, in the form CompanyName.ProductName with no version, so that upgrades keep it ("Application User
    /// Model IDs", learn.microsoft.com, read 2026-10-22). The installer puts the same ID on the Start menu shortcut.
    /// </summary>
    public const string AppUserModelId = "Skeksik.Current";

    /// <summary>The single-instance key for <c>AppInstance.FindOrRegisterForKey</c>.</summary>
    public const string InstanceKey = "main";

    /// <summary>
    /// The app's data folder, %LOCALAPPDATA%\Current: never the program folder (%LOCALAPPDATA%\Programs\Current), and never touched by
    /// the installer, so uninstall and upgrade keep it. Nothing writes here yet; when storage arrives (M1), the app passes this path to
    /// the core when it creates it.
    /// </summary>
    public static string DataFolder { get; } =
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Current");

    /// <summary>Must run before the process shows any UI ("Where to Assign an AppUserModelID", same page).</summary>
    public static void ApplyToProcess() => SetCurrentProcessExplicitAppUserModelID(AppUserModelId);

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    private static extern void SetCurrentProcessExplicitAppUserModelID(string appId);
}
