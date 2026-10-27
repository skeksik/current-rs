//! What the player says, worded once for every platform (CLAUDE.md: all logic lives in the core): its times, shown and spoken, the
//! route and its reasons, the plan's notes, track names, the skip button, the next-episode card, the track menus' fixed items and
//! problems in plain words (ADR 0019). Never an address or a server's message: the inputs hold none. Bitrates and the quality menu
//! are [`super::labels`]; the playback info panel is [`super::info_panel`].
//!
//! These are the Windows app's words as it wrote them in C# before they moved here; the tests pin them.

use current_model::RouteLabel;
use current_playback::PlanNote;

use super::labels::bitrate_label;
use super::problem::{PlaybackProblem, PlayerProblem};
use crate::numbers::NumberFormat;
use crate::problems::UNEXPECTED;

const MS_PER_SECOND: i64 = 1_000;
const MS_PER_MINUTE: i64 = 60 * MS_PER_SECOND;
const MS_PER_HOUR: i64 = 60 * MS_PER_MINUTE;

/// The hours, minutes and seconds components of a time in milliseconds, below 0 as 0, as .NET's `TimeSpan` has them: the hours
/// component is the hours of the day, 0 to 23 ("TimeSpan.Hours: the hour component", learn.microsoft.com, read 2026-10-27).
fn components(ms: i64) -> (i64, i64, i64) {
    let ms = ms.max(0);
    (
        (ms / MS_PER_HOUR) % 24,
        (ms / MS_PER_MINUTE) % 60,
        (ms / MS_PER_SECOND) % 60,
    )
}

