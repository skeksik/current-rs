using Ffi = uniffi.current_ffi;

namespace Current.Bindings;

/// <summary>
/// A track for the platform engine to show, by the server's stream index.
/// </summary>
public abstract record PlatformTrack
{
    private PlatformTrack() { }

    /// <summary>The engine's own choice.</summary>
    public sealed record Auto : PlatformTrack;

    public sealed record Off : PlatformTrack;

    public sealed record Stream(uint Index) : PlatformTrack;
}

/// <summary>
/// One file for the platform engine. <see cref="Url"/> is on the core's loopback relay (<c>http://127.0.0.1:&lt;port&gt;/&lt;route
/// token&gt;/...</c>) and holds no session token (ADR 0025): never log, store or show it. <see cref="ToString"/> leaves it out.
/// </summary>
public sealed record PlatformLoad(ulong Generation, string Url, TimeSpan Start, PlatformTrack Audio, PlatformTrack Subtitle, bool Hls)
{
    public override string ToString() =>
        $"PlatformLoad {{ Generation = {Generation}, Start = {Start}, Audio = {Audio}, Subtitle = {Subtitle}, Hls = {Hls} }}";
}

/// <summary>A subtitle the core fetched, for the engine to show over the video; <see cref="Format"/> by the server's name.</summary>
public sealed record PlatformSubtitle(uint Index, string? Format, byte[] Data, string? Title, string? Language, bool Select)
{
    public override string ToString() =>
        $"PlatformSubtitle {{ Index = {Index}, Format = {Format}, Bytes = {Data.Length}, Select = {Select} }}";
}

/// <summary>What a platform engine reports about a load (<see cref="PlatformEngineHost.Report(PlatformEngineEvent)"/>).</summary>
public abstract record PlatformEngineEvent
{
    private PlatformEngineEvent() { }

    /// <summary>The file opened.</summary>
    public sealed record Loaded : PlatformEngineEvent;

    /// <summary>Playback (re)started after opening or a seek: the first frame shows.</summary>
    public sealed record Restarted : PlatformEngineEvent;

    public sealed record Position(TimeSpan At) : PlatformEngineEvent;

    public sealed record Duration(TimeSpan Length) : PlatformEngineEvent;

    /// <summary>How far ahead of the position the engine holds data.</summary>
    public sealed record Buffered(TimeSpan Ahead) : PlatformEngineEvent;

    public sealed record Paused(bool IsPaused) : PlatformEngineEvent;

    public sealed record Buffering(bool IsBuffering) : PlatformEngineEvent;

    /// <summary>The file played to its end.</summary>
    public sealed record EndOfFile : PlatformEngineEvent;

    /// <summary>Stopped or replaced by the core's own command.</summary>
    public sealed record Stopped : PlatformEngineEvent;

    /// <summary>The file cannot play on. <see cref="Kind"/>: a short lower-case name (<c>decoder_failed</c>); anything else reads
    /// as <c>unexpected</c> in the core.</summary>
    public sealed record Failed(string Kind) : PlatformEngineEvent;

    /// <summary>A selected track cannot be played although the file opened (Windows: its <c>DecoderStatus</c> is not
    /// <c>FullySupported</c>, or it raised <c>OpenFailed</c>). The core moves the file to the next route.</summary>
    public sealed record TrackFailed(string Kind) : PlatformEngineEvent;

    /// <summary>Whether rates other than 1 work with the tracks playing (Windows: <c>IsSupportedPlaybackRateRange</c>). Report it
    /// after every load and on every track change.</summary>
    public sealed record RatesSupported(bool Supported) : PlatformEngineEvent;
}

/// <summary>A failure the platform engine knows, by a short lower-case name (<c>unsupported_format</c>); a failed load moves the
/// file to the next route. Any other exception from an engine method counts as unexpected, with the same effect.</summary>
public sealed class PlatformEngineFailure(string kind) : Exception(kind)
{
    public string Kind { get; } = kind;
}

/// <summary>Which list a decoder is looked for in.</summary>
public enum CodecQueryKind
{
    Video,
    Audio,
}

/// <summary>A decoder the core asks the Windows UI to look for: <c>CodecQuery.FindAllAsync(kind, CodecCategory.Decoder, subtype)</c>
/// with the value of the <c>Windows.Media.Core.CodecSubtypes</c> property named <see cref="Subtype"/>.</summary>
public sealed record CodecQueryAsk(CodecQueryKind Kind, string Subtype);

