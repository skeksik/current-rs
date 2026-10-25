//! `current-mpv` as the player's engine: the mapping from `current-playback`'s files and tracks to libmpv's, and from libmpv's events
//! to the controller's. The mapping ([`MpvMapper`]) needs no libmpv and is tested everywhere; the engine itself ([`MpvBackend`]) is
//! built only with the `libmpv` feature.
//!
//! - The URL and its credentials move into a [`FileRequest`] as they are: never formatted, logged or shown.
//! - A playlist (HLS, DASH) with gateway headers is refused with [`EngineErrorKind::GATEWAY_PLAYLIST_UNCHECKED`] unless the file says
//!   the core checked its playlists (ADR 0019, "Playlists and headers"); `current-mpv` refuses it too.
//! - Tracks are chosen by the server's stream index. libmpv names tracks by its own ids, so an embedded stream is found by `ff-index`
//!   (that it equals Jellyfin's index is checked on real files in task 5) and an added external subtitle by the order it was added in.
//!   A choice made before the track list is known is applied when it arrives.

// Without the `libmpv` feature only the test engine uses the mapping (its load refusal); the rest is still compiled and tested.
#![cfg_attr(not(feature = "libmpv"), allow(dead_code))]

use std::collections::VecDeque;
use std::time::Duration;

use current_model::EngineErrorKind;
use current_mpv::{
    EndReason as MpvEnd, EngineError, EngineEvent as MpvEvent, FileRequest, MpvError,
    PropertyChange, StreamCookie, SubtitleFile, Track, TrackChoice,
};
use current_playback::{EndReason, EngineEvent, EngineFile, EngineTrack, ExternalSubtitle};

use super::backend::{EngineOutput, Signal, SwapChain, VideoOutput};

/// An environment variable naming one of mpv's `video-sync` modes, for comparing `display-resample` with mpv's default on a device
/// (ADR 0007 item 5, ADR 0019 choice 11; docs/notes/windows-video.md). A test aid, not a setting: unset, mpv keeps its default.
pub(crate) const VIDEO_SYNC_VARIABLE: &str = "CURRENT_MPV_VIDEO_SYNC";

/// Which kind of track a choice is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackKind {
    Audio,
    Subtitle,
}

impl TrackKind {
    fn mpv_type(self) -> &'static str {
        match self {
            TrackKind::Audio => "audio",
            TrackKind::Subtitle => "sub",
        }
    }
}

/// A track change for the engine to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MpvCommand {
    Audio(TrackChoice),
    Subtitle(TrackChoice),
}

/// The state of the mapping for the file playing.
#[derive(Debug, Default)]
pub(crate) struct MpvMapper {
    tracks: Vec<Track>,
    loaded: bool,
    ended: bool,
    position: Option<f64>,
    want_audio: Option<u32>,
    want_subtitle: Option<u32>,
    /// External subtitles added and not yet seen in the track list, by server index, oldest first.
    pending_external: VecDeque<u32>,
    /// Server index to libmpv track id, for added external subtitles.
    external: Vec<(u32, i64)>,
    output: EngineOutput,
}

fn seconds(duration: Duration) -> f64 {
    duration.as_secs_f64()
}

fn duration(seconds: f64) -> Duration {
    if seconds.is_finite() && seconds > 0.0 {
        Duration::from_secs_f64(seconds)
    } else {
        Duration::ZERO
    }
}

/// Why `current-mpv` refused a load, as the controller's kind.
pub(crate) fn engine_error_kind(error: &EngineError) -> EngineErrorKind {
    match error {
        EngineError::UncheckedPlaylist => EngineErrorKind::GATEWAY_PLAYLIST_UNCHECKED,
        _ => EngineErrorKind::LOADING_FAILED,
    }
}

