using System.Runtime.InteropServices;
using System.Text;

namespace VideoSpike;

// Minimal P/Invoke over libmpv's client API (mpv/client.h, API 2.5). The real engine is Rust (crates/mpv, ADR 0002);
// this only records what the UI needs from it. Thread rules: every mpv_* call is thread-safe; events are read on one
// dedicated thread and handed to the UI thread by the caller.
public sealed class Mpv : IDisposable
{
    const string Lib = "mpv-2.dll";

    public enum Format { None = 0, String = 1, OsdString = 2, Flag = 3, Int64 = 4, Double = 5, Node = 6 }

    public enum EventId
    {
        None = 0, Shutdown = 1, LogMessage = 2, GetPropertyReply = 3, SetPropertyReply = 4, CommandReply = 5,
        StartFile = 6, EndFile = 7, FileLoaded = 8, ClientMessage = 16, VideoReconfig = 17, AudioReconfig = 18,
        Seek = 20, PlaybackRestart = 21, PropertyChange = 22, QueueOverflow = 24, Hook = 25,
    }

    [StructLayout(LayoutKind.Sequential)]
    struct NativeEvent { public EventId Id; public int Error; public ulong ReplyUserdata; public IntPtr Data; }

    [StructLayout(LayoutKind.Sequential)]
    struct NativeProperty { public IntPtr Name; public Format Format; public IntPtr Data; }

    [StructLayout(LayoutKind.Sequential)]
    struct NativeLog { public IntPtr Prefix; public IntPtr Level; public IntPtr Text; public int LogLevel; }

    [StructLayout(LayoutKind.Sequential)]
    struct NativeEndFile { public int Reason; public int Error; public long EntryId; public long InsertId; public int InsertNum; }

    [DllImport(Lib)] static extern ulong mpv_client_api_version();
    [DllImport(Lib)] static extern IntPtr mpv_create();
    [DllImport(Lib)] static extern int mpv_initialize(IntPtr ctx);
    [DllImport(Lib)] static extern void mpv_terminate_destroy(IntPtr ctx);
    [DllImport(Lib)] static extern IntPtr mpv_error_string(int error);
    [DllImport(Lib)] static extern void mpv_free(IntPtr data);
    [DllImport(Lib)] static extern int mpv_set_option_string(IntPtr ctx, byte[] name, byte[] data);
    [DllImport(Lib)] static extern int mpv_set_property_string(IntPtr ctx, byte[] name, byte[] data);
    [DllImport(Lib)] static extern int mpv_set_property_async(IntPtr ctx, ulong userdata, byte[] name, Format format, ref IntPtr data);
    [DllImport(Lib)] static extern int mpv_get_property(IntPtr ctx, byte[] name, Format format, out long data);
    [DllImport(Lib)] static extern int mpv_get_property(IntPtr ctx, byte[] name, Format format, out double data);
    [DllImport(Lib)] static extern IntPtr mpv_get_property_string(IntPtr ctx, byte[] name);
    [DllImport(Lib)] static extern int mpv_command(IntPtr ctx, IntPtr[] args);
    [DllImport(Lib)] static extern int mpv_command_async(IntPtr ctx, ulong userdata, IntPtr[] args);
    [DllImport(Lib)] static extern int mpv_observe_property(IntPtr ctx, ulong userdata, byte[] name, Format format);
    [DllImport(Lib)] static extern int mpv_request_log_messages(IntPtr ctx, byte[] minLevel);
    [DllImport(Lib)] static extern IntPtr mpv_wait_event(IntPtr ctx, double timeout);
    [DllImport(Lib)] static extern void mpv_wakeup(IntPtr ctx);

    static byte[] U8(string s) => Encoding.UTF8.GetBytes(s + "\0");
    static string? Str(IntPtr p) => p == IntPtr.Zero ? null : Marshal.PtrToStringUTF8(p);

    IntPtr _ctx;
    Thread? _thread;
    volatile bool _stopping;

    public static string ApiVersion { get { var v = mpv_client_api_version(); return $"{v >> 16}.{v & 0xFFFF}"; } }

    /// <summary>Raised on the event thread. Property values arrive as boxed long/double/bool/string (nodes as JSON strings).</summary>
    public event Action<string, object?>? PropertyChanged;
    public event Action<EventId, int>? Event; // id, error or end-file reason
    public event Action<string, string, string>? Log; // level, prefix, text

    public Mpv() { _ctx = mpv_create(); if (_ctx == IntPtr.Zero) throw new InvalidOperationException("mpv_create failed"); }