/// <summary>The UI's answer: how many decoders <c>CodecQuery</c> returned for <see cref="Subtype"/> (0 when an extension is
/// missing).</summary>
public sealed record CodecQueryAnswer(string Subtype, uint Decoders);

/// <summary>
/// The base of a platform's own player beside libmpv (ADR 0025): on Windows the app's Media Foundation engine derives from it (W4).
/// The core decides which engine plays a file and moves it between them; the engine only plays what it is told and reports facts.
/// <para>
/// Threads: the core calls the abstract methods on one of its own threads. Each must return quickly and never wait for the UI
/// thread: hand the work to the UI dispatcher and return. <see cref="Report(PlatformEngineEvent)"/> may be called from any thread
/// and never blocks.
/// </para>
/// <para>
/// Loads: each <see cref="Load"/> has a generation; events are about the load they are reported for, and the core drops those of an
/// older load or of a stopped one. <see cref="Report(PlatformEngineEvent)"/> uses the last load's.
/// </para>
/// <para>
/// Errors: throw <see cref="PlatformEngineFailure"/> for a failure the engine knows. Any exception from a method is caught by the
/// bridge: from <see cref="Load"/> it is a failed route (the core plays the file on libmpv, then the server); from any other method
/// the command is dropped. Nothing an engine throws brings the core down.
/// </para>
/// </summary>
public abstract class PlatformEngineHost : Ffi.PlatformEngine
{
    private Ffi.PlatformEngineEvents? _events;
    private long _generation;

    /// <summary>The last file the core loaded, as it crossed the bridge (for the tests: its generated <c>ToString()</c>).</summary>
    internal Ffi.PlatformFile? LastFile { get; private set; }

    /// <summary>The generation of the last load.</summary>
    protected ulong Generation => (ulong)Interlocked.Read(ref _generation);

    /// <summary>Reports <paramref name="report"/> about the last load. Any thread; never blocks. Before the player opened, nothing
    /// is sent.</summary>
    protected void Report(PlatformEngineEvent report) => Report(Generation, report);

    /// <summary>Reports <paramref name="report"/> about load <paramref name="generation"/>: for an event that comes late, after a
    /// newer load (the core drops it).</summary>
    protected void Report(ulong generation, PlatformEngineEvent report)
    {
        ArgumentNullException.ThrowIfNull(report);
        Volatile.Read(ref _events)?.Send(generation, PlatformEngineMap.ToFfi(report));
    }

    /// <summary>Replaces the file playing, if any. Throw <see cref="PlatformEngineFailure"/> when the engine cannot take it.</summary>
    protected abstract void Load(PlatformLoad file);

    protected abstract void AddSubtitle(PlatformSubtitle subtitle);

    protected abstract void SetPaused(bool paused);

    protected abstract void SeekTo(TimeSpan position);

    protected abstract void SelectAudio(PlatformTrack track);

    protected abstract void SelectSubtitle(PlatformTrack track);

    /// <summary>0 to 100.</summary>
    protected abstract void SetVolume(byte volume);

    protected abstract void SetMuted(bool muted);

    /// <summary>The playback rate, 1 for normal; within 0.01 to 100.</summary>
    protected abstract void SetSpeed(double speed);

    /// <summary>Unloads the file. The core stops taking its events before this call.</summary>
    protected abstract void Stop();

    void Ffi.PlatformEngine.Attach(Ffi.PlatformEngineEvents events) => Volatile.Write(ref _events, events);

    void Ffi.PlatformEngine.Load(Ffi.PlatformFile file)
    {
        LastFile = file;
        Interlocked.Exchange(ref _generation, unchecked((long)file.Generation));
        Run(() => Load(PlatformEngineMap.From(file)));
    }

    void Ffi.PlatformEngine.AddSubtitle(Ffi.PlatformSubtitleFile subtitle) => Run(() => AddSubtitle(PlatformEngineMap.From(subtitle)));

    void Ffi.PlatformEngine.SetPaused(bool paused) => Run(() => SetPaused(paused));

    void Ffi.PlatformEngine.SeekTo(ulong positionMs) => Run(() => SeekTo(TimeSpan.FromMilliseconds(positionMs)));

    void Ffi.PlatformEngine.SelectAudio(Ffi.PlatformTrack track) => Run(() => SelectAudio(PlatformEngineMap.From(track)));