/// A time as the player shows it: "1:02:03" from an hour on, else "2:03" (the Windows app wrote .NET's `TimeSpan` formats
/// `h\:mm\:ss` and `m\:ss`, whose escaped colons are literals, so the culture changes nothing: "Custom TimeSpan format strings",
/// learn.microsoft.com, read 2026-10-27). Below 0 is "0:00". `h` is the hours component, not the total, so 24 hours or more wrap
/// ("25:00:00" shows "1:00:00"), as they did in C#: kept until the roadmap's fix.
pub fn time_label(ms: i64) -> String {
    let (hours, minutes, seconds) = components(ms);
    if ms >= MS_PER_HOUR {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// A time as a screen reader should say it: "1 hour 2 minutes 3 seconds", "0 seconds" (Narrator reads "1:02:03" as a time of day).
/// The hours are the whole hours in total, so this does not wrap at 24.
pub fn spoken_time(ms: i64) -> String {
    let ms = ms.max(0);
    let (_, minutes, seconds) = components(ms);
    let parts: Vec<String> = [
        (ms / MS_PER_HOUR, "hour"),
        (minutes, "minute"),
        (seconds, "second"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, unit)| {
        if count == 1 {
            format!("1 {unit}")
        } else {
            format!("{count} {unit}s")
        }
    })
    .collect();
    if parts.is_empty() {
        "0 seconds".to_owned()
    } else {
        parts.join(" ")
    }
}

/// The elapsed time's spoken name: "Time elapsed 12 minutes 3 seconds".
pub fn elapsed_label(ms: i64) -> String {
    format!("Time elapsed {}", spoken_time(ms))
}

/// The remaining time's spoken name: "Time remaining 12 minutes 3 seconds".
pub fn remaining_label(ms: i64) -> String {
    format!("Time remaining {}", spoken_time(ms))
}

/// The seek bar's spoken status: where it is of how long, or where it is when the length is not known.
pub fn position_label(ms: i64, duration_ms: Option<i64>) -> String {
    match duration_ms {
        Some(total) => format!("{} of {}", spoken_time(ms), spoken_time(total)),
        None => spoken_time(ms),
    }
}

/// ADR 0019's route labels: a remux and a direct stream are transcode jobs that copy streams.
pub fn route_name(route: RouteLabel) -> &'static str {
    match route {
        RouteLabel::DirectPlay => "Direct play",
        RouteLabel::Remux => "Remux",
        RouteLabel::DirectStream => "Direct stream",
        RouteLabel::Transcode => "Transcode",
    }
}

/// Why the server streams rather than plays the file directly, in plain words; a reason no release knows is shown by the server's
/// own name (ADR 0019, "Reasons").
pub fn reason_text(reason: &str) -> &str {
    match reason {
        "container_not_supported" => "The player can't read this container.",
        "video_codec_not_supported" => "The player can't decode this video codec.",
        "audio_codec_not_supported" => "The player can't decode this audio codec.",
        "subtitle_codec_not_supported" => {
            "The player can't show these subtitles, so they're burned into the video."
        }
        "audio_is_external" => "The audio is in a separate file.",
        "secondary_audio_not_supported" => "This audio track can't be played directly.",
        "stream_count_exceeds_limit" => "The file has more streams than the player takes.",
        "video_profile_not_supported" => "The player can't decode this video profile.",
        "video_range_type_not_supported" => "The player can't show this video's dynamic range.",
        "video_codec_tag_not_supported" => "The player can't read this video codec tag.",
        "video_level_not_supported" => "The player can't decode this video level.",
        "video_resolution_not_supported" => "The video's resolution is too high for the player.",
        "video_bit_depth_not_supported" => "The player can't decode this video bit depth.",
        "video_framerate_not_supported" => "The player can't play this frame rate.",
        "video_rotation_not_supported" => "The player can't rotate this video.",
        "ref_frames_not_supported" => "The video uses more reference frames than the player takes.",
        "anamorphic_video_not_supported" => "The player can't show anamorphic video.",
        "interlaced_video_not_supported" => "The player can't show interlaced video.",
        "audio_channels_not_supported" => "The player can't play this many audio channels.",
        "audio_profile_not_supported" => "The player can't decode this audio profile.",
        "audio_sample_rate_not_supported" => "The player can't play this audio sample rate.",
        "audio_bit_depth_not_supported" => "The player can't play this audio bit depth.",
        "container_bitrate_exceeds_limit" => "The file's bitrate is above the quality limit.",
        "video_bitrate_not_supported" => "The video's bitrate is above the quality limit.",
        "audio_bitrate_not_supported" => "The audio's bitrate is above the quality limit.",
        "unknown_video_stream_info" => "The server doesn't know enough about the video.",
        "unknown_audio_stream_info" => "The server doesn't know enough about the audio.",
        "direct_play_error" => "Direct play failed on the server.",
        "quality_limit" => "The quality you chose is below the file's.",
        "fallback_after_engine_error" => "The player couldn't play the file directly.",
        "user_chose_transcode" => "You chose a converted stream.",
        other => other,
    }
}

/// What the server's permissions say about the plan; the bitrate as [`bitrate_label`] writes it.
pub fn plan_note_text(note: &PlanNote, format: &NumberFormat) -> String {
    match note {
        PlanNote::RemoteBitrateLimit { limit } => format!(
            "The server limits streaming outside your network to {}.",
            bitrate_label(*limit, format)
        ),
        PlanNote::TranscodingNotAllowed => "The server doesn't allow you to convert video.".into(),
        PlanNote::RemuxingNotAllowed => "The server doesn't allow you to repackage video.".into(),
        PlanNote::RemoteSourceTranscodingForced => {
            "The server converts every stream from a remote source.".into()
        }
    }
}

/// A track as the server names it (its display title), else by its language and codec ("eng · AC3"), else its number ("Track 3").
/// Empty parts count as missing. The codec is upper-cased as .NET's `ToUpperInvariant` did for the codec names servers send (ASCII).
pub fn track_name(
    label: Option<&str>,
    language: Option<&str>,
    codec: Option<&str>,
    index: u32,
) -> String {
    if let Some(label) = label.filter(|label| !label.is_empty()) {
        return label.to_owned();
    }
    let codec = codec.map(str::to_uppercase);
    let parts: Vec<&str> = [language, codec.as_deref()]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        format!("Track {index}")
    } else {
        parts.join(" · ")
    }
}

