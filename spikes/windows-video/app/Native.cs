using System.Runtime.InteropServices;
using Microsoft.UI.Xaml.Controls;
using WinRT;

namespace VideoSpike;

// Raw COM vtable calls for the few DXGI / XAML interop methods the spike needs (no extra packages).
public static unsafe class Native
{
    // microsoft.ui.xaml.media.dxinterop.idl (Microsoft.WindowsAppSDK.WinUI 2.3.9)
    static Guid IID_ISwapChainPanelNative = new("63aad0b8-7c24-40ff-85a8-640d944cc325");
    static Guid IID_IDXGISwapChain2 = new("a8be2ac4-199f-4946-b331-79599fb98de7");
    static Guid IID_IDXGIFactory1 = new("770aae78-f26f-4dba-a829-253c83d1b387");
    static Guid IID_IDXGIOutput6 = new("068346e8-aaec-4b84-add7-137f513f77a1");

    static void* Slot(IntPtr obj, int index) => (*(void***)obj)[index];

    static void Check(int hr, string what) { if (hr < 0) throw new COMException(what, hr); }

    /// <summary>ISwapChainPanelNative::SetSwapChain. UI thread only. Pass IntPtr.Zero to detach.</summary>
    public static void SetSwapChain(SwapChainPanel panel, IntPtr swapChain)
    {
        var unk = ((IWinRTObject)panel).NativeObject.ThisPtr;
        Check(Marshal.QueryInterface(unk, in IID_ISwapChainPanelNative, out var native), "QI ISwapChainPanelNative");
        try
        {
            var set = (delegate* unmanaged[Stdcall]<IntPtr, IntPtr, int>)Slot(native, 3);
            Check(set(native, swapChain), "SetSwapChain");
        }
        finally { Marshal.Release(native); }
    }

