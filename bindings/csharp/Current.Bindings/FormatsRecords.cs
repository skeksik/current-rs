using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>
/// One of the graphics card's video decoder profiles (<c>ID3D11VideoDevice::GetVideoDecoderProfile</c>) and whether the driver accepts
/// NV12 (8-bit) and P010 (10-bit) output for it (<c>CheckVideoDecoderFormat</c>). The GUID as text, in any case.
/// </summary>
public sealed record DecoderProfileFact(string Guid, bool Nv12, bool P010);

/// <summary>
/// What the app's test of the graphics card found (ADR 0021): the adapter's name, shown on the page and never logged, and its decoder
/// profiles. Its text form leaves the name out.
/// </summary>
public sealed record HardwareDecodeFacts(string Adapter, IReadOnlyList<DecoderProfileFact> Profiles)
{
    public override string ToString() => $"HardwareDecodeFacts {{ Adapter = <redacted>, Profiles = {Profiles.Count} }}";
}

/// <summary>The tabs of Settings' "Supported formats" page, in order.</summary>
public enum FormatTabKind
{
    Video,
    Audio,
    Containers,
    Subtitles,
    Hdr,
}

/// <summary>The "On this PC" column.</summary>
public enum FormatSupport
{
    /// <summary>Decoded by the graphics card.</summary>
    Hardware,
    /// <summary>Decoded by the processor.</summary>
    Software,
    /// <summary>Plays (containers, subtitles; video when the graphics card could not be tested).</summary>
    Plays,
    /// <summary>Plays after the server repackages the stream (remux).</summary>
    ServerRepackages,
    /// <summary>Plays after the server converts it (transcode).</summary>
    ServerConverts,
    NotSupported,
    /// <summary>HDR shown in HDR.</summary>
    Hdr,
    /// <summary>HDR converted to SDR by the player.</summary>
    ToneMapped,
    /// <summary>Not known: the player could not be started to find out.</summary>
    Unknown,
    /// <summary>Plays in Windows' own player (ADR 0025, W6).</summary>
    Native,
}

/// <summary>A link a row offers: its words and its address (the Store page of an extension Windows' player lacks). The UI opens only
/// <c>ms-windows-store://pdp/?ProductId=</c> addresses.</summary>
public sealed record FormatAction(string Label, string Uri);

/// <summary>One row: the format's name, the column (its value and the word to show), and a note (empty when there is nothing to say).</summary>
public sealed record FormatRow(string Name, FormatSupport Support, string SupportLabel, string Notes, FormatAction? Action = null);

/// <summary>One tab: its title, its three column headers and its rows. Every word the page shows comes from the core.</summary>
public sealed record FormatTab(
    FormatTabKind Kind, string Title, string NameHeader, string SupportHeader, string NotesHeader, IReadOnlyList<FormatRow> Rows);

/// <summary>The page, built by the core: the text above the tabs, what the Hardware column rests on, and the tabs.</summary>
public sealed record SupportedFormats(string Description, string HardwareNote, IReadOnlyList<FormatTab> Tabs);

/// <summary>Settings' "Supported formats" page (ADR 0021), on <see cref="CoreHost"/>.</summary>
public static class FormatsExtensions
{
    /// <summary>
    /// The page for the facts the app found out: <paramref name="hardware"/> from its test of the graphics card (null when it could not
    /// run), <paramref name="display"/> for the display the window is on (null when unknown). The first call may start a headless engine
    /// for a moment (no window, no audio device), so call it off the UI thread's hot path; it never blocks the UI thread.
    /// </summary>
    /// <exception cref="OperationCanceledException"><paramref name="ct"/> was cancelled first.</exception>
    public static Task<SupportedFormats> SupportedFormatsAsync(
        this CoreHost core, HardwareDecodeFacts? hardware, DisplayFacts? display, CancellationToken ct = default) =>
        SupportedFormatsAsync(core, hardware, display, null, ct);

    /// <summary>
    /// As above, with <paramref name="native"/>, the answers to <see cref="PlatformEngineHost.WindowsCodecQueries"/>, when Windows' own
    /// player plays beside the app engine (null otherwise): each row then names the route that plays the format (ADR 0025, W6).
    /// </summary>
    public static async Task<SupportedFormats> SupportedFormatsAsync(
        this CoreHost core, HardwareDecodeFacts? hardware, DisplayFacts? display, IReadOnlyList<CodecQueryAnswer>? native,
        CancellationToken ct = default)
    {
        var page = await CoreCall.RunAsync(
            token => core.Core.SupportedFormats(
                hardware is null ? null : FormatsMap.ToFfi(hardware),
                display is null ? null : PlayerMap.ToFfi(display),
                native?.Select(answer => PlatformEngineMap.ToFfi(answer)).ToArray(),
                token),
            ct).ConfigureAwait(false);
        return FormatsMap.From(page);
    }
}

internal static class FormatsMap
{
    public static Ffi.HardwareDecodeFacts ToFfi(HardwareDecodeFacts facts) => new(
        facts.Adapter,
        facts.Profiles.Select(p => new Ffi.DecoderProfileFact(p.Guid, p.Nv12, p.P010)).ToArray());

    public static SupportedFormats From(Ffi.SupportedFormats page) => new(
        page.Description,
        page.HardwareNote,
        Array.ConvertAll(page.Tabs, tab => new FormatTab(
            From(tab.Kind),
            tab.Title,
            tab.NameHeader,
            tab.SupportHeader,
            tab.NotesHeader,
            Array.ConvertAll(tab.Rows, row => new FormatRow(row.Name, From(row.Support), row.SupportLabel, row.Notes,
                row.Action is { } action ? new FormatAction(action.Label, action.Uri) : null)))));

    private static FormatTabKind From(Ffi.FormatTabKind kind) => kind switch
    {
        Ffi.FormatTabKind.Video => FormatTabKind.Video,
        Ffi.FormatTabKind.Audio => FormatTabKind.Audio,
        Ffi.FormatTabKind.Containers => FormatTabKind.Containers,
        Ffi.FormatTabKind.Subtitles => FormatTabKind.Subtitles,
        _ => FormatTabKind.Hdr,
    };

    private static FormatSupport From(Ffi.FormatSupport support) => support switch
    {
        Ffi.FormatSupport.Hardware => FormatSupport.Hardware,
        Ffi.FormatSupport.Software => FormatSupport.Software,
        Ffi.FormatSupport.Plays => FormatSupport.Plays,
        Ffi.FormatSupport.ServerRepackages => FormatSupport.ServerRepackages,
        Ffi.FormatSupport.ServerConverts => FormatSupport.ServerConverts,
        Ffi.FormatSupport.Hdr => FormatSupport.Hdr,
        Ffi.FormatSupport.ToneMapped => FormatSupport.ToneMapped,
        Ffi.FormatSupport.Unknown => FormatSupport.Unknown,
        Ffi.FormatSupport.Native => FormatSupport.Native,
        _ => FormatSupport.NotSupported,
    };
}
