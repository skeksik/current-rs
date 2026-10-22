using System.Runtime.InteropServices;
using Windows.Storage;

namespace Current.Shell;

/// <summary>
/// How Windows identifies the app, and where its files go (docs/notes/windows-shell.md). The app runs unpackaged today (the Inno Setup
/// installer, ADR 0009) and may also ship as an MSIX package later, so nothing here assumes either.
/// </summary>
internal static class AppIdentity
{
    /// <summary>
    /// The explicit AppUserModelID of the unpackaged app, in the form CompanyName.ProductName with no version, so that upgrades keep it
    /// ("Application User Model IDs", learn.microsoft.com, read 2026-10-22). The installer puts the same ID on the Start menu shortcut.
    /// A packaged app gets its ID from the package and must not set one.
    /// </summary>
    public const string AppUserModelId = "Skeksik.Current";

    /// <summary>The single-instance key for <c>AppInstance.FindOrRegisterForKey</c>.</summary>
    public const string InstanceKey = "main";

    /// <summary>
    /// A named mutex the app holds while it runs, so that the installer and uninstaller can see it (Inno Setup's AppMutex, set to the
    /// same name in installer/Current.iss).
    /// </summary>
    public const string RunningMutexName = "Skeksik.Current.Running";

    private static Mutex? _running;

    /// <summary>Creates <see cref="RunningMutexName"/> and keeps it for the life of the process; Windows releases it on exit.</summary>
    public static void MarkRunning() => _running ??= new Mutex(false, RunningMutexName);

    /// <summary>True when the process runs with package identity (an MSIX install).</summary>
    public static bool IsPackaged { get; } = HasPackageIdentity();

    /// <summary>
    /// The app's data folder. Unpackaged: %LOCALAPPDATA%\Current through the known-folder API, never the program folder, and never
    /// touched by the installer, so uninstall and upgrade keep it. Packaged: the package's local folder, which Windows owns. Nothing
    /// writes here yet; when storage arrives (M1), the app passes this path to the core when it creates it.
    /// </summary>
    public static string DataFolder { get; } = IsPackaged
        ? ApplicationData.Current.LocalFolder.Path
        : Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Current");

    /// <summary>Must run before the process shows any UI ("Where to Assign an AppUserModelID", same page).</summary>
    public static void ApplyToProcess() => SetCurrentProcessExplicitAppUserModelID(AppUserModelId);

    private static bool HasPackageIdentity()
    {
        // With a zero-length buffer the call fails with ERROR_INSUFFICIENT_BUFFER when the process has package identity, and with
        // APPMODEL_ERROR_NO_PACKAGE when it has none ("GetCurrentPackageFullName", learn.microsoft.com, read 2026-10-22).
        const int ErrorInsufficientBuffer = 122;
        var length = 0;
        return GetCurrentPackageFullName(ref length, IntPtr.Zero) == ErrorInsufficientBuffer;
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    private static extern void SetCurrentProcessExplicitAppUserModelID(string appId);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetCurrentPackageFullName(ref int packageFullNameLength, IntPtr packageFullName);
}