/// The skip button for the segment under the position (ADR 0019, choice 8); "Skip" for a kind no release knows.
pub fn skip_label(kind: &str) -> &'static str {
    match kind {
        "intro" => "Skip intro",
        "outro" => "Skip credits",
        "recap" => "Skip recap",
        "preview" => "Skip preview",
        "commercial" => "Skip ad",
        _ => "Skip",
    }
}

/// The next-episode card's line: "S1:E3 Name", or the name alone when the number label is empty.
pub fn episode_line(number: &str, name: &str) -> String {
    if number.is_empty() {
        name.to_owned()
    } else {
        format!("{number} {name}")
    }
}

/// The next-episode card's countdown: "Starts in 5 seconds", "Starts in 1 second".
pub fn countdown_text(seconds: u32) -> String {
    if seconds == 1 {
        "Starts in 1 second".to_owned()
    } else {
        format!("Starts in {seconds} seconds")
    }
}

/// The audio menu's only item when the item has no audio track.
pub const NO_AUDIO_TRACKS: &str = "No audio tracks";

/// The subtitles menu's first item: no subtitles.
pub const SUBTITLES_OFF: &str = "Off";

/// Why playback stopped, in plain words (ADR 0019, "Errors").
pub fn playback_problem_text(problem: &PlaybackProblem) -> String {
    match problem {
        PlaybackProblem::NotPlayable { .. } => "The server offers no way to play this item.".into(),
        PlaybackProblem::Engine { kind } => match kind.as_str() {
            "unsupported_format" => "The player can't play this file's format.".into(),
            "decoder_failed" => "The player couldn't decode this file.".into(),
            "engine_gone" => "The player stopped working. Go back and play the item again.".into(),
            "gateway_playlist_unchecked" => playlist_elsewhere(),
            _ => "The player couldn't open the stream.".into(),
        },
        PlaybackProblem::PlaylistElsewhere => playlist_elsewhere(),
        PlaybackProblem::LocalRelay => {
            "Current could not start its local stream connection.".into()
        }
        PlaybackProblem::Network => {
            "The server can't be reached. Check your connection and try again.".into()
        }
        PlaybackProblem::GatewaySignInNeeded => {
            "Sign in to your server's gateway again in Settings, then play again.".into()
        }
        PlaybackProblem::SignedOut => {
            "The server ended your session. Sign in again in Settings.".into()
        }
        PlaybackProblem::Server { status } => {
            format!("The server couldn't play this item (error {status}).")
        }
        PlaybackProblem::NotFound => "The server can't find this item anymore.".into(),
    }
}

fn playlist_elsewhere() -> String {
    "The server's stream points to another address, so Current won't send it your gateway sign-in."
        .into()
}