impl MpvMapper {
    /// The request for `file`, from now on the file the mapping is about. A file whose playlists the core checked
    /// ([`EngineFile::playlist_checked`]) is marked so; any other playlist with gateway headers is refused.
    pub(crate) fn file_request(
        &mut self,
        file: EngineFile,
    ) -> Result<FileRequest, EngineErrorKind> {
        let EngineFile {
            url,
            auth,
            start,
            audio,
            subtitle,
            playlist_checked,
        } = file;
        let mut request = FileRequest::new(url.expose().to_owned());
        request.start = (start > Duration::ZERO).then(|| seconds(start));
        request.cookies = auth
            .cookies
            .into_iter()
            .map(|cookie| StreamCookie {
                name: cookie.name,
                value: cookie.value,
                path: cookie.path,
                secure: cookie.secure,
            })
            .collect();
        request.headers = auth.headers;
        request.secrets = auth.secrets;
        if playlist_checked {
            request = request.playlist_checked_same_origin();
        }
        // The defensive refusal: an unchecked playlist with headers never reaches libmpv (the controller checks before it loads).
        request
            .check_header_scope()
            .map_err(|error| engine_error_kind(&error))?;
        request
            .validate()
            .map_err(|error| engine_error_kind(&error))?;
        *self = MpvMapper::default();
        let (aid, want_audio) = initial(audio);
        let (sid, want_subtitle) = initial(subtitle);
        request.audio = aid;
        request.subtitle = sid;
        self.want_audio = want_audio;
        self.want_subtitle = want_subtitle;
        Ok(request)
    }

    /// The `sub-add` for a subtitle the core fetched; its track is found once libmpv lists it. One added to be shown is also chosen
    /// then, as an embedded one is: `sub-add`'s `select` on a file loaded with `sid=no` left it unshown on the owner's run
    /// (2026-10-25) although a headless libmpv selects it (`current-mpv`'s live test), and choosing it by id shows it.
    pub(crate) fn subtitle_file(&mut self, subtitle: ExternalSubtitle) -> SubtitleFile {
        self.pending_external.push_back(subtitle.index);
        if subtitle.select {
            self.want_subtitle = Some(subtitle.index);
        }
        SubtitleFile {
            data: subtitle.data,
            title: subtitle.title,
            language: subtitle.language,
            select: subtitle.select,
        }
    }

    /// libmpv's choice for `track`, or `None` while the track is not listed yet (it is chosen when it is).
    pub(crate) fn select(&mut self, kind: TrackKind, track: EngineTrack) -> Option<TrackChoice> {
        let want = match kind {
            TrackKind::Audio => &mut self.want_audio,
            TrackKind::Subtitle => &mut self.want_subtitle,
        };
        *want = None;
        match track {
            EngineTrack::Auto => Some(TrackChoice::Auto),
            EngineTrack::Off => Some(TrackChoice::Off),
            EngineTrack::Stream(index) => match self.track_id(kind, index) {
                Some(id) => Some(TrackChoice::Id(id)),
                None => {
                    let want = match kind {
                        TrackKind::Audio => &mut self.want_audio,
                        TrackKind::Subtitle => &mut self.want_subtitle,
                    };
                    *want = Some(index);
                    None
                }
            },
        }
    }

    pub(crate) fn output(&self) -> EngineOutput {
        self.output.clone()
    }

    fn track_id(&self, kind: TrackKind, index: u32) -> Option<i64> {
        if kind == TrackKind::Subtitle
            && let Some((_, id)) = self.external.iter().find(|(server, _)| *server == index)
        {
            return Some(*id);
        }
        self.tracks
            .iter()
            .find(|track| {
                track.kind == kind.mpv_type()
                    && !track.external
                    && track.ff_index == Some(i64::from(index))
            })
            .map(|track| track.id)
    }

