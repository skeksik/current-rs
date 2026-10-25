using System.Runtime.InteropServices;
using Microsoft.UI.Xaml.Controls;
using WinRT;

namespace Current.Shell.Player;

/// <summary>
/// The few Win32, DXGI and XAML interop calls the player needs (ADR 0007), as the D2 spike made them (docs/notes/windows-video.md;
/// prototype commit 181dce5, Native.cs), through COM vtables and delegates, so no package and no unsafe code.
/// </summary>
internal static class PlayerNative
{
    /// <summary><c>ISwapChainPanelNative</c>: <c>include/microsoft.ui.xaml.media.dxinterop.idl</c> in Microsoft.WindowsAppSDK.WinUI 2.3.9
    /// (ADR 0007, "Sources").</summary>
    private static readonly Guid SwapChainPanelNative = new("63aad0b8-7c24-40ff-85a8-640d944cc325");

    /// <summary><c>IDXGISwapChain2</c> (<c>dxgi1_3.h</c>, Windows SDK).</summary>
    private static readonly Guid SwapChain2 = new("a8be2ac4-199f-4946-b331-79599fb98de7");

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int SetSwapChainMethod(IntPtr self, IntPtr swapChain);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int SetMatrixTransformMethod(IntPtr self, ref Matrix3x2 matrix);

    /// <summary><c>DXGI_MATRIX_3X2_F</c>.</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct Matrix3x2
    {
        public float M11, M12, M21, M22, M31, M32;
    }

    /// <summary>The method in vtable slot <paramref name="slot"/> of the COM object <paramref name="self"/>.</summary>
    private static T Method<T>(IntPtr self, int slot) where T : Delegate =>
        Marshal.GetDelegateForFunctionPointer<T>(Marshal.ReadIntPtr(Marshal.ReadIntPtr(self), slot * IntPtr.Size));

    /// <summary>
    /// <c>ISwapChainPanelNative::SetSwapChain</c> (slot 3, after IUnknown's three): the panel takes its own reference. Zero detaches the
    /// panel, which gives its reference back. UI thread only.
    /// </summary>
    public static void SetSwapChain(SwapChainPanel panel, IntPtr swapChain)
    {
        var unknown = ((IWinRTObject)panel).NativeObject.ThisPtr;
        Marshal.ThrowExceptionForHR(Marshal.QueryInterface(unknown, in SwapChainPanelNative, out var native));
        try
        {
            Marshal.ThrowExceptionForHR(Method<SetSwapChainMethod>(native, 3)(native, swapChain));
        }
        finally
        {
            Marshal.Release(native);
        }
    }

    /// <summary>
    /// <c>IDXGISwapChain2::SetMatrixTransform</c> with 1/CompositionScale, so the buffer's pixels map 1:1 to the screen's (ADR 0007).
    /// Slot 34: IUnknown 3, IDXGIObject 4, IDXGIDeviceSubObject 1, IDXGISwapChain 10, IDXGISwapChain1 11, then IDXGISwapChain2's
    /// SetSourceSize, GetSourceSize, SetMaximumFrameLatency, GetMaximumFrameLatency, GetFrameLatencyWaitableObject, SetMatrixTransform
    /// (<c>dxgi1_3.h</c>; the spike's call).
    /// </summary>
    public static void SetMatrixTransform(IntPtr swapChain, float scaleX, float scaleY)
    {
        Marshal.ThrowExceptionForHR(Marshal.QueryInterface(swapChain, in SwapChain2, out var chain));
        try
        {
            var matrix = new Matrix3x2 { M11 = 1f / scaleX, M22 = 1f / scaleY };
            Marshal.ThrowExceptionForHR(Method<SetMatrixTransformMethod>(chain, 34)(chain, ref matrix));
        }
        finally
        {
            Marshal.Release(chain);
        }
    }

    private const uint MonitorDefaultToNearest = 2;

    [DllImport("user32.dll")]
    public static extern IntPtr MonitorFromWindow(IntPtr window, uint flags);

    public static IntPtr MonitorOf(IntPtr window) => MonitorFromWindow(window, MonitorDefaultToNearest);

