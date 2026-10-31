using System.Diagnostics;
using Current.Bindings;
using Windows.Media.Core;

namespace Current.Shell.Player;

/// <summary>
/// The UI's answers to the core's decoder questions (<see cref="PlatformEngineHost.WindowsCodecQueries"/>), asked before a player
/// opens with Windows' own player (ADR 0025, W4): <c>CodecQuery.FindAllAsync(kind, CodecCategory.Decoder, subtype)</c> per question,
/// which returns one entry per installed decoder and none when a Store extension is missing, and works unpackaged (W1 §3,
/// docs/notes/windows-native-player.md). Each answer carries the entries' <c>DisplayName</c>s, and the core decides which decode (W7 §4:
/// Windows' own IEC 61937 converter is returned for AC-3 and E-AC-3 and decodes nothing). The subtype is the value of the <c>CodecSubtypes</c> property the core names (CodecSubtypes
/// Class, learn.microsoft.com, ms.date 2017-02-08, read 2026-10-29: every name below is listed there), except AV1's, asked for by its
/// FOURCC (below). A name not in that list, or a query that fails, answers 0 decoders: the codec then plays on libmpv. Asked once per
/// app run, in parallel, off the UI thread.
/// </summary>
internal static class CodecAnswers
{
    private static readonly object Gate = new();
    private static Task<IReadOnlyList<CodecQueryAnswer>>? _answers;

    /// <summary>The answers, asked on the first call; never throws. A run that failed is asked again next time, never kept.</summary>
    public static Task<IReadOnlyList<CodecQueryAnswer>> GetAsync()
    {
        lock (Gate)
        {
            if (_answers is { IsFaulted: true } or { IsCanceled: true })
            {
                _answers = null;
            }
            return _answers ??= Task.Run(AskAsync);
        }
    }

    /// <summary>Every answer; none when the questions or CodecQuery cannot be had, and the native route then decodes nothing.</summary>
    private static async Task<IReadOnlyList<CodecQueryAnswer>> AskAsync()
    {
        IReadOnlyList<CodecQueryAsk> asks;
        CodecQuery query;
        try
        {
            asks = PlatformEngineHost.WindowsCodecQueries();
            query = new CodecQuery();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: CodecQuery is not available: {error.GetType().Name}");
            return [];
        }
        var answers = await Task.WhenAll(asks.Select(async ask =>
        {
            try
            {
                if (Subtype(ask.Subtype) is not { } subtype)
                {
                    Trace.TraceWarning($"Current: no CodecSubtypes property for {ask.Subtype}");
                    return new CodecQueryAnswer(ask.Subtype, []);
                }
                var kind = ask.Kind == CodecQueryKind.Audio ? CodecKind.Audio : CodecKind.Video;
                var found = await query.FindAllAsync(kind, CodecCategory.Decoder, subtype);
                // The names go to the core only, never to the log.
                var names = found.Select(codec => codec.DisplayName).ToList();
                return new CodecQueryAnswer(ask.Subtype, names);
            }
            catch (Exception error)
            {
                Trace.TraceError($"Current: CodecQuery for {ask.Subtype} failed: {error.GetType().Name}");
                return new CodecQueryAnswer(ask.Subtype, []);
            }
        }));
        return answers;
    }

    /// <summary>The <c>CodecSubtypes</c> property named <paramref name="name"/>, without reflection.</summary>
    private static string? Subtype(string name) => name switch
    {
        nameof(CodecSubtypes.VideoFormatH264) => CodecSubtypes.VideoFormatH264,
        nameof(CodecSubtypes.VideoFormatHevc) => CodecSubtypes.VideoFormatHevc,
        // The CodecSubtypes page lists VideoFormatAv1, but the projection of the Windows SDK this app builds against
        // (net10.0-windows10.0.26100.0) has no such property (CI, 2026-10-29; which SDK adds it is not verified). W1 §3 asked for AV1
        // with the subtype "AV01" (its FOURCC) and got one entry per installed decoder, none without the extension.
        "VideoFormatAv1" => "AV01",
        nameof(CodecSubtypes.VideoFormatVP90) => CodecSubtypes.VideoFormatVP90,
        nameof(CodecSubtypes.VideoFormatVP80) => CodecSubtypes.VideoFormatVP80,
        nameof(CodecSubtypes.VideoFormatMpeg2) => CodecSubtypes.VideoFormatMpeg2,
        nameof(CodecSubtypes.AudioFormatAac) => CodecSubtypes.AudioFormatAac,
        nameof(CodecSubtypes.AudioFormatDolbyAC3) => CodecSubtypes.AudioFormatDolbyAC3,
        nameof(CodecSubtypes.AudioFormatDolbyDDPlus) => CodecSubtypes.AudioFormatDolbyDDPlus,
        nameof(CodecSubtypes.AudioFormatFlac) => CodecSubtypes.AudioFormatFlac,
        nameof(CodecSubtypes.AudioFormatOpus) => CodecSubtypes.AudioFormatOpus,
        nameof(CodecSubtypes.AudioFormatMP3) => CodecSubtypes.AudioFormatMP3,
        _ => null,
    };
}