    /// Turns one libmpv event into the controller's events and the swap chain, and the track changes now possible.
    pub(crate) fn translate(
        &mut self,
        event: MpvEvent,
        signals: &mut Vec<Signal>,
        commands: &mut Vec<MpvCommand>,
    ) {
        let mut push = |event: EngineEvent| signals.push(Signal::Event(event));
        match event {
            MpvEvent::StartFile => {
                self.loaded = false;
                self.ended = false;
            }
            MpvEvent::FileLoaded => {
                self.loaded = true;
                push(EngineEvent::Loaded);
            }
            MpvEvent::PlaybackRestart => push(EngineEvent::Restarted),
            MpvEvent::EndFile { reason, error } => {
                let end = match reason {
                    MpvEnd::Eof => EndReason::Eof,
                    MpvEnd::Error => EndReason::Error(match error {
                        Some(MpvError::UNKNOWN_FORMAT) => EngineErrorKind::UNSUPPORTED_FORMAT,
                        _ => EngineErrorKind::LOADING_FAILED,
                    }),
                    _ => EndReason::Stopped,
                };
                if end == EndReason::Stopped || !self.ended {
                    self.ended = true;
                    push(EngineEvent::Ended(end));
                }
            }
            MpvEvent::Property(change) => self.property(change, signals, commands),
            MpvEvent::SwapChain(swap_chain) => signals.push(Signal::SwapChain(SwapChain::new(
                swap_chain.handle(),
                swap_chain.generation(),
                move || swap_chain.release(),
            ))),
            MpvEvent::CommandFailed {
                what: "loadfile", ..
            } => {
                push(EngineEvent::Ended(EndReason::Error(
                    EngineErrorKind::LOADING_FAILED,
                )));
            }
            MpvEvent::Terminated => signals.push(Signal::Gone),
            // Other failed commands are track or option changes the next state shows; log lines are already redacted and have
            // no reader yet; an overflow is followed by fresh property values.
            MpvEvent::CommandFailed { .. }
            | MpvEvent::Seek
            | MpvEvent::Idle
            | MpvEvent::Log(_)
            | MpvEvent::Overflow
            | MpvEvent::Shutdown => {}
        }
    }

    fn property(
        &mut self,
        change: PropertyChange,
        signals: &mut Vec<Signal>,
        commands: &mut Vec<MpvCommand>,
    ) {
        let mut push = |event: EngineEvent| signals.push(Signal::Event(event));
        match change {
            PropertyChange::TimePos(Some(position)) => {
                self.position = Some(position);
                push(EngineEvent::Position(duration(position)));
            }
            PropertyChange::Duration(Some(length)) => push(EngineEvent::Duration(duration(length))),
            PropertyChange::DemuxerCacheTime(Some(end)) => {
                let ahead = end - self.position.unwrap_or(0.0);
                push(EngineEvent::Buffered(duration(ahead)));
            }
            PropertyChange::Pause(paused) => push(EngineEvent::Paused(paused)),
            PropertyChange::PausedForCache(waiting) => push(EngineEvent::Buffering(waiting)),
            // With `keep-open=yes` the end of the file is `eof-reached` (`current-playback`'s `EngineEvent::Ended`).
            PropertyChange::EofReached(true) if self.loaded && !self.ended => {
                self.ended = true;
                push(EngineEvent::Ended(EndReason::Eof));
            }
            PropertyChange::TrackList(tracks) => {
                self.tracks = tracks;
                self.match_external();
                if let Some(index) = self.want_audio
                    && let Some(id) = self.track_id(TrackKind::Audio, index)
                {
                    self.want_audio = None;
                    commands.push(MpvCommand::Audio(TrackChoice::Id(id)));
                }
                if let Some(index) = self.want_subtitle
                    && let Some(id) = self.track_id(TrackKind::Subtitle, index)
                {
                    self.want_subtitle = None;
                    commands.push(MpvCommand::Subtitle(TrackChoice::Id(id)));
                }
            }
            PropertyChange::VideoParams(params) => {
                self.output.video = params.map(|params| VideoOutput {
                    width: u32::try_from(params.width).unwrap_or(0),
                    height: u32::try_from(params.height).unwrap_or(0),
                    pixel_format: params.pixel_format,
                    primaries: params.primaries,
                    transfer: params.gamma,
                });
            }
            PropertyChange::HwdecCurrent(decoder) => {
                self.output.hardware_decoder =
                    decoder.filter(|name| !name.is_empty() && name != "no");
            }
            _ => {}
        }
    }

    /// Gives each external subtitle track libmpv lists for the first time the oldest server index still waiting.
    fn match_external(&mut self) {
        let new: Vec<i64> = self
            .tracks
            .iter()
            .filter(|track| track.kind == "sub" && track.external)
            .map(|track| track.id)
            .filter(|id| !self.external.iter().any(|(_, known)| known == id))
            .collect();
        for id in new {
            let Some(index) = self.pending_external.pop_front() else {
                break;
            };
            self.external.push((index, id));
        }
    }
}