    /// <summary><c>MONITORINFOEXW</c>.</summary>
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct MonitorInfo
    {
        public int Size;
        public int Left, Top, Right, Bottom;
        public int WorkLeft, WorkTop, WorkRight, WorkBottom;
        public uint Flags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
        public string Device;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetMonitorInfoW(IntPtr monitor, ref MonitorInfo info);

    [StructLayout(LayoutKind.Sequential)]
    private struct Luid
    {
        public uint Low;
        public int High;
    }

    /// <summary><c>DISPLAYCONFIG_PATH_INFO</c> (72 bytes): only the fields read here are named.</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct PathInfo
    {
        public Luid SourceAdapter;
        public uint SourceId;
        public uint SourceModeIndex;
        public uint SourceStatus;
        public Luid TargetAdapter;
        public uint TargetId;
        public uint TargetModeIndex;
        public uint OutputTechnology;
        public uint Rotation;
        public uint Scaling;
        public uint RefreshNumerator;
        public uint RefreshDenominator;
        public uint ScanLineOrdering;
        public int TargetAvailable;
        public uint TargetStatus;
        public uint Flags;
    }

    /// <summary><c>DISPLAYCONFIG_SOURCE_DEVICE_NAME</c>: header (type, size, adapter, id), then the GDI device name.</summary>
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct SourceDeviceName
    {
        public uint Type;
        public uint Size;
        public Luid Adapter;
        public uint Id;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
        public string Device;
    }

    private const uint QdcOnlyActivePaths = 2;
    private const uint GetSourceName = 1;
    private const int ModeInfoSize = 64;

    [DllImport("user32.dll")]
    private static extern int GetDisplayConfigBufferSizes(uint flags, out uint paths, out uint modes);

    [DllImport("user32.dll")]
    private static extern int QueryDisplayConfig(uint flags, ref uint pathCount, [Out] PathInfo[] paths, ref uint modeCount, IntPtr modes,
        IntPtr topology);

    [DllImport("user32.dll")]
    private static extern int DisplayConfigGetDeviceInfo(ref SourceDeviceName request);

    /// <summary>
    /// The refresh rate of <paramref name="monitor"/> as the display path reports it, a fraction (59.94 rather than 60), or 0 if it cannot
    /// be read. The monitor's GDI name ("\\.\DISPLAY1", <c>GetMonitorInfoW</c>) is matched to an active path's source name
    /// (<c>QueryDisplayConfig</c>, <c>DisplayConfigGetDeviceInfo</c> with DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME); the path's
    /// <c>targetInfo.refreshRate</c> is the rate (wingdi.h, Windows SDK; the API pages were not re-read for this).
    /// </summary>
    public static double RefreshRate(IntPtr monitor)
    {
        var info = new MonitorInfo { Size = Marshal.SizeOf<MonitorInfo>() };
        if (!GetMonitorInfoW(monitor, ref info))
        {
            return 0;
        }
        if (GetDisplayConfigBufferSizes(QdcOnlyActivePaths, out var pathCount, out var modeCount) != 0)
        {
            return 0;
        }
        var paths = new PathInfo[pathCount];
        var modes = Marshal.AllocHGlobal((int)(modeCount * ModeInfoSize));
        try
        {
            if (QueryDisplayConfig(QdcOnlyActivePaths, ref pathCount, paths, ref modeCount, modes, IntPtr.Zero) != 0)
            {
                return 0;
            }
        }
        finally
        {
            Marshal.FreeHGlobal(modes);
        }
        foreach (var path in paths.Take((int)pathCount))
        {
            var name = new SourceDeviceName
            {
                Type = GetSourceName,
                Size = (uint)Marshal.SizeOf<SourceDeviceName>(),
                Adapter = path.SourceAdapter,
                Id = path.SourceId,
                Device = "",
            };
            if (DisplayConfigGetDeviceInfo(ref name) == 0 && name.Device == info.Device && path.RefreshDenominator != 0)
            {
                return (double)path.RefreshNumerator / path.RefreshDenominator;
            }
        }
        return 0;
    }

    /// <summary>
    /// <c>ShowCursor</c>: the cursor shows while the thread's display count is 0 or more. The player calls it in pairs only (hide once,
    /// show once), so the count goes back where it was.
    /// </summary>
    [DllImport("user32.dll")]
    public static extern int ShowCursor([MarshalAs(UnmanagedType.Bool)] bool show);
}
