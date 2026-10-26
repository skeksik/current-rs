using System.Diagnostics;
using System.Runtime.InteropServices;
using Current.Bindings;

namespace Current.Shell.Player;

/// <summary>
/// Which video decoder profiles this PC's graphics card offers through Direct3D 11, for Settings' "Supported formats" (ADR 0021). Only
/// facts: every profile the card lists and whether the driver accepts NV12 (8-bit) and P010 (10-bit) output for it; which codec a
/// profile decodes is the core's to say. Platform integration, so it lives here; through COM vtables and delegates as PlayerNative does,
/// so no package and no unsafe code.
/// <list type="bullet">
/// <item>The adapter is DXGI's first (<c>IDXGIFactory1::EnumAdapters1(0)</c>), the one mpv decodes on. With <c>--d3d11-adapter</c>
/// unset, mpv's <c>mp_d3d11_create_present_device</c> gets no adapter from <c>mp_get_dxgi_adapter</c> and calls
/// <c>D3D11CreateDevice</c> with a null adapter and <c>D3D_DRIVER_TYPE_HARDWARE</c> (<c>video/out/gpu/d3d11_helpers.c</c>, mpv v0.41.0,
/// read 2026-10-26); "Pass NULL to use the default adapter, which is the first adapter that is enumerated by
/// IDXGIFactory1::EnumAdapters" (D3D11CreateDevice page, learn.microsoft.com, read 2026-10-26). d3d11va decodes on that device:
/// "Hardware decoders utilizing the D3D11 rendering abstraction's helper functionality to receive a device, such as D3D11VA ... will
/// be affected by this choice" (<c>--d3d11-adapter</c>, <c>DOCS/man/options.rst</c>, v0.41.0). The adapter's name is shown on the page
/// and never logged.</item>
/// <item><c>D3D11CreateDevice</c> with <c>D3D11_CREATE_DEVICE_VIDEO_SUPPORT</c> (0x800), then <c>ID3D11VideoDevice</c>'s
/// <c>GetVideoDecoderProfileCount</c>, <c>GetVideoDecoderProfile</c> ("To get the number of profiles that the driver supports, call
/// ID3D11VideoDevice::GetVideoDecoderProfileCount") and <c>CheckVideoDecoderFormat</c> ("Receives the value TRUE if the format is
/// supported, or FALSE otherwise"; "If the driver does not support the profile given in pDecoderProfile, the method returns
/// E_INVALIDARG"); learn.microsoft.com, d3d11.h reference pages, read 2026-10-26. <c>DXGI_FORMAT_NV12</c> is 103 and
/// <c>DXGI_FORMAT_P010</c> 104 (DXGI_FORMAT page, ms.date 2025-01-15, read 2026-10-26).</item>
/// <item>Vtable slots and IIDs from mingw-w64's <c>d3d11.idl</c> and <c>dxgi.idl</c> (read 2026-10-26); the Windows SDK's headers
/// were not read: <c>IDXGIFactory1</c> 770aae78-…: IUnknown 3, IDXGIObject 4, IDXGIFactory 5, then <c>EnumAdapters1</c> (12) and
/// <c>IsCurrent</c> (13); <c>IDXGIAdapter1</c>: IUnknown 3, IDXGIObject 4, IDXGIAdapter 3, then <c>GetDesc1</c> (10);
/// <c>ID3D11VideoDevice</c> 10ec4d5b-…: IUnknown 3, eight Create methods, then <c>GetVideoDecoderProfileCount</c> (11),
/// <c>GetVideoDecoderProfile</c> (12), <c>CheckVideoDecoderFormat</c> (13).</item>
/// </list>
/// Run once per app run, off the UI thread, and again when <c>IDXGIFactory1::IsCurrent</c> says the adapters changed ("FALSE, if a new
/// adapter is becoming available or the current adapter is going away", IsCurrent page, learn.microsoft.com, read 2026-10-26), which is
/// one call.
/// </summary>
internal static class DecoderTest
{
    private static readonly Guid Factory1 = new("770aae78-f26f-4dba-a829-253c83d1b387");
    private static readonly Guid VideoDevice = new("10ec4d5b-975a-4689-b9e4-d0aac30fe333");

    private const int FormatNv12 = 103;
    private const int FormatP010 = 104;
    private const uint CreateDeviceVideoSupport = 0x800;
    /// <summary><c>D3D_DRIVER_TYPE_UNKNOWN</c>, required with an adapter.</summary>
    private const int DriverTypeUnknown = 0;
    /// <summary><c>D3D11_SDK_VERSION</c> (mingw-w64's <c>d3d11.idl</c>: "const UINT D3D11_SDK_VERSION = 7").</summary>
    private const uint SdkVersion = 7;

    private static readonly object Gate = new();
    /// <summary>The factory the last result was found with, kept to ask <c>IsCurrent</c>; released when a new test replaces it.</summary>
    private static IntPtr _factory;
    private static HardwareDecodeFacts? _facts;
    private static Task<HardwareDecodeFacts?>? _running;