/// The load-time choice for a track, and the server index to choose once the track list is known.
fn initial(track: EngineTrack) -> (Option<TrackChoice>, Option<u32>) {
    match track {
        EngineTrack::Auto => (None, None),
        EngineTrack::Off => (Some(TrackChoice::Off), None),
        EngineTrack::Stream(index) => (None, Some(index)),
    }
}

#[cfg(feature = "libmpv")]
pub(crate) use real::MpvBackend;

#[cfg(feature = "libmpv")]
mod real {
    use std::time::Duration;

    use current_model::{EngineCapabilities, EngineErrorKind};
    use current_mpv::{
        CompositionSize, DisplayFacts, Engine as Mpv, EngineConfig, EngineError,
        EngineEvent as MpvEvent, LogLevel, Output,
    };
    use current_playback::{Engine, EngineFile, EngineTrack, ExternalSubtitle};
    use tokio::sync::mpsc::UnboundedSender;

    use super::{MpvCommand, MpvMapper, TrackKind, VIDEO_SYNC_VARIABLE, engine_error_kind};
    use crate::player::backend::{Backend, EngineOutput, Signal};

    /// The libmpv engine of one player.
    pub(crate) struct MpvBackend {
        mpv: Option<Mpv>,
        capabilities: EngineCapabilities,
        mapper: MpvMapper,
        /// libmpv quit on its own: nothing more loads.
        gone: bool,
    }

    impl MpvBackend {
        /// Starts libmpv with its event thread sending into `events`. Blocking (mpv starts its threads and its VO): run it on the
        /// blocking pool. Waits for an earlier engine of this process to go, then fails with [`EngineError::Busy`] (one engine per
        /// process for now, ADR 0019).
        pub(crate) fn start(
            size: CompositionSize,
            display: DisplayFacts,
            events: UnboundedSender<MpvEvent>,
        ) -> Result<Self, EngineError> {
            // Composition mode is Windows' D3D11 route (ADR 0007); elsewhere libmpv is for development only (ADR 0011's GTK
            // route comes later) and plays without output.
            let output = if cfg!(windows) {
                Output::Composition { size, display }
            } else {
                Output::Headless
            };
            let config = EngineConfig {
                output,
                user_agent: current_mpv::user_agent(),
                log_level: LogLevel::Info,
                // A test aid only (VIDEO_SYNC_VARIABLE); anything that is not one of mpv's modes is ignored.
                video_sync: std::env::var(VIDEO_SYNC_VARIABLE)
                    .ok()
                    .and_then(|name| current_mpv::video_sync_mode(&name)),
            };
            let mpv = Mpv::start(
                &config,
                Box::new(move |event| {
                    // The player is gone when the receiver is: nothing to tell.
                    let _ = events.send(event);
                }),
            )?;
            let capabilities = mpv.capabilities().engine_capabilities();
            Ok(Self {
                mpv: Some(mpv),
                capabilities,
                mapper: MpvMapper::default(),
                gone: false,
            })
        }

        fn apply(&self, command: MpvCommand) {
            let Some(mpv) = &self.mpv else {
                return;
            };
            // Failures arrive as events (`CommandFailed`).
            let _ = match command {
                MpvCommand::Audio(choice) => mpv.set_audio(choice),
                MpvCommand::Subtitle(choice) => mpv.set_subtitle(choice),
            };
        }
    }

    impl Engine for MpvBackend {
        fn capabilities(&self) -> EngineCapabilities {
            self.capabilities.clone()
        }

        fn load(&mut self, file: EngineFile) -> Result<(), EngineErrorKind> {
            let request = self.mapper.file_request(file)?;
            let mpv = self
                .mpv
                .as_ref()
                .filter(|_| !self.gone)
                .ok_or(EngineErrorKind::ENGINE_GONE)?;
            mpv.load(&request)
                .map_err(|error| engine_error_kind(&error))
        }

        fn add_subtitle(&mut self, subtitle: ExternalSubtitle) {
            let file = self.mapper.subtitle_file(subtitle);
            if let Some(mpv) = &self.mpv {
                let _ = mpv.add_subtitle(&file);
            }
        }

        fn set_paused(&mut self, paused: bool) {
            if let Some(mpv) = &self.mpv {
                let _ = mpv.set_paused(paused);
            }
        }

