//! Fakes for the tests: an engine that records its commands, a server with scripted answers that records its reports with the fake
//! clock's time, a clock moved by hand, and an executor for futures that never wait (the fakes answer at once).

use std::collections::VecDeque;
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use current_model::{
    Codec, CodecSupport, Container, EngineCapabilities, EngineErrorKind, ItemId, ItemRef,
    ItemSummary, MediaKind, MediaSource, MediaStream, PlaybackPreferences, PlaybackProblem,
    RouteReason, Segment, ServerKey, ServerOffer, ServerRef, ServiceType, StreamAction, StreamKind,
    StreamProtocol, SubtitleDelivery, SubtitleMode, TranscodingFacts, UserState,
};

use crate::clock::Clock;
use crate::engine::{Engine, EngineFile, EngineTrack, ExternalSubtitle};
use crate::server::{
    MediaServer, PlanRequest, ProgressReport, ServerAnswer, StopReport, TranscodePermissions,
};
use crate::stream::{EngineStream, StreamAuth, StreamUrl};

/// The token, cookie and header values the fake server hands out; no Debug output may show them.
/// The session token in the server's URLs and the route token in relayed ones.
pub const SECRETS: [&str; 2] = ["test-token-1234", "test-route-5678"];

/// Runs a future that never waits.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("a fake waited"),
    }
}

#[derive(Clone, Debug, Default)]
pub struct FakeClock(Arc<Mutex<Duration>>);

impl FakeClock {
    pub fn advance(&self, by: Duration) {
        *lock(&self.0) += by;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        *lock(&self.0)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Load(EngineFile),
    AddSubtitle(ExternalSubtitle),
    Paused(bool),
    Seek(Duration),
    Audio(EngineTrack),
    Subtitle(EngineTrack),
    Volume(u8),
    Muted(bool),
    /// The rate in hundredths (150 for 1.5).
    Speed(u32),
    Stop,
}

#[derive(Debug, Default)]
pub struct FakeEngine {
    pub commands: Vec<Command>,
    /// What it plays; `None` for the default (MKV and MP4, H.264, AAC).
    pub caps: Option<EngineCapabilities>,
    pub refuse_loads: bool,
    /// Set by `stop`, cleared by `load`: whether nothing is loaded (shared with [`ServerState::engine_unloaded`]).
    pub unloaded: Arc<AtomicBool>,
}

impl FakeEngine {
    pub fn take(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }
}

impl Engine for FakeEngine {
    fn capabilities(&self) -> EngineCapabilities {
        self.caps.clone().unwrap_or_else(|| EngineCapabilities {
            containers: vec![Container::MKV, Container::MP4],
            video: vec![CodecSupport::any(Codec::H264)],
            audio: vec![CodecSupport::any(Codec::AAC)],
            subtitles: vec![],
            external_subtitles: true,
        })
    }

    fn load(&mut self, file: EngineFile) -> Result<(), EngineErrorKind> {
        if self.refuse_loads {
            return Err(EngineErrorKind::LOADING_FAILED);
        }
        self.commands.push(Command::Load(file));
        self.unloaded.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn add_subtitle(&mut self, subtitle: ExternalSubtitle) {
        self.commands.push(Command::AddSubtitle(subtitle));
    }

    fn set_paused(&mut self, paused: bool) {
        self.commands.push(Command::Paused(paused));
    }

    fn seek_to(&mut self, position: Duration) {
        self.commands.push(Command::Seek(position));
    }

    fn select_audio(&mut self, track: EngineTrack) {
        self.commands.push(Command::Audio(track));
    }

    fn select_subtitle(&mut self, track: EngineTrack) {
        self.commands.push(Command::Subtitle(track));
    }

    fn set_volume(&mut self, volume: u8) {
        self.commands.push(Command::Volume(volume));
    }

    fn set_muted(&mut self, muted: bool) {
        self.commands.push(Command::Muted(muted));
    }

    fn set_speed(&mut self, speed: f64) {
        self.commands.push(Command::Speed((speed * 100.0).round() as u32));
    }

    fn stop(&mut self) {
        self.commands.push(Command::Stop);
        self.unloaded.store(true, Ordering::SeqCst);
    }
}

/// A fake engine the test keeps a handle on after handing it to the controller (as the native engine, which the controller boxes).
#[derive(Clone, Debug, Default)]
pub struct SharedEngine(pub Arc<Mutex<FakeEngine>>);

impl SharedEngine {
    pub fn take(&self) -> Vec<Command> {
        lock(&self.0).take()
    }
}

impl Engine for SharedEngine {
    fn capabilities(&self) -> EngineCapabilities {
        lock(&self.0).capabilities()
    }