/// Why the player couldn't open or act; a replaced action or a closed player, which the UI does not show as a problem, gets the
/// general words.
pub fn player_problem_text(problem: &PlayerProblem) -> &'static str {
    match problem {
        PlayerProblem::EngineUnavailable => "This build of Current has no player.",
        PlayerProblem::Busy => "Another video is still closing. Try again in a moment.",
        PlayerProblem::EngineFailed { .. } => "The player couldn't start.",
        PlayerProblem::NotSignedIn => "Sign in to this item's server first.",
        PlayerProblem::Replaced | PlayerProblem::Closed => UNEXPECTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000;
    const M: i64 = 60 * S;
    const H: i64 = 60 * M;

    #[test]
    fn times_read_as_the_windows_app_wrote_them() {
        let cases = [
            (-5 * S, "0:00"),
            (0, "0:00"),
            (999, "0:00"),
            (3 * S, "0:03"),
            (2 * M + 3 * S, "2:03"),
            (59 * M + 59 * S + 999, "59:59"),
            (H, "1:00:00"),
            (H + 2 * M + 3 * S, "1:02:03"),
            (12 * H + 34 * M + 56 * S, "12:34:56"),
            (23 * H + 59 * M + 59 * S, "23:59:59"),
            // The hours component wraps at a day, as .NET's "h" did (roadmap: "Fix and polish").
            (24 * H, "0:00:00"),
            (25 * H + 2 * M + 3 * S, "1:02:03"),
        ];
        for (ms, text) in cases {
            assert_eq!(time_label(ms), text, "{ms}");
        }
    }

    #[test]
    fn spoken_times_name_their_units() {
        let cases = [
            (-S, "0 seconds"),
            (0, "0 seconds"),
            (S, "1 second"),
            (2 * S, "2 seconds"),
            (M, "1 minute"),
            (12 * M + 3 * S, "12 minutes 3 seconds"),
            (H + 2 * M + 3 * S, "1 hour 2 minutes 3 seconds"),
            (2 * H + M, "2 hours 1 minute"),
            // Whole hours in total: no wrap.
            (25 * H + S, "25 hours 1 second"),
        ];
        for (ms, text) in cases {
            assert_eq!(spoken_time(ms), text, "{ms}");
        }
        assert_eq!(
            elapsed_label(12 * M + 3 * S),
            "Time elapsed 12 minutes 3 seconds"
        );
        assert_eq!(remaining_label(-S), "Time remaining 0 seconds");
        assert_eq!(
            position_label(M, Some(H + 30 * M)),
            "1 minute of 1 hour 30 minutes"
        );
        assert_eq!(position_label(3 * S, None), "3 seconds");
    }

    #[test]
    fn routes_and_reasons_in_plain_words() {
        assert_eq!(route_name(RouteLabel::DirectPlay), "Direct play");
        assert_eq!(route_name(RouteLabel::Remux), "Remux");
        assert_eq!(route_name(RouteLabel::DirectStream), "Direct stream");
        assert_eq!(route_name(RouteLabel::Transcode), "Transcode");
        assert_eq!(
            reason_text("video_codec_not_supported"),
            "The player can't decode this video codec."
        );
        assert_eq!(
            reason_text("user_chose_transcode"),
            "You chose a converted stream."
        );
        assert_eq!(
            reason_text("a_reason_no_release_knows"),
            "a_reason_no_release_knows"
        );
    }

    #[test]
    fn notes_write_bitrates_with_the_platforms_separator() {
        let comma = NumberFormat {
            decimal_separator: ",".to_owned(),
            ..NumberFormat::default()
        };
        let limit = PlanNote::RemoteBitrateLimit { limit: 1_500_000 };
        assert_eq!(
            plan_note_text(&limit, &NumberFormat::default()),
            "The server limits streaming outside your network to 1.5 Mbit/s."
        );
        assert_eq!(
            plan_note_text(&limit, &comma),
            "The server limits streaming outside your network to 1,5 Mbit/s."
        );
        assert_eq!(
            plan_note_text(&PlanNote::TranscodingNotAllowed, &comma),
            "The server doesn't allow you to convert video."
        );
        assert_eq!(
            plan_note_text(&PlanNote::RemuxingNotAllowed, &comma),
            "The server doesn't allow you to repackage video."
        );
        assert_eq!(
            plan_note_text(&PlanNote::RemoteSourceTranscodingForced, &comma),
            "The server converts every stream from a remote source."
        );
    }

    #[test]
    fn a_track_is_named_by_the_server_else_by_its_facts() {
        assert_eq!(
            track_name(Some("English - Dolby Digital"), Some("eng"), Some("ac3"), 1),
            "English - Dolby Digital"
        );
        assert_eq!(
            track_name(Some(""), Some("eng"), Some("ac3"), 1),
            "eng · AC3"
        );
        assert_eq!(track_name(None, None, Some("srt"), 4), "SRT");
        assert_eq!(track_name(None, Some("eng"), Some(""), 4), "eng");
        assert_eq!(track_name(None, Some(""), None, 3), "Track 3");
    }

    #[test]
    fn the_skip_button_and_the_next_episode_card() {
        assert_eq!(skip_label("intro"), "Skip intro");
        assert_eq!(skip_label("outro"), "Skip credits");
        assert_eq!(skip_label("recap"), "Skip recap");
        assert_eq!(skip_label("preview"), "Skip preview");
        assert_eq!(skip_label("commercial"), "Skip ad");
        assert_eq!(skip_label("a_kind_no_release_knows"), "Skip");
        assert_eq!(episode_line("S1:E3", "Name"), "S1:E3 Name");
        assert_eq!(episode_line("", "Name"), "Name");
        assert_eq!(countdown_text(1), "Starts in 1 second");
        assert_eq!(countdown_text(0), "Starts in 0 seconds");
        assert_eq!(countdown_text(5), "Starts in 5 seconds");
        assert_eq!(NO_AUDIO_TRACKS, "No audio tracks");
        assert_eq!(SUBTITLES_OFF, "Off");
    }

    #[test]
    fn playback_problems_in_plain_words() {
        let engine = |kind: &str| PlaybackProblem::Engine { kind: kind.into() };
        let elsewhere = "The server's stream points to another address, so Current won't send it your gateway sign-in.";
        let cases = [
            (
                PlaybackProblem::NotPlayable { reasons: vec![] },
                "The server offers no way to play this item.",
            ),
            (
                engine("unsupported_format"),
                "The player can't play this file's format.",
            ),
            (
                engine("decoder_failed"),
                "The player couldn't decode this file.",
            ),
            (
                engine("engine_gone"),
                "The player stopped working. Go back and play the item again.",
            ),
            (engine("gateway_playlist_unchecked"), elsewhere),
            (PlaybackProblem::PlaylistElsewhere, elsewhere),
            (
                engine("loading_failed"),
                "The player couldn't open the stream.",
            ),
            (
                engine("a_kind_no_release_knows"),
                "The player couldn't open the stream.",
            ),
            (
                PlaybackProblem::LocalRelay,
                "Current could not start its local stream connection.",
            ),
            (
                PlaybackProblem::Network,
                "The server can't be reached. Check your connection and try again.",
            ),
            (
                PlaybackProblem::GatewaySignInNeeded,
                "Sign in to your server's gateway again in Settings, then play again.",
            ),
            (
                PlaybackProblem::SignedOut,
                "The server ended your session. Sign in again in Settings.",
            ),
            (
                PlaybackProblem::Server { status: 500 },
                "The server couldn't play this item (error 500).",
            ),
            (
                PlaybackProblem::NotFound,
                "The server can't find this item anymore.",
            ),
        ];
        for (problem, text) in cases {
            assert_eq!(playback_problem_text(&problem), text, "{problem:?}");
        }
    }

    #[test]
    fn player_problems_in_plain_words() {
        let cases = [
            (
                PlayerProblem::EngineUnavailable,
                "This build of Current has no player.",
            ),
            (
                PlayerProblem::Busy,
                "Another video is still closing. Try again in a moment.",
            ),
            (
                PlayerProblem::EngineFailed {
                    kind: "loading_failed".into(),
                },
                "The player couldn't start.",
            ),
            (
                PlayerProblem::NotSignedIn,
                "Sign in to this item's server first.",
            ),
            (
                PlayerProblem::Replaced,
                "Something went wrong in Current. Try again.",
            ),
            (
                PlayerProblem::Closed,
                "Something went wrong in Current. Try again.",
            ),
        ];
        for (problem, text) in cases {
            assert_eq!(player_problem_text(&problem), text, "{problem:?}");
        }
    }

    #[test]
    fn the_fallback_is_the_windows_apps_unexpected_text() {
        assert_eq!(UNEXPECTED, "Something went wrong in Current. Try again.");
    }
}