    /// <summary>IDXGISwapChain2::SetMatrixTransform: map buffer pixels 1:1 to physical pixels at a CompositionScale.</summary>
    public static void SetMatrixTransform(IntPtr swapChain, float scaleX, float scaleY)
    {
        Check(Marshal.QueryInterface(swapChain, in IID_IDXGISwapChain2, out var sc2), "QI IDXGISwapChain2");
        try
        {
            var m = stackalloc float[6] { 1f / scaleX, 0, 0, 1f / scaleY, 0, 0 };
            var set = (delegate* unmanaged[Stdcall]<IntPtr, float*, int>)Slot(sc2, 34);
            Check(set(sc2, m), "SetMatrixTransform");
        }
        finally { Marshal.Release(sc2); }
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct SwapChainDesc1
    {
        public uint Width, Height, Format, Stereo, SampleCount, SampleQuality, BufferUsage, BufferCount, Scaling, SwapEffect,
            AlphaMode, Flags;
        public override string ToString() =>
            $"{Width}x{Height} {FormatName(Format)} buffers={BufferCount} swap={SwapEffect} alpha={AlphaMode} scaling={Scaling}";
    }

    public static string FormatName(uint f) => f switch
    {
        10 => "R16G16B16A16_FLOAT", 24 => "R10G10B10A2_UNORM", 28 => "R8G8B8A8_UNORM", 87 => "B8G8R8A8_UNORM",
        _ => $"DXGI_FORMAT({f})",
    };

    /// <summary>IDXGISwapChain1::GetDesc1 (slot 18).</summary>
    public static SwapChainDesc1 GetDesc1(IntPtr swapChain)
    {
        SwapChainDesc1 d;
        var get = (delegate* unmanaged[Stdcall]<IntPtr, SwapChainDesc1*, int>)Slot(swapChain, 18);
        Check(get(swapChain, &d), "GetDesc1");
        return d;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct OutputDesc1
    {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string DeviceName;
        public int Left, Top, Right, Bottom;
        public int AttachedToDesktop, Rotation;
        public IntPtr Monitor;
        public uint BitsPerColor, ColorSpace;
        public float RedX, RedY, GreenX, GreenY, BlueX, BlueY, WhiteX, WhiteY;
        public float MinLuminance, MaxLuminance, MaxFullFrameLuminance;
    }

    public static string ColorSpaceName(uint cs) => cs switch
    {
        0 => "RGB_FULL_G22_NONE_P709 (SDR)", 1 => "RGB_FULL_G10_NONE_P709 (scRGB)", 12 => "RGB_FULL_G2084_NONE_P2020 (HDR10)",
        _ => $"DXGI_COLOR_SPACE({cs})",
    };

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct DevMode
    {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string DeviceName;
        public ushort SpecVersion, DriverVersion, Size, DriverExtra;
        public uint Fields;
        public int PositionX, PositionY;
        public uint DisplayOrientation, DisplayFixedOutput;
        public short Color, Duplex, YResolution, TTOption, Collate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string FormName;
        public ushort LogPixels;
        public uint BitsPerPel, PelsWidth, PelsHeight, DisplayFlags, DisplayFrequency;
        public uint IcmMethod, IcmIntent, MediaType, DitherType, Reserved1, Reserved2, PanningWidth, PanningHeight;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    static extern bool EnumDisplaySettingsW(string deviceName, int modeNum, ref DevMode mode);

    /// <summary>Current refresh rate (integer Hz) of a GDI display device such as \\.\DISPLAY1.</summary>
    public static int RefreshRate(string deviceName)
    {
        var m = new DevMode { Size = (ushort)Marshal.SizeOf<DevMode>() };
        return EnumDisplaySettingsW(deviceName, -1 /* ENUM_CURRENT_SETTINGS */, ref m) ? (int)m.DisplayFrequency : 0;
    }

    [DllImport("dxgi.dll")] static extern int CreateDXGIFactory1(in Guid riid, out IntPtr factory);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr hwnd, uint flags);

    /// <summary>The DXGI output (and its adapter's name) showing the given monitor, via IDXGIOutput6::GetDesc1.</summary>
    public static (OutputDesc1 Desc, string Adapter)? OutputForMonitor(IntPtr hmonitor)
    {
        Check(CreateDXGIFactory1(in IID_IDXGIFactory1, out var factory), "CreateDXGIFactory1");
        try
        {
            var enumAdapters = (delegate* unmanaged[Stdcall]<IntPtr, uint, IntPtr*, int>)Slot(factory, 12);
            var adesc = stackalloc byte[512];
            var buf = stackalloc byte[256];
            for (uint a = 0; ; a++)
            {
                IntPtr adapter;
                if (enumAdapters(factory, a, &adapter) < 0) return null;
                try
                {
                    ((delegate* unmanaged[Stdcall]<IntPtr, byte*, int>)Slot(adapter, 8))(adapter, adesc);
                    var adapterName = new string((char*)adesc).TrimEnd('\0');
                    var enumOutputs = (delegate* unmanaged[Stdcall]<IntPtr, uint, IntPtr*, int>)Slot(adapter, 7);
                    for (uint o = 0; ; o++)
                    {
                        IntPtr output;
                        if (enumOutputs(adapter, o, &output) < 0) break;
                        try
                        {
                            if (Marshal.QueryInterface(output, in IID_IDXGIOutput6, out var o6) < 0) continue;
                            try
                            {
                                Check(((delegate* unmanaged[Stdcall]<IntPtr, byte*, int>)Slot(o6, 27))(o6, buf), "GetDesc1");
                                var desc = Marshal.PtrToStructure<OutputDesc1>((IntPtr)buf);
                                if (desc.Monitor == hmonitor) return (desc, adapterName);
                            }
                            finally { Marshal.Release(o6); }
                        }
                        finally { Marshal.Release(output); }
                    }
                }
                finally { Marshal.Release(adapter); }
            }
        }
        finally { Marshal.Release(factory); }
    }
}