    fn load(&mut self, file: EngineFile) -> Result<(), EngineErrorKind> {
        lock(&self.0).load(file)
    }

    fn add_subtitle(&mut self, subtitle: ExternalSubtitle) {
        lock(&self.0).add_subtitle(subtitle);
    }

    fn set_paused(&mut self, paused: bool) {
        lock(&self.0).set_paused(paused);
    }

    fn seek_to(&mut self, position: Duration) {
        lock(&self.0).seek_to(position);
    }

    fn select_audio(&mut self, track: EngineTrack) {
        lock(&self.0).select_audio(track);
    }

    fn select_subtitle(&mut self, track: EngineTrack) {
        lock(&self.0).select_subtitle(track);
    }

    fn set_volume(&mut self, volume: u8) {
        lock(&self.0).set_volume(volume);
    }

    fn set_muted(&mut self, muted: bool) {
        lock(&self.0).set_muted(muted);
    }

    fn set_speed(&mut self, speed: f64) {
        lock(&self.0).set_speed(speed);
    }

    fn stop(&mut self) {
        lock(&self.0).stop();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    Start(ProgressReport),
    Progress(ProgressReport),
    Stopped(StopReport),
}

#[derive(Debug)]
pub struct ServerState {
    /// Answers for the next PlaybackInfo calls, in order; when empty, a direct play answer.
    pub answers: VecDeque<Result<ServerAnswer, PlaybackProblem>>,
    pub requests: Vec<(ItemRef, PlanRequest)>,
    /// The capabilities each request's device profile was made from.
    pub profiles: Vec<EngineCapabilities>,
    /// Each report with the clock's time.
    pub reports: Vec<(Duration, Report)>,
    pub segments: Vec<Segment>,
    pub segment_calls: u32,
    pub next: Option<ItemSummary>,
    pub next_calls: u32,
    pub preferences: PlaybackPreferences,
    pub permissions: TranscodePermissions,
    pub facts: Option<TranscodingFacts>,
    pub subtitle_fetches: Vec<u32>,
    pub local: bool,
    /// The server URLs given to `stream_url`, exposed.
    pub streams: Vec<String>,
    /// Whether each of `streams` was given as HLS.
    pub hls_streams: Vec<bool>,
    /// Whether `stream_url` relays (an address with gateway credentials).
    pub relay: bool,
    /// What `stream_url` fails with; `None` gives a URL.
    pub stream_problem: Option<PlaybackProblem>,
    /// Whether a stream from `stream_url` is live: not yet ended or replaced.
    pub stream_live: bool,
    /// How many times `end_stream` was called.
    pub stream_ends: u32,
    /// The engine's [`FakeEngine::unloaded`], when the test shares it.
    pub engine_unloaded: Option<Arc<AtomicBool>>,
    /// For each stop report, whether the engine had nothing loaded when it was sent.
    pub unloaded_at_stop: Vec<bool>,
}

#[derive(Clone, Debug)]
pub struct FakeServer {
    pub state: Arc<Mutex<ServerState>>,
    clock: FakeClock,
}

impl FakeServer {
    pub fn new(clock: FakeClock) -> Self {
        Self {
            state: Arc::new(Mutex::new(ServerState {
                answers: VecDeque::new(),
                requests: Vec::new(),
                profiles: Vec::new(),
                reports: Vec::new(),
                segments: Vec::new(),
                segment_calls: 0,
                next: None,
                next_calls: 0,
                preferences: PlaybackPreferences {
                    audio_language: None,
                    play_default_audio_track: true,
                    subtitle_language: None,
                    subtitle_mode: SubtitleMode::DEFAULT,
                    next_episode_autoplay: true,
                    remember_audio: true,
                    remember_subtitles: true,
                },
                permissions: TranscodePermissions::default(),
                facts: None,
                subtitle_fetches: Vec::new(),
                local: true,
                streams: Vec::new(),
                hls_streams: Vec::new(),
                relay: false,
                stream_problem: None,
                stream_live: false,
                stream_ends: 0,
                engine_unloaded: None,
                unloaded_at_stop: Vec::new(),
            })),
            clock,
        }
    }