    public static string Error(int code) => Str(mpv_error_string(code)) ?? code.ToString();

    public int SetOption(string name, string value) => mpv_set_option_string(_ctx, U8(name), U8(value));

    public void Initialize()
    {
        var r = mpv_initialize(_ctx);
        if (r < 0) throw new InvalidOperationException("mpv_initialize: " + Error(r));
        _thread = new Thread(EventLoop) { IsBackground = true, Name = "mpv events" };
        _thread.Start();
    }

    public void RequestLog(string level) => mpv_request_log_messages(_ctx, U8(level));

    public int SetProperty(string name, string value) => mpv_set_property_string(_ctx, U8(name), U8(value));

    /// <summary>Non-blocking set for the UI thread (mpv copies the value before returning).</summary>
    public int SetPropertyAsync(string name, string value)
    {
        var p = Marshal.StringToCoTaskMemUTF8(value);
        try { return mpv_set_property_async(_ctx, 0, U8(name), Format.String, ref p); }
        finally { Marshal.FreeCoTaskMem(p); }
    }

    public string? GetString(string name)
    {
        var p = mpv_get_property_string(_ctx, U8(name));
        if (p == IntPtr.Zero) return null;
        try { return Str(p); } finally { mpv_free(p); }
    }

    public long? GetInt64(string name) => mpv_get_property(_ctx, U8(name), Format.Int64, out long v) >= 0 ? v : null;
    public double? GetDouble(string name) => mpv_get_property(_ctx, U8(name), Format.Double, out double v) >= 0 ? v : null;

    public void Observe(string name, Format format) => mpv_observe_property(_ctx, 0, U8(name), format);

    public int Command(params string[] args) => RunCommand(args, async: false);
    public int CommandAsync(params string[] args) => RunCommand(args, async: true);

    int RunCommand(string[] args, bool async)
    {
        var ptrs = new IntPtr[args.Length + 1];
        try
        {
            for (int i = 0; i < args.Length; i++) ptrs[i] = Marshal.StringToCoTaskMemUTF8(args[i]);
            return async ? mpv_command_async(_ctx, 0, ptrs) : mpv_command(_ctx, ptrs);
        }
        finally { foreach (var p in ptrs) if (p != IntPtr.Zero) Marshal.FreeCoTaskMem(p); }
    }

    void EventLoop()
    {
        while (!_stopping)
        {
            var ev = Marshal.PtrToStructure<NativeEvent>(mpv_wait_event(_ctx, -1));
            switch (ev.Id)
            {
                case EventId.None:
                    continue;
                case EventId.Shutdown:
                    Event?.Invoke(ev.Id, 0);
                    return;
                case EventId.LogMessage:
                    var log = Marshal.PtrToStructure<NativeLog>(ev.Data);
                    Log?.Invoke(Str(log.Level) ?? "", Str(log.Prefix) ?? "", Str(log.Text) ?? "");
                    break;
                case EventId.PropertyChange:
                    var prop = Marshal.PtrToStructure<NativeProperty>(ev.Data);
                    PropertyChanged?.Invoke(Str(prop.Name) ?? "", ReadValue(prop.Format, prop.Data));
                    break;
                case EventId.EndFile:
                    var end = Marshal.PtrToStructure<NativeEndFile>(ev.Data);
                    Event?.Invoke(ev.Id, end.Reason);
                    break;
                default:
                    Event?.Invoke(ev.Id, ev.Error);
                    break;
            }
        }
    }

    static object? ReadValue(Format format, IntPtr data)
    {
        if (data == IntPtr.Zero) return null;
        return format switch
        {
            Format.String or Format.OsdString => Str(Marshal.ReadIntPtr(data)),
            Format.Flag => Marshal.ReadInt32(data) != 0,
            Format.Int64 => Marshal.ReadInt64(data),
            Format.Double => BitConverter.Int64BitsToDouble(Marshal.ReadInt64(data)),
            _ => null,
        };
    }

    public void Dispose()
    {
        if (_ctx == IntPtr.Zero) return;
        // Stop the event thread first: no API call may race mpv_terminate_destroy on the same handle.
        _stopping = true;
        mpv_wakeup(_ctx);
        _thread?.Join(2000);
        // Blocks until mpv's threads (VO included) have stopped; the swap chain is released here.
        mpv_terminate_destroy(_ctx);
        _ctx = IntPtr.Zero;
    }
}