        fn seek_to(&mut self, position: Duration) {
            if let Some(mpv) = &self.mpv {
                let _ = mpv.seek_to(position.as_secs_f64());
            }
        }

        fn select_audio(&mut self, track: EngineTrack) {
            if let Some(choice) = self.mapper.select(TrackKind::Audio, track) {
                self.apply(MpvCommand::Audio(choice));
            }
        }

        fn select_subtitle(&mut self, track: EngineTrack) {
            if let Some(choice) = self.mapper.select(TrackKind::Subtitle, track) {
                self.apply(MpvCommand::Subtitle(choice));
            }
        }

        fn set_volume(&mut self, volume: u8) {
            if let Some(mpv) = &self.mpv {
                let _ = mpv.set_volume(f64::from(volume));
            }
        }

        fn set_muted(&mut self, muted: bool) {
            if let Some(mpv) = &self.mpv {
                let _ = mpv.set_muted(muted);
            }
        }

        fn stop(&mut self) {
            if let Some(mpv) = &self.mpv {
                let _ = mpv.stop();
            }
        }
    }

    impl Backend for MpvBackend {
        type Raw = MpvEvent;

        fn translate(&mut self, raw: MpvEvent, out: &mut Vec<Signal>) {
            if matches!(raw, MpvEvent::Terminated) {
                self.gone = true;
            }
            let mut commands = Vec::new();
            self.mapper.translate(raw, out, &mut commands);
            for command in commands {
                self.apply(command);
            }
        }

        fn set_display_facts(&mut self, facts: &DisplayFacts) {
            if let Some(mpv) = &self.mpv {
                let _ = mpv.set_display_facts(facts);
            }
        }

        fn set_composition_size(&mut self, size: CompositionSize) {
            if let Some(mpv) = &self.mpv {
                let _ = mpv.set_composition_size(size);
            }
        }

        fn output(&self) -> EngineOutput {
            self.mapper.output()
        }