    pub fn state(&self) -> MutexGuard<'_, ServerState> {
        lock(&self.state)
    }

    pub fn answer(&self, answer: Result<ServerAnswer, PlaybackProblem>) {
        self.state().answers.push_back(answer);
    }

    pub fn take_reports(&self) -> Vec<(Duration, Report)> {
        std::mem::take(&mut self.state().reports)
    }

    fn record(&self, report: Report) {
        let now = self.clock.now();
        self.state().reports.push((now, report));
    }
}

impl MediaServer for FakeServer {
    async fn playback_info(
        &self,
        item: &ItemRef,
        request: &PlanRequest,
        capabilities: &EngineCapabilities,
    ) -> Result<ServerAnswer, PlaybackProblem> {
        let mut state = self.state();
        state.requests.push((item.clone(), request.clone()));
        state.profiles.push(capabilities.clone());
        let count = state.requests.len();
        let mut answer = state
            .answers
            .pop_front()
            .unwrap_or_else(|| Ok(answer(ServerOffer::DirectPlay, vec![])))?;
        answer.play_session_id = format!("ps-{count}");
        Ok(answer)
    }

    async fn report_start(&self, report: &ProgressReport) -> Result<(), PlaybackProblem> {
        self.record(Report::Start(report.clone()));
        Ok(())
    }

    async fn report_progress(&self, report: &ProgressReport) -> Result<(), PlaybackProblem> {
        self.record(Report::Progress(report.clone()));
        Ok(())
    }

    async fn report_stopped(&self, report: &StopReport) -> Result<(), PlaybackProblem> {
        {
            let mut state = self.state();
            let unloaded = state
                .engine_unloaded
                .as_ref()
                .is_some_and(|unloaded| unloaded.load(Ordering::SeqCst));
            state.unloaded_at_stop.push(unloaded);
        }
        self.record(Report::Stopped(report.clone()));
        Ok(())
    }

    async fn segments(&self, _item: &ItemRef) -> Result<Vec<Segment>, PlaybackProblem> {
        let mut state = self.state();
        state.segment_calls += 1;
        Ok(state.segments.clone())
    }

    async fn next_episode(&self, _item: &ItemRef) -> Result<Option<ItemSummary>, PlaybackProblem> {
        let mut state = self.state();
        state.next_calls += 1;
        Ok(state.next.clone())
    }

    async fn preferences(&self) -> Result<PlaybackPreferences, PlaybackProblem> {
        Ok(self.state().preferences.clone())
    }

    async fn permissions(&self) -> Result<TranscodePermissions, PlaybackProblem> {
        Ok(self.state().permissions)
    }

    async fn transcoding_facts(&self) -> Result<Option<TranscodingFacts>, PlaybackProblem> {
        Ok(self.state().facts.clone())
    }

    async fn external_subtitle(
        &self,
        _item: &ItemRef,
        _source: &MediaSource,
        stream: &MediaStream,
    ) -> Result<Vec<u8>, PlaybackProblem> {
        self.state().subtitle_fetches.push(stream.index);
        Ok(b"1\n00:00:01,000 --> 00:00:02,000\nHello\n".to_vec())
    }