    void Ffi.PlatformEngine.SelectSubtitle(Ffi.PlatformTrack track) => Run(() => SelectSubtitle(PlatformEngineMap.From(track)));

    void Ffi.PlatformEngine.SetVolume(byte volume) => Run(() => SetVolume(volume));

    void Ffi.PlatformEngine.SetMuted(bool muted) => Run(() => SetMuted(muted));

    void Ffi.PlatformEngine.SetSpeed(double speed) => Run(() => SetSpeed(speed));

    void Ffi.PlatformEngine.Stop() => Run(Stop);

    /// <summary>A failure the engine knows crosses as the core's typed error; any other exception as unexpected (the bridge's own
    /// handling).</summary>
    private static void Run(Action call)
    {
        try
        {
            call();
        }
        catch (PlatformEngineFailure failure)
        {
            throw new Ffi.PlatformEngineException.Failed(failure.Kind);
        }
    }

    /// <summary>What the core asks the Windows UI to look for before it opens a player with Windows' own player: the core's table,
    /// the UI only answers (<see cref="PlayerSession.OpenWithPlatformEngineAsync"/>).</summary>
    public static IReadOnlyList<CodecQueryAsk> WindowsCodecQueries() =>
        Array.ConvertAll(Ffi.CurrentFfiMethods.WindowsCodecQueries(), ask => new CodecQueryAsk(
            ask.Kind == Ffi.CodecQueryKind.Audio ? CodecQueryKind.Audio : CodecQueryKind.Video, ask.Subtype));
}

internal static class PlatformEngineMap
{
    private static TimeSpan Ms(ulong ms) => TimeSpan.FromMilliseconds(ms);

    private static ulong ToMs(TimeSpan time) => time <= TimeSpan.Zero ? 0 : (ulong)time.TotalMilliseconds;

    public static PlatformTrack From(Ffi.PlatformTrack track) => track switch
    {
        Ffi.PlatformTrack.Stream stream => new PlatformTrack.Stream(stream.Index),
        Ffi.PlatformTrack.Off => new PlatformTrack.Off(),
        _ => new PlatformTrack.Auto(),
    };

    public static PlatformLoad From(Ffi.PlatformFile file) =>
        new(file.Generation, file.Url, Ms(file.StartMs), From(file.Audio), From(file.Subtitle), file.Hls);

    public static PlatformSubtitle From(Ffi.PlatformSubtitleFile subtitle) =>
        new(subtitle.Index, subtitle.Format, subtitle.Data, subtitle.Title, subtitle.Language, subtitle.Select);

    public static Ffi.PlatformEvent ToFfi(PlatformEngineEvent report) => report switch
    {
        PlatformEngineEvent.Loaded => new Ffi.PlatformEvent.Loaded(),
        PlatformEngineEvent.Restarted => new Ffi.PlatformEvent.Restarted(),
        PlatformEngineEvent.Position position => new Ffi.PlatformEvent.Position(ToMs(position.At)),
        PlatformEngineEvent.Duration duration => new Ffi.PlatformEvent.Duration(ToMs(duration.Length)),
        PlatformEngineEvent.Buffered buffered => new Ffi.PlatformEvent.Buffered(ToMs(buffered.Ahead)),
        PlatformEngineEvent.Paused paused => new Ffi.PlatformEvent.Paused(paused.IsPaused),
        PlatformEngineEvent.Buffering buffering => new Ffi.PlatformEvent.Buffering(buffering.IsBuffering),
        PlatformEngineEvent.EndOfFile => new Ffi.PlatformEvent.Ended(new Ffi.PlatformEndReason.Eof()),
        PlatformEngineEvent.Stopped => new Ffi.PlatformEvent.Ended(new Ffi.PlatformEndReason.Stopped()),
        PlatformEngineEvent.Failed failed => new Ffi.PlatformEvent.Ended(new Ffi.PlatformEndReason.Error(failed.Kind)),
        PlatformEngineEvent.TrackFailed failed => new Ffi.PlatformEvent.TrackFailed(failed.Kind),
        PlatformEngineEvent.RatesSupported rates => new Ffi.PlatformEvent.RatesSupported(rates.Supported),
        _ => throw new ArgumentOutOfRangeException(nameof(report), report, "an event the bridge does not know"),
    };

    public static Ffi.CodecQueryAnswer ToFfi(CodecQueryAnswer answer) => new(answer.Subtype, answer.Decoders);
}