        fn take_shutdown(&mut self) -> Box<dyn FnOnce() + Send> {
            let mpv = self.mpv.take();
            Box::new(move || {
                if let Some(mpv) = mpv {
                    mpv.shutdown();
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use current_mpv::VideoParams;
    use current_playback::{StreamAuth, StreamCookie as PlaybackCookie, StreamUrl};

    use super::*;

    fn file(url: &str, auth: StreamAuth) -> EngineFile {
        EngineFile {
            url: StreamUrl::new(url),
            auth,
            start: Duration::from_secs(90),
            audio: EngineTrack::Stream(1),
            subtitle: EngineTrack::Stream(3),
            playlist_checked: false,
        }
    }

    fn checked(file: EngineFile) -> EngineFile {
        EngineFile {
            playlist_checked: true,
            ..file
        }
    }

    fn track(id: i64, kind: &str, ff_index: Option<i64>, external: bool) -> Track {
        Track {
            id,
            kind: kind.into(),
            ff_index,
            external,
            ..Track::default()
        }
    }

    fn events(signals: &[Signal]) -> Vec<EngineEvent> {
        signals
            .iter()
            .filter_map(|signal| match signal {
                Signal::Event(event) => Some(event.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_file_moves_into_the_request_with_its_credentials() {
        let mut mapper = MpvMapper::default();
        let auth = StreamAuth {
            cookies: vec![PlaybackCookie {
                name: "gate".into(),
                value: "cookie-value".into(),
                path: "/".into(),
                secure: true,
            }],
            headers: vec![("X-Gate".into(), "header-value".into())],
            secrets: vec!["t0ken".into()],
        };
        let request = mapper
            .file_request(file(
                "https://media.example.com/Videos/1/stream?ApiKey=t0ken",
                auth,
            ))
            .unwrap();
        assert_eq!(
            request.url,
            "https://media.example.com/Videos/1/stream?ApiKey=t0ken"
        );
        assert_eq!(request.start, Some(90.0));
        assert_eq!(request.cookies[0].value, "cookie-value");
        assert_eq!(
            request.headers,
            vec![("X-Gate".to_owned(), "header-value".to_owned())]
        );
        assert_eq!(request.secrets, vec!["t0ken".to_owned()]);
        // Tracks by server index wait for the track list.
        assert_eq!(request.audio, None);
        assert_eq!(request.subtitle, None);
    }

    #[test]
    fn an_unchecked_playlist_with_headers_is_refused_and_a_checked_one_passes() {
        let auth = StreamAuth {
            headers: vec![("Authorization".into(), "Basic header-value".into())],
            ..StreamAuth::default()
        };
        let url = "https://media.example.com/videos/1/master.m3u8?ApiKey=t0ken";
        let mut mapper = MpvMapper::default();
        assert_eq!(
            mapper.file_request(file(url, auth.clone())).unwrap_err(),
            EngineErrorKind::GATEWAY_PLAYLIST_UNCHECKED
        );
        assert!(
            mapper
                .file_request(checked(file(url, auth.clone())))
                .is_ok()
        );
        // Cookies alone are refused too: FFmpeg matches their domain as a suffix, so a look-alike host would get them.
        let cookie_only = StreamAuth {
            cookies: vec![PlaybackCookie {
                name: "gate".into(),
                value: "cookie-value".into(),
                path: "/".into(),
                secure: true,
            }],
            ..StreamAuth::default()
        };
        assert_eq!(
            mapper
                .file_request(file(url, cookie_only.clone()))
                .unwrap_err(),
            EngineErrorKind::GATEWAY_PLAYLIST_UNCHECKED
        );
        assert!(mapper.file_request(checked(file(url, cookie_only))).is_ok());
        // Without credentials, or for a file, nothing to check.
        assert!(
            mapper
                .file_request(file(url, StreamAuth::default()))
                .is_ok()
        );
        assert!(
            mapper
                .file_request(file("https://media.example.com/Videos/1/stream", auth))
                .is_ok()
        );
    }

    #[test]
    fn tracks_are_chosen_by_ff_index_once_listed_and_external_ones_by_order() {
        let mut mapper = MpvMapper::default();
        mapper
            .file_request(file("https://media.example.com/v", StreamAuth::default()))
            .unwrap();
        let (mut signals, mut commands) = (Vec::new(), Vec::new());
        mapper.translate(
            MpvEvent::Property(PropertyChange::TrackList(vec![
                track(1, "video", Some(0), false),
                track(1, "audio", Some(1), false),
                track(2, "audio", Some(2), false),
                track(1, "sub", Some(3), false),
            ])),
            &mut signals,
            &mut commands,
        );
        assert_eq!(
            commands,
            vec![
                MpvCommand::Audio(TrackChoice::Id(1)),
                MpvCommand::Subtitle(TrackChoice::Id(1))
            ]
        );
        assert_eq!(
            mapper.select(TrackKind::Audio, EngineTrack::Stream(2)),
            Some(TrackChoice::Id(2))
        );
        assert_eq!(
            mapper.select(TrackKind::Subtitle, EngineTrack::Off),
            Some(TrackChoice::Off)
        );
        // An external subtitle is chosen once libmpv lists it.
        mapper.subtitle_file(ExternalSubtitle {
            index: 7,
            format: None,
            data: b"1".to_vec(),
            title: None,
            language: None,
            select: true,
        });
        assert_eq!(
            mapper.select(TrackKind::Subtitle, EngineTrack::Stream(7)),
            None
        );
        commands.clear();
        mapper.translate(
            MpvEvent::Property(PropertyChange::TrackList(vec![
                track(1, "audio", Some(1), false),
                track(1, "sub", Some(3), false),
                track(2, "sub", None, true),
            ])),
            &mut signals,
            &mut commands,
        );
        assert_eq!(commands, vec![MpvCommand::Subtitle(TrackChoice::Id(2))]);
    }

    #[test]
    fn a_subtitle_fetched_at_start_is_chosen_once_listed() {
        // The start with a fetched subtitle: the file loads with it off, it is added with `select`, and nothing else chooses it.
        let mut mapper = MpvMapper::default();
        let mut start = file("https://media.example.com/v", StreamAuth::default());
        start.subtitle = EngineTrack::Off;
        let request = mapper.file_request(start).unwrap();
        assert_eq!(request.subtitle, Some(TrackChoice::Off));
        let added = mapper.subtitle_file(ExternalSubtitle {
            index: 3,
            format: None,
            data: b"1".to_vec(),
            title: None,
            language: None,
            select: true,
        });
        assert!(added.select);
        let (mut signals, mut commands) = (Vec::new(), Vec::new());
        mapper.translate(
            MpvEvent::Property(PropertyChange::TrackList(vec![
                track(1, "video", Some(0), false),
                track(1, "sub", None, true),
            ])),
            &mut signals,
            &mut commands,
        );
        assert_eq!(commands, vec![MpvCommand::Subtitle(TrackChoice::Id(1))]);

        // One added only to be offered is not chosen.
        let mut mapper = MpvMapper::default();
        mapper
            .file_request(file("https://media.example.com/v", StreamAuth::default()))
            .unwrap();
        mapper.subtitle_file(ExternalSubtitle {
            index: 4,
            format: None,
            data: b"1".to_vec(),
            title: None,
            language: None,
            select: false,
        });
        commands.clear();
        mapper.translate(
            MpvEvent::Property(PropertyChange::TrackList(vec![track(1, "sub", None, true)])),
            &mut signals,
            &mut commands,
        );
        assert!(commands.is_empty(), "{commands:?}");
    }

    #[test]
    fn events_map_to_the_controllers() {
        let mut mapper = MpvMapper::default();
        let (mut signals, mut commands) = (Vec::new(), Vec::new());
        for event in [
            MpvEvent::StartFile,
            MpvEvent::FileLoaded,
            MpvEvent::Property(PropertyChange::Duration(Some(600.0))),
            MpvEvent::PlaybackRestart,
            MpvEvent::Property(PropertyChange::TimePos(Some(12.5))),
            MpvEvent::Property(PropertyChange::DemuxerCacheTime(Some(42.5))),
            MpvEvent::Property(PropertyChange::Pause(true)),
            MpvEvent::Property(PropertyChange::PausedForCache(true)),
            MpvEvent::Property(PropertyChange::EofReached(true)),
            // Once per file.
            MpvEvent::Property(PropertyChange::EofReached(true)),
            MpvEvent::EndFile {
                reason: MpvEnd::Stop,
                error: None,
            },
        ] {
            mapper.translate(event, &mut signals, &mut commands);
        }
        assert_eq!(
            events(&signals),
            vec![
                EngineEvent::Loaded,
                EngineEvent::Duration(Duration::from_secs(600)),
                EngineEvent::Restarted,
                EngineEvent::Position(Duration::from_millis(12_500)),
                EngineEvent::Buffered(Duration::from_secs(30)),
                EngineEvent::Paused(true),
                EngineEvent::Buffering(true),
                EngineEvent::Ended(EndReason::Eof),
                EngineEvent::Ended(EndReason::Stopped),
            ]
        );
        signals.clear();
        mapper.translate(MpvEvent::StartFile, &mut signals, &mut commands);
        mapper.translate(
            MpvEvent::EndFile {
                reason: MpvEnd::Error,
                error: Some(MpvError::UNKNOWN_FORMAT),
            },
            &mut signals,
            &mut commands,
        );
        mapper.translate(MpvEvent::Terminated, &mut signals, &mut commands);
        assert_eq!(
            events(&signals),
            vec![EngineEvent::Ended(EndReason::Error(
                EngineErrorKind::UNSUPPORTED_FORMAT
            ))]
        );
        assert!(matches!(signals.last(), Some(Signal::Gone)));
    }

    #[test]
    fn output_facts_are_kept_without_any_url() {
        let mut mapper = MpvMapper::default();
        let (mut signals, mut commands) = (Vec::new(), Vec::new());
        mapper.translate(
            MpvEvent::Property(PropertyChange::HwdecCurrent(Some("d3d11va".into()))),
            &mut signals,
            &mut commands,
        );
        mapper.translate(
            MpvEvent::Property(PropertyChange::VideoParams(Some(VideoParams {
                width: 3840,
                height: 2160,
                pixel_format: Some("p010".into()),
                primaries: Some("bt.2020".into()),
                gamma: Some("pq".into()),
            }))),
            &mut signals,
            &mut commands,
        );
        let output = mapper.output();
        assert_eq!(output.hardware_decoder.as_deref(), Some("d3d11va"));
        let video = output.video.unwrap();
        assert_eq!((video.width, video.height), (3840, 2160));
        assert_eq!(video.transfer.as_deref(), Some("pq"));
    }
}