    [DllImport("dxgi.dll")]
    private static extern int CreateDXGIFactory1(ref Guid riid, out IntPtr factory);

    [DllImport("d3d11.dll")]
    private static extern int D3D11CreateDevice(
        IntPtr adapter, int driverType, IntPtr software, uint flags, IntPtr featureLevels, uint featureLevelCount, uint sdkVersion,
        out IntPtr device, IntPtr featureLevel, IntPtr immediateContext);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int EnumAdapters1Method(IntPtr self, uint index, out IntPtr adapter);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int IsCurrentMethod(IntPtr self);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int GetDesc1Method(IntPtr self, out AdapterDesc1 desc);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate uint GetVideoDecoderProfileCountMethod(IntPtr self);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int GetVideoDecoderProfileMethod(IntPtr self, uint index, out Guid profile);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int CheckVideoDecoderFormatMethod(IntPtr self, ref Guid profile, int format, out int supported);

    /// <summary><c>DXGI_ADAPTER_DESC1</c> (mingw-w64's <c>dxgi.idl</c>).</summary>
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct AdapterDesc1
    {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)]
        public string Description;
        public uint VendorId;
        public uint DeviceId;
        public uint SubSysId;
        public uint Revision;
        public nuint DedicatedVideoMemory;
        public nuint DedicatedSystemMemory;
        public nuint SharedSystemMemory;
        public uint LuidLowPart;
        public int LuidHighPart;
        public uint Flags;
    }

    private static T Method<T>(IntPtr self, int slot) where T : Delegate =>
        Marshal.GetDelegateForFunctionPointer<T>(Marshal.ReadIntPtr(Marshal.ReadIntPtr(self), slot * IntPtr.Size));

    /// <summary>
    /// The facts, from this run's test while the adapters are unchanged, else from a new test on a pool thread. Null when the test
    /// could not run (no Direct3D 11 video device, an error); the page then says so. Never throws.
    /// </summary>
    public static Task<HardwareDecodeFacts?> GetAsync()
    {
        lock (Gate)
        {
            if (_running is { IsCompleted: false } running)
            {
                return running;
            }
            if (_facts is { } facts && _factory != IntPtr.Zero && Method<IsCurrentMethod>(_factory, 13)(_factory) != 0)
            {
                return Task.FromResult<HardwareDecodeFacts?>(facts);
            }
            _running = Task.Run(Run);
            return _running;
        }
    }

    private static HardwareDecodeFacts? Run()
    {
        try
        {
            var iid = Factory1;
            Marshal.ThrowExceptionForHR(CreateDXGIFactory1(ref iid, out var factory));
            HardwareDecodeFacts? facts = null;
            try
            {
                facts = Test(factory);
            }
            finally
            {
                lock (Gate)
                {
                    if (_factory != IntPtr.Zero)
                    {
                        Marshal.Release(_factory);
                    }
                    _factory = factory;
                    _facts = facts;
                }
            }
            return facts;
        }
        catch (Exception error)
        {
            // The type only: an HRESULT message could name the device.
            Trace.TraceError($"Current: testing the graphics card's video decoders failed: {error.GetType().Name}");
            return null;
        }
    }

    private static HardwareDecodeFacts Test(IntPtr factory)
    {
        Marshal.ThrowExceptionForHR(Method<EnumAdapters1Method>(factory, 12)(factory, 0, out var adapter));
        try
        {
            Marshal.ThrowExceptionForHR(Method<GetDesc1Method>(adapter, 10)(adapter, out var desc));
            Marshal.ThrowExceptionForHR(D3D11CreateDevice(
                adapter, DriverTypeUnknown, IntPtr.Zero, CreateDeviceVideoSupport, IntPtr.Zero, 0, SdkVersion, out var device, IntPtr.Zero,
                IntPtr.Zero));
            try
            {
                Marshal.ThrowExceptionForHR(Marshal.QueryInterface(device, in VideoDevice, out var video));
                try
                {
                    return new HardwareDecodeFacts(desc.Description ?? "", Profiles(video));
                }
                finally
                {
                    Marshal.Release(video);
                }
            }
            finally
            {
                Marshal.Release(device);
            }
        }
        finally
        {
            Marshal.Release(adapter);
        }
    }

    private static List<DecoderProfileFact> Profiles(IntPtr video)
    {
        var count = Method<GetVideoDecoderProfileCountMethod>(video, 11)(video);
        var getProfile = Method<GetVideoDecoderProfileMethod>(video, 12);
        var checkFormat = Method<CheckVideoDecoderFormatMethod>(video, 13);
        bool Accepts(Guid profile, int format) => checkFormat(video, ref profile, format, out var supported) >= 0 && supported != 0;
        var profiles = new List<DecoderProfileFact>();
        for (uint index = 0; index < count; index++)
        {
            if (getProfile(video, index, out var profile) < 0)
            {
                continue;
            }
            profiles.Add(new DecoderProfileFact(profile.ToString("D"), Accepts(profile, FormatNv12), Accepts(profile, FormatP010)));
        }
        return profiles;
    }
}