    fn stream_url(
        &self,
        _item: &ItemRef,
        url: &StreamUrl,
        hls: bool,
    ) -> Result<EngineStream, PlaybackProblem> {
        let mut state = self.state();
        state.streams.push(url.expose().to_owned());
        state.hls_streams.push(hls);
        state.stream_live = false;
        if let Some(problem) = state.stream_problem.clone() {
            return Err(problem);
        }
        state.stream_live = true;
        Ok(if state.relay {
            EngineStream {
                url: StreamUrl::new(format!(
                    "http://127.0.0.1:50123/{}/Videos/e1/stream?static=true",
                    SECRETS[1]
                )),
                auth: StreamAuth {
                    secrets: vec![SECRETS[1].into()],
                },
                relayed: true,
            }
        } else {
            EngineStream {
                url: url.clone(),
                auth: StreamAuth {
                    secrets: vec![SECRETS[0].into()],
                },
                relayed: false,
            }
        })
    }

    fn end_stream(&self) {
        let mut state = self.state();
        state.stream_live = false;
        state.stream_ends += 1;
    }

    fn is_local_address(&self) -> bool {
        self.state().local
    }
}

pub fn item(id: &str) -> ItemRef {
    ItemRef::new(
        ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
        ItemId::new(id),
    )
}

pub fn summary(id: &str) -> ItemSummary {
    ItemSummary {
        item: item(id),
        kind: MediaKind::EPISODE,
        name: "Episode".into(),
        year: None,
        runtime: None,
        artwork: vec![],
        user: UserState::default(),
        episode: None,
        child_count: None,
    }
}

pub fn stream(index: u32, kind: StreamKind) -> MediaStream {
    MediaStream {
        index,
        kind,
        codec: None,
        profile: None,
        level: None,
        language: None,
        title: None,
        display_title: None,
        is_default: false,
        is_forced: false,
        is_hearing_impaired: false,
        is_external: false,
        bitrate: None,
        video: None,
        audio: None,
        subtitle_delivery: None,
    }
}

fn subtitle(index: u32, delivery: SubtitleDelivery, external: bool) -> MediaStream {
    MediaStream {
        codec: Some(Codec::new("subrip")),
        is_external: external,
        subtitle_delivery: Some(delivery),
        ..stream(index, StreamKind::SUBTITLE)
    }
}

/// A source with video 0, audio 1 and 2 (the server picks 1), an embedded subtitle 3, an embedded subtitle 4 the server burns in on a
/// stream, and an external subtitle 5; 20 Mbit/s, 1 hour.
pub fn source() -> MediaSource {
    MediaSource {
        id: "source-1".into(),
        name: None,
        container: Some("mkv".into()),
        size: None,
        bitrate: Some(20_000_000),
        duration: Some(Duration::from_secs(3600)),
        streams: vec![
            stream(0, StreamKind::VIDEO),
            stream(1, StreamKind::AUDIO),
            stream(2, StreamKind::AUDIO),
            subtitle(3, SubtitleDelivery::EMBED, false),
            subtitle(4, SubtitleDelivery::ENCODE, false),
            subtitle(5, SubtitleDelivery::EXTERNAL, true),
        ],
        default_audio: Some(1),
        default_subtitle: None,
        has_segments: false,
        is_remote: false,
    }
}

pub fn hls(video: StreamAction, audio: StreamAction) -> ServerOffer {
    ServerOffer::Stream {
        protocol: StreamProtocol::Hls,
        container: Some("mp4".into()),
        video,
        audio,
    }
}

pub fn answer(offer: ServerOffer, reasons: Vec<RouteReason>) -> ServerAnswer {
    ServerAnswer {
        play_session_id: "ps-1".into(),
        source: source(),
        offer,
        reasons,
        url: StreamUrl::new(format!(
            "https://media.example.com/Videos/e1/stream?static=true&ApiKey={}",
            SECRETS[0]
        )),
    }
}
