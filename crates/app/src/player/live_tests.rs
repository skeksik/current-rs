//! The player on a real libmpv through the loopback relay (ADR 0020 task 4), against the Jellyfin stand-in behind its gateway modes:
//! direct play and HLS through a portal, a Basic and a header gateway; what libmpv sent to the relay (no cookie, gateway header or
//! `ApiKey`) and what the stand-in received upstream (the gateway credential and the token, added by the core); a hostile playlist
//! naming another port or host; a seek's new range; every upstream connection closed on stop; the engine's and the controller's
//! reaction to refusals, failing segments, a cut connection and a re-plan; and plain-playlist bodies on a relayed HLS route, which
//! libmpv opens with references allowed.
//!
//! The media is generated: a WAV tone (FFmpeg's built-in `wav` demuxer and `pcm_s16le` decoder), served whole for direct play and
//! cut into segments for HLS. Under mpv FFmpeg's `extension_picky` check of segment names is off (mpv v0.41.0 `demux/demux_lavf.c`
//! sets `extension_picky=0` for `hls`, and FFmpeg 9.0.2 `libavformat/hls.c`'s `test_segment` then returns at once; both read
//! 2026-10-26). What keeps a segment from opening anything is elsewhere in `hls.c`: a segment's own demuxer gets `nested_io_open`,
//! which refuses every open ("Opening this file was forbidden for security reasons", `AVERROR(EPERM)`), and `open_url` takes only
//! http(s), file (with `allowed_extensions`), data and crypto URLs.
//!
//! What libmpv sent is read on a tap: a recording forwarder between libmpv and the relay, which the route's URL points at in this
//! crate's tests only (`server.rs`). What libmpv reported is recorded from the engine's events (`mpv.rs`), and each case prints it
//! with the controller's answer (`observed:` lines; CI runs these tests with `--nocapture`).
//!
//! They need the `libmpv` feature (the "mpv (Windows)" CI job links the D6 build; on Linux a system libmpv works too) and are
//! ignored without it. Nothing leaves 127.0.0.1.

// Without libmpv the tests are ignored and only `through_tap` is called (it passes URLs on unchanged): the tap is compiled, unused.
#![cfg_attr(not(feature = "libmpv"), allow(dead_code))]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A request as the tap read it from libmpv.
#[derive(Clone, Debug)]
pub(super) struct TapSeen {
    pub(super) method: String,
    /// The path and query.
    pub(super) target: String,
    /// Names lowercased.
    pub(super) headers: Vec<(String, String)>,
}

impl TapSeen {
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A forwarder on 127.0.0.1 that records every request head and passes it to the relay with the relay's `Host`.
pub(super) struct Tap {
    port: u16,
    relay: Mutex<Option<u16>>,
    seen: Mutex<Vec<TapSeen>>,
}

/// The tap the route URLs point at while a live test runs.
static TAP: Mutex<Option<Arc<Tap>>> = Mutex::new(None);

/// The relay URL `url` (`http://127.0.0.1:<relay port>/...`), on the tap when one is installed.
pub(super) fn through_tap(url: String) -> String {
    let Some(tap) = lock(&TAP).clone() else {
        return url;
    };
    let Some((port, path)) = url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.split_at_checked(rest.find('/')?))
    else {
        return url;
    };
    *lock(&tap.relay) = port.parse().ok();
    format!("http://127.0.0.1:{}{path}", tap.port)
}

impl Tap {
    /// Starts a tap on the current runtime and points route URLs at it until the guard drops.
    pub(super) async fn install() -> TapGuard {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let tap = Arc::new(Tap {
            port: listener.local_addr().expect("address").port(),
            relay: Mutex::new(None),
            seen: Mutex::new(Vec::new()),
        });
        let serving = tap.clone();
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                tokio::spawn(serving.clone().forward(client));
            }
        });
        *lock(&TAP) = Some(tap.clone());
        TapGuard { tap, task }
    }

    pub(super) fn seen(&self) -> Vec<TapSeen> {
        lock(&self.seen).clone()
    }

    /// One client connection: request heads in (GET and HEAD have no body), the relay's bytes out as they come.
    async fn forward(self: Arc<Self>, client: TcpStream) {
        let Some(port) = *lock(&self.relay) else {
            return;
        };
        let Ok(relay) = TcpStream::connect(("127.0.0.1", port)).await else {
            return;
        };
        let (mut client_in, mut client_out) = client.into_split();
        let (mut relay_in, mut relay_out) = relay.into_split();
        let back = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut relay_in, &mut client_out).await;
            let _ = client_out.shutdown().await;
        });
        let mut data = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            while let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                let head: Vec<u8> = data.drain(..end + 4).collect();
                let head = String::from_utf8_lossy(&head).into_owned();
                let mut lines = head.split("\r\n");
                let first = lines.next().unwrap_or_default();
                let mut parts = first.split(' ');
                let method = parts.next().unwrap_or_default().to_owned();
                let target = parts.next().unwrap_or_default().to_owned();
                let headers: Vec<(String, String)> = lines
                    .filter_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
                    })
                    .collect();
                let mut out = format!("{first}\r\n");
                for (name, value) in &headers {
                    let value = if name == "host" {
                        format!("127.0.0.1:{port}")
                    } else {
                        value.clone()
                    };
                    out.push_str(&format!("{name}: {value}\r\n"));
                }
                out.push_str("\r\n");
                lock(&self.seen).push(TapSeen {
                    method,
                    target,
                    headers,
                });
                if relay_out.write_all(out.as_bytes()).await.is_err() {
                    back.abort();
                    return;
                }
            }
            match client_in.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => data.extend_from_slice(&buffer[..read]),
            }
        }
        let _ = relay_out.shutdown().await;
        let _ = back.await;
    }
}

/// Removes the tap when the test ends.
pub(super) struct TapGuard {
    tap: Arc<Tap>,
    task: tokio::task::JoinHandle<()>,
}

impl std::ops::Deref for TapGuard {
    type Target = Tap;

    fn deref(&self) -> &Tap {
        &self.tap
    }
}

impl Drop for TapGuard {
    fn drop(&mut self) {
        self.task.abort();
        *lock(&TAP) = None;
    }
}

/// What libmpv reported, as the engine's events, in order: every event but property changes, and its warnings and errors.
#[cfg(feature = "libmpv")]
static REPORTED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Records one of libmpv's events (called by the engine on its event path, in this crate's tests only).
#[cfg(feature = "libmpv")]
pub(crate) fn record(event: &current_mpv::EngineEvent) {
    use current_mpv::EngineEvent;
    let line = match event {
        EngineEvent::Log(line) if matches!(line.level.as_str(), "fatal" | "error" | "warn") => {
            format!("log {} [{}] {}", line.level, line.prefix, line.text)
        }
        EngineEvent::Log(_) | EngineEvent::Property(_) | EngineEvent::Overflow => return,
        other => format!("{other:?}"),
    };
    lock(&REPORTED).push(line);
}

/// Declares a test that runs [`live`]'s case, ignored without libmpv.
macro_rules! libmpv_test {
    ($name:ident, $case:expr) => {
        #[tokio::test]
        #[cfg_attr(
            not(feature = "libmpv"),
            ignore = "needs libmpv: run with --features libmpv (the \"mpv (Windows)\" CI job links the D6 build)"
        )]
        async fn $name() {
            #[cfg(feature = "libmpv")]
            {
                #[allow(unused_imports)]
                use jellyfin_stand_in::Gate;
                use live::*;
                $case.await;
            }
            #[cfg(not(feature = "libmpv"))]
            panic!("built without the libmpv feature");
        }
    };
}

libmpv_test!(
    direct_play_through_a_portal_gateway,
    direct_play(Gate::Portal)
);
libmpv_test!(
    direct_play_through_a_basic_gateway,
    direct_play(Gate::Basic)
);
libmpv_test!(
    direct_play_through_a_header_gateway,
    direct_play(Gate::Header)
);
libmpv_test!(hls_through_a_portal_gateway, hls(Gate::Portal));
libmpv_test!(hls_through_a_basic_gateway, hls(Gate::Basic));
libmpv_test!(hls_through_a_header_gateway, hls(Gate::Header));
libmpv_test!(
    a_hostile_playlist_naming_another_port_or_host_is_refused,
    hostile_playlist()
);
libmpv_test!(a_seek_opens_a_new_range_upstream, seek_opens_a_new_range());
libmpv_test!(
    stopping_closes_every_upstream_connection,
    stopping_closes_every_upstream_connection()
);
libmpv_test!(
    a_basic_gateway_401_on_a_segment_mid_play_asks_for_the_gateway,
    refused_mid_play_hls(Gate::Basic)
);
libmpv_test!(
    a_header_gateway_403_on_a_segment_mid_play_asks_for_the_gateway,
    refused_mid_play_hls(Gate::Header)
);
libmpv_test!(
    a_portal_page_on_a_segment_mid_play_asks_for_the_gateway,
    refused_mid_play_hls(Gate::Portal)
);
libmpv_test!(
    a_gateway_refusal_of_a_seek_asks_for_the_gateway,
    refused_seek()
);
libmpv_test!(a_5xx_on_one_segment_fails_at_once, failing_segment(500));
libmpv_test!(a_404_on_one_segment_fails_at_once, failing_segment(404));
libmpv_test!(a_connection_cut_mid_body_is_never_an_end, cut_mid_body());
libmpv_test!(
    re_plans_mid_play_take_no_end_of_the_old_file_for_the_new,
    re_plans()
);
libmpv_test!(
    a_plain_playlist_on_a_relayed_hls_route_opens_nothing_directly,
    plain_playlists()
);

#[cfg(feature = "libmpv")]
mod live {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use current_model::{ItemId, ItemRef, PlaybackProblem, PlayerState, ServerRef};
    use current_playback::{PlayerStatus, Quality};
    use jellyfin_stand_in::{
        DIRECT_PLAY_ITEM, GATE_BASIC, GATE_COOKIE, GATE_COOKIE_VALUE, GATE_HEADER,
        GATE_HEADER_VALUE, GATE_PASSWORD, GATE_USER, Gate, PASSWORD, PLAY_SESSION, StandIn,
        StreamSeen, TRANSCODE_ITEM, TRANSCODE_ITEM_PATH_ID, USER_NAME, master_playlist_path,
        media_playlist_path,
    };
    use tokio::net::TcpListener;
    use tokio::sync::watch;

    use super::{REPORTED, Tap, TapGuard, TapSeen, lock};
    use crate::player::{CompositionSize, DisplayFacts, Player};
    use crate::{AddServerState, App, CookieInput, HeaderInput, SignInState};

    /// One engine per process: the tests take turns.
    static ENGINE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Long enough for libmpv on a loaded CI runner; every wait ends as soon as its condition holds.
    const WAIT: Duration = Duration::from_secs(30);
    /// The tone: 8 kHz mono 16-bit PCM, 16,000 bytes a second.
    const RATE: u32 = 8000;
    const BYTES_PER_SECOND: usize = 16_000;
    const WAV_HEADER: usize = 44;
    /// Each HLS segment's length.
    const SEGMENT_SECONDS: usize = 2;

    const SIZE: CompositionSize = CompositionSize {
        width: 1280,
        height: 720,
    };

    fn display() -> DisplayFacts {
        DisplayFacts {
            hdr: false,
            peak_nits: 300.0,
            sdr_white_nits: 200.0,
            refresh_hz: 60.0,
        }
    }

    /// `seconds` of a 440 Hz tone in a WAV container.
    fn wav(seconds: usize) -> Vec<u8> {
        let samples = RATE as usize * seconds;
        let data_len = (samples * 2) as u32;
        let mut out = Vec::with_capacity(WAV_HEADER + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&RATE.to_le_bytes());
        out.extend_from_slice(&(RATE * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..samples {
            let sample =
                ((i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 8000.0) as i16;
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }

    fn direct_path() -> String {
        format!("/Videos/{DIRECT_PLAY_ITEM}/stream")
    }

    fn segment_path(n: usize) -> String {
        format!("/videos/{TRANSCODE_ITEM_PATH_ID}/hls1/main/{n}.wav")
    }

    /// A VOD media playlist of `count` WAV segments, relative, in the shape of the stand-in's own.
    fn media_playlist(count: usize) -> String {
        let mut out = format!(
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{SEGMENT_SECONDS}\n#EXT-X-MEDIA-SEQUENCE:0\n"
        );
        for n in 0..count {
            out.push_str(&format!(
                "#EXTINF:{SEGMENT_SECONDS}.0,\nhls1/main/{n}.wav?PlaySessionId={PLAY_SESSION}\n"
            ));
        }
        out.push_str("#EXT-X-ENDLIST\n");
        out
    }

    /// A playlist of `count` WAV segments on the stand-in: one tone cut into pieces, the header in the first, so the segments
    /// together are one WAV file.
    fn serve_hls(stand_in: &StandIn, count: usize) {
        let tone = wav(count * SEGMENT_SECONDS);
        let piece = SEGMENT_SECONDS * BYTES_PER_SECOND;
        for n in 0..count {
            let start = if n == 0 { 0 } else { WAV_HEADER + n * piece };
            let end = (WAV_HEADER + (n + 1) * piece).min(tone.len());
            stand_in.set_stream(&segment_path(n), "audio/wav", tone[start..end].to_vec());
        }
        stand_in.set_playlist(&media_playlist_path(), &media_playlist(count));
    }

    /// A player on the stand-in behind `gate`, with the tap in front of the relay.
    struct Run {
        player: Player,
        stand_in: StandIn,
        server: ServerRef,
        tap: TapGuard,
        _app: App,
        // Last: the next test's engine waits for this one's to go.
        _turn: tokio::sync::MutexGuard<'static, ()>,
    }

    impl Run {
        async fn start(gate: Gate) -> Run {
            let turn = ENGINE.lock().await;
            lock(&REPORTED).clear();
            let stand_in = StandIn::start().await.unwrap();
            let tap = Tap::install().await;
            let app = App::new();
            let server = signed_in(&stand_in, &app, gate).await;
            let player = app.open_player(SIZE, display()).await.expect("a player");
            Run {
                player,
                stand_in,
                server,
                tap,
                _app: app,
                _turn: turn,
            }
        }

        fn item(&self, id: &str) -> ItemRef {
            ItemRef::new(self.server.clone(), ItemId::new(id))
        }

        async fn play(&self, id: &str) {
            self.player
                .play(self.item(id), Duration::ZERO)
                .await
                .expect("play");
        }

        /// Waits until `check` holds on the player's state, at most [`WAIT`].
        async fn until(
            &self,
            what: &str,
            mut check: impl FnMut(&PlayerStatus) -> bool,
        ) -> PlayerStatus {
            let mut state: watch::Receiver<PlayerStatus> = self.player.state();
            let wait = state.wait_for(|status| check(status));
            match tokio::time::timeout(WAIT, wait).await {
                Ok(Ok(status)) => status.clone(),
                _ => panic!(
                    "timed out waiting for {what}: {:?}; libmpv reported: {:#?}",
                    self.player.state().borrow().state,
                    reported()
                ),
            }
        }

        /// Waits until the player plays past `seconds`.
        async fn playing_past(&self, seconds: u64) -> PlayerStatus {
            self.until(&format!("playing past {seconds} s"), |status| {
                status.state == PlayerState::Playing
                    && status.position >= Duration::from_secs(seconds)
            })
            .await
        }

        /// Waits until playback fails or ends.
        async fn finished(&self) -> PlayerState {
            self.until("a failure or an end", |status| {
                matches!(
                    status.state,
                    PlayerState::Failed { .. } | PlayerState::Ended
                )
            })
            .await
            .state
        }

        /// Prints what libmpv reported and what the controller did, for the PR.
        fn observed(&self, case: &str, controller: &str) {
            eprintln!(
                "observed: {case}\n  controller: {controller}\n  libmpv: {:#?}",
                reported()
            );
        }

        async fn close(self) {
            self.player.close().await;
        }
    }

    fn reported() -> Vec<String> {
        lock(&REPORTED).clone()
    }

    async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT;
        while !check() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}; libmpv reported: {:#?}",
                reported()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn signed_in(stand_in: &StandIn, app: &App, gate: Gate) -> ServerRef {
        stand_in.set_gate(gate);
        let setup = app.add_server();
        setup.probe(stand_in.address()).await;
        let submitted = match gate {
            Gate::Basic => setup.submit_password(GATE_USER, GATE_PASSWORD, false).await,
            Gate::Portal => {
                setup
                    .submit_cookies(
                        &[CookieInput {
                            name: GATE_COOKIE.into(),
                            value: GATE_COOKIE_VALUE.into(),
                            domain: "127.0.0.1".into(),
                            path: "/".into(),
                            secure: false,
                            http_only: true,
                            expires: None,
                        }],
                        false,
                    )
                    .await
            }
            Gate::Header => {
                setup
                    .submit_headers(
                        &[HeaderInput {
                            name: GATE_HEADER.into(),
                            value: GATE_HEADER_VALUE.into(),
                        }],
                        false,
                    )
                    .await
            }
            Gate::Off => Ok(()),
        };
        submitted.expect("the gateway's credentials");
        let found = match setup.state().borrow().clone() {
            AddServerState::Ready { server } => server,
            state => panic!("{state:?}"),
        };
        let flow = app.sign_in(found).unwrap();
        flow.with_password(USER_NAME, PASSWORD).await;
        let state = flow.state().borrow().clone();
        assert!(matches!(state, SignInState::SignedIn { .. }), "{state:?}");
        eventually("an address", || {
            app.capabilities().borrow().servers[0].address.is_some()
        })
        .await;
        app.capabilities().borrow().servers[0].server.clone()
    }

    /// libmpv's requests to the relay carry no cookie, no gateway header, no `Authorization`, no `ApiKey`, and the session token
    /// nowhere (no `X-Emby-Authorization`, `X-Emby-Token` or `X-MediaBrowser-Token` either).
    fn assert_bare(seen: &[TapSeen], stand_in: &StandIn) {
        assert!(!seen.is_empty(), "libmpv sent nothing to the relay");
        let token = stand_in.live_token().expect("a session token");
        for request in seen {
            let target = request.target.to_ascii_lowercase();
            assert!(
                request.header("cookie").is_none()
                    && request.header("authorization").is_none()
                    && request.header(GATE_HEADER).is_none()
                    && !target.contains("apikey")
                    && !target.contains("api_key"),
                "a credential in libmpv's request: {request:?}"
            );
            assert!(
                !request.target.contains(&token)
                    && request
                        .headers
                        .iter()
                        .all(|(_, value)| !value.contains(&token)),
                "the session token in libmpv's request: {request:?}"
            );
            assert!(
                matches!(request.method.as_str(), "GET" | "HEAD"),
                "{request:?}"
            );
        }
    }

    /// The stand-in got the gateway's credential and the session token on every stream request, from the core: in headers, never
    /// as `ApiKey`, with the core's `User-Agent`.
    fn assert_upstream(gate: Gate, streams: &[StreamSeen]) {
        assert!(
            !streams.is_empty(),
            "no stream request reached the stand-in"
        );
        for stream in streams {
            let gated = match gate {
                Gate::Portal => stream
                    .cookie
                    .split("; ")
                    .any(|pair| pair == format!("{GATE_COOKIE}={GATE_COOKIE_VALUE}")),
                Gate::Basic => stream.authorization == format!("Basic {GATE_BASIC}"),
                Gate::Header => stream.gate_header == GATE_HEADER_VALUE,
                Gate::Off => true,
            };
            let token = stream.emby_authorization.contains("Token=\"")
                || stream.authorization.contains("Token=\"");
            let target = stream.target.to_ascii_lowercase();
            assert!(
                gated && token && !target.contains("apikey") && !target.contains("api_key"),
                "upstream request without the core's credentials: {stream:?}"
            );
            assert_eq!(
                stream.user_agent,
                concat!("Current/", env!("CARGO_PKG_VERSION")),
                "{stream:?}"
            );
        }
    }

    pub(super) async fn direct_play(gate: Gate) {
        let run = Run::start(gate).await;
        run.stand_in
            .set_stream(&direct_path(), "audio/wav", wav(20));
        run.play(DIRECT_PLAY_ITEM).await;
        run.playing_past(1).await;
        let seen = run.tap.seen();
        assert!(!seen.is_empty(), "libmpv sent nothing to the relay");
        let user_agent = seen[0].header("user-agent").unwrap_or_default().to_owned();
        run.player.stop().await.unwrap();
        assert_bare(&seen, &run.stand_in);
        assert!(
            seen.iter().all(|request| request
                .target
                .contains(&format!("/Videos/{DIRECT_PLAY_ITEM}/stream"))),
            "{seen:?}"
        );
        assert_upstream(gate, &run.stand_in.streams());
        run.observed(
            &format!("direct play through {gate:?}"),
            &format!("played; libmpv's User-Agent to the relay: {user_agent:?}"),
        );
        run.close().await;
    }

    pub(super) async fn hls(gate: Gate) {
        let run = Run::start(gate).await;
        serve_hls(&run.stand_in, 4);
        run.play(TRANSCODE_ITEM).await;
        // Into the second segment.
        run.playing_past(3).await;
        run.player.stop().await.unwrap();
        let seen = run.tap.seen();
        assert_bare(&seen, &run.stand_in);
        for part in ["/master.m3u8", "/main.m3u8", "/hls1/main/1.wav"] {
            assert!(
                seen.iter().any(|request| request.target.contains(part)),
                "no request for {part}: {seen:?}"
            );
        }
        let streams = run.stand_in.streams();
        assert!(streams.len() >= 2, "{streams:?}");
        assert_upstream(gate, &streams);
        assert!(
            run.stand_in
                .playlists()
                .iter()
                .all(|playlist| !playlist.target.to_ascii_lowercase().contains("apikey")),
            "{:?}",
            run.stand_in.playlists()
        );
        run.observed(&format!("HLS through {gate:?}"), "played");
        run.close().await;
    }

    /// A listener that only counts the connections it gets, on 127.0.0.1 and, where the machine has IPv6 loopback and the port is
    /// free there, on `::1` too (where `localhost` may resolve first).
    async fn counting_listener() -> (u16, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let count = Arc::new(AtomicUsize::new(0));
        let listeners =
            std::iter::once(listener).chain(TcpListener::bind(("::1", port)).await.ok());
        for listener in listeners {
            let counting = count.clone();
            tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    counting.fetch_add(1, Ordering::SeqCst);
                    drop(socket);
                }
            });
        }
        (port, count)
    }

    pub(super) async fn hostile_playlist() {
        let run = Run::start(Gate::Header).await;
        let (port, connections) = counting_listener().await;
        serve_hls(&run.stand_in, 4);
        // Another port of this machine, and another host name for it.
        for elsewhere in [
            format!("http://127.0.0.1:{port}/videos/{TRANSCODE_ITEM_PATH_ID}/hls1/main/0.wav"),
            format!("http://localhost:{port}/videos/{TRANSCODE_ITEM_PATH_ID}/hls1/main/0.wav"),
        ] {
            let playlist = media_playlist(4).replacen(
                &format!("hls1/main/0.wav?PlaySessionId={PLAY_SESSION}"),
                &elsewhere,
                1,
            );
            run.stand_in.set_playlist(&media_playlist_path(), &playlist);
            lock(&REPORTED).clear();
            run.play(TRANSCODE_ITEM).await;
            let state = run.finished().await;
            assert_eq!(
                state,
                PlayerState::Failed {
                    problem: PlaybackProblem::PlaylistElsewhere
                }
            );
            run.observed(
                &format!(
                    "a playlist naming {}",
                    elsewhere
                        .split('/')
                        .nth(2)
                        .unwrap_or_default()
                        .split(':')
                        .next()
                        .unwrap_or_default()
                ),
                &format!("{state:?}"),
            );
        }
        // Whatever libmpv did meanwhile, nothing reached the other port.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(connections.load(Ordering::SeqCst), 0);
        assert_bare(&run.tap.seen(), &run.stand_in);
        run.close().await;
    }

    /// The `<first>` of a `bytes=<first>-...` range, 0 without one.
    fn range_start(range: &str) -> usize {
        range
            .strip_prefix("bytes=")
            .and_then(|rest| rest.split('-').next())
            .and_then(|first| first.parse().ok())
            .unwrap_or(0)
    }

    pub(super) async fn seek_opens_a_new_range() {
        let run = Run::start(Gate::Portal).await;
        // Twice the tone's rate: libmpv's read-ahead stays far behind the seek's target.
        run.stand_in.set_stream_pace(2 * BYTES_PER_SECOND);
        run.stand_in
            .set_stream(&direct_path(), "audio/wav", wav(120));
        run.play(DIRECT_PLAY_ITEM).await;
        run.playing_past(1).await;
        let before = run.stand_in.streams().len();
        run.player.seek_to(Duration::from_secs(90)).await.unwrap();
        run.playing_past(91).await;
        let streams = run.stand_in.streams();
        let target = WAV_HEADER + 90 * BYTES_PER_SECOND;
        let later = &streams[before..];
        assert!(
            later.iter().any(|stream| {
                let first = range_start(&stream.range);
                first > WAV_HEADER + 10 * BYTES_PER_SECOND && first <= target
            }),
            "no new range at the seek: {streams:?}"
        );
        assert!(
            run.tap.seen().iter().any(|request| request
                .header("range")
                .is_some_and(|range| range_start(range) > WAV_HEADER + 10 * BYTES_PER_SECOND)),
            "libmpv sent no new range: {:?}",
            run.tap.seen()
        );
        assert_upstream(Gate::Portal, &streams);
        // The connection before the seek is gone: one paced answer left at most.
        eventually("the old connection closed", || {
            run.stand_in.open_streams() <= 1
        })
        .await;
        run.observed(
            "a seek to 90 s",
            &format!(
                "played on; ranges upstream: {:?}",
                streams
                    .iter()
                    .map(|stream| stream.range.clone())
                    .collect::<Vec<_>>()
            ),
        );
        run.close().await;
    }

    pub(super) async fn stopping_closes_every_upstream_connection() {
        let run = Run::start(Gate::Basic).await;
        run.stand_in.set_stream_pace(2 * BYTES_PER_SECOND);
        run.stand_in
            .set_stream(&direct_path(), "audio/wav", wav(60));
        serve_hls(&run.stand_in, 20);
        for id in [DIRECT_PLAY_ITEM, TRANSCODE_ITEM] {
            run.play(id).await;
            run.playing_past(1).await;
            eventually("an upstream answer in progress", || {
                run.stand_in.open_streams() >= 1
            })
            .await;
            run.player.stop().await.unwrap();
            eventually("every upstream connection closed", || {
                run.stand_in.open_streams() == 0
            })
            .await;
        }
        run.observed(
            "stop during paced direct play and HLS",
            "stopped; no upstream connection left",
        );
        run.close().await;
    }

    /// The 401 rule's answer for a gateway that let nothing through any more.
    pub(super) async fn refused_mid_play_hls(gate: Gate) {
        let run = Run::start(gate).await;
        // Each segment takes a second to arrive: later ones are asked for while it plays.
        run.stand_in.set_stream_pace(2 * BYTES_PER_SECOND);
        serve_hls(&run.stand_in, 30);
        run.play(TRANSCODE_ITEM).await;
        run.playing_past(1).await;
        run.stand_in.expire_gate();
        let state = run.finished().await;
        run.observed(
            &format!("{gate:?} gateway expired mid-play (HLS)"),
            &format!("{state:?}"),
        );
        assert_eq!(
            state,
            PlayerState::Failed {
                problem: PlaybackProblem::GatewaySignInNeeded
            }
        );
        run.close().await;
    }

    pub(super) async fn refused_seek() {
        let run = Run::start(Gate::Header).await;
        run.stand_in.set_stream_pace(2 * BYTES_PER_SECOND);
        run.stand_in
            .set_stream(&direct_path(), "audio/wav", wav(120));
        run.play(DIRECT_PLAY_ITEM).await;
        run.playing_past(1).await;
        run.stand_in.expire_gate();
        run.player.seek_to(Duration::from_secs(90)).await.unwrap();
        let state = run.finished().await;
        run.observed(
            "header gateway expired, then a seek (direct play)",
            &format!("{state:?}"),
        );
        assert_eq!(
            state,
            PlayerState::Failed {
                problem: PlaybackProblem::GatewaySignInNeeded
            }
        );
        run.close().await;
    }

    /// Segment 2 of 6 answers `status`: through the relay playback fails at once; without a gateway (no relay) libmpv gets the
    /// status itself, and what it then does is recorded.
    pub(super) async fn failing_segment(status: u16) {
        let problem = if status == 404 {
            PlaybackProblem::NotFound
        } else {
            PlaybackProblem::Server { status }
        };
        for gate in [Gate::Header, Gate::Off] {
            let run = Run::start(gate).await;
            serve_hls(&run.stand_in, 6);
            if gate == Gate::Off {
                // Unrelayed, libmpv sends the token itself, as `ApiKey` in every URL, as Jellyfin's playlists name it.
                let token = run.stand_in.live_token().expect("a token");
                run.stand_in.set_playlist(
                    &master_playlist_path(),
                    &format!(
                        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=128000\nmain.m3u8?ApiKey={token}\n"
                    ),
                );
                run.stand_in.set_playlist(
                    &media_playlist_path(),
                    &media_playlist(6).replace(
                        &format!("PlaySessionId={PLAY_SESSION}"),
                        &format!("PlaySessionId={PLAY_SESSION}&ApiKey={token}"),
                    ),
                );
            }
            run.stand_in.fail_stream(&segment_path(2), status);
            run.play(TRANSCODE_ITEM).await;
            if gate == Gate::Off {
                // libmpv on its own: it either fails, ends, or rides the segment out and plays past it.
                let state = run
                    .until("an outcome", |status| {
                        matches!(
                            status.state,
                            PlayerState::Failed { .. } | PlayerState::Ended
                        ) || status.position
                            >= Duration::from_secs((3 * SEGMENT_SECONDS) as u64 + 1)
                    })
                    .await;
                run.observed(
                    &format!("a {status} on segment 2 of 6, unrelayed"),
                    &format!("{:?} at {:?}", state.state, state.position),
                );
                assert!(
                    run.tap.seen().is_empty(),
                    "an unrelayed stream went through the relay"
                );
            } else {
                let state = run.finished().await;
                run.observed(
                    &format!("a {status} on segment 2 of 6, relayed"),
                    &format!("{state:?}"),
                );
                assert_eq!(
                    state,
                    PlayerState::Failed {
                        problem: problem.clone()
                    }
                );
            }
            run.close().await;
        }
    }

    pub(super) async fn cut_mid_body() {
        let run = Run::start(Gate::Portal).await;
        // Paced, so the cut comes while it plays.
        run.stand_in.set_stream_pace(2 * BYTES_PER_SECOND);
        run.stand_in
            .set_stream(&direct_path(), "audio/wav", wav(40));
        // After 20 s of the 40, about 10 s after the start: libmpv plays by then (after about 5 s here).
        run.stand_in
            .cut_stream(&direct_path(), WAV_HEADER + 20 * BYTES_PER_SECOND);
        run.play(DIRECT_PLAY_ITEM).await;
        run.playing_past(1).await;
        let state = run
            .until("a failure, an end, or play past the cut", |status| {
                matches!(
                    status.state,
                    PlayerState::Failed { .. } | PlayerState::Ended
                ) || status.position >= Duration::from_secs(23)
            })
            .await;
        run.observed(
            "the upstream connection cut after 20 s of 40, while playing (direct play)",
            &format!(
                "{:?} at {:?}; ranges upstream: {:?}",
                state.state,
                state.position,
                run.stand_in
                    .streams()
                    .iter()
                    .map(|stream| stream.range.clone())
                    .collect::<Vec<_>>()
            ),
        );
        // Never the end of the file, never an engine error: either the connection's failure, or playing on.
        assert!(
            state.state
                == PlayerState::Failed {
                    problem: PlaybackProblem::Network
                }
                || (state.state == PlayerState::Playing
                    && state.position >= Duration::from_secs(23)),
            "{state:?}"
        );
        run.close().await;
    }

    pub(super) async fn re_plans() {
        let run = Run::start(Gate::Portal).await;
        // Paced: each re-plan drops a route whose connection is mid-body.
        run.stand_in.set_stream_pace(2 * BYTES_PER_SECOND);
        run.stand_in
            .set_stream(&direct_path(), "audio/wav", wav(120));
        run.play(DIRECT_PLAY_ITEM).await;
        run.playing_past(1).await;
        let mut at = 1;
        for (round, quality) in [
            Quality::Bitrate(20_000_000),
            Quality::Maximum,
            Quality::Bitrate(30_000_000),
            Quality::Maximum,
            Quality::Bitrate(20_000_000),
        ]
        .into_iter()
        .enumerate()
        {
            run.player.set_quality(quality).await.unwrap();
            at += 1;
            let state = run
                .until(&format!("round {round}: playing past {at} s"), |status| {
                    matches!(
                        status.state,
                        PlayerState::Failed { .. } | PlayerState::Ended
                    ) || (status.state == PlayerState::Playing
                        && status.position >= Duration::from_secs(at))
                })
                .await;
            assert_eq!(
                state.state,
                PlayerState::Playing,
                "round {round}: {state:?}"
            );
        }
        let streams = run.stand_in.streams();
        run.observed(
            "five re-plans mid-play (direct play)",
            &format!("played on; {} stream requests upstream", streams.len()),
        );
        assert!(streams.len() >= 6, "{streams:?}");
        run.close().await;
    }

    pub(super) async fn plain_playlists() {
        let run = Run::start(Gate::Header).await;
        let (port, connections) = counting_listener().await;
        let entry = format!("http://127.0.0.1:{port}/elsewhere.wav");
        // The main file of an HLS plan at a path the relay does not take for a playlist by its name or type, with bodies libmpv's own
        // playlist and EDL readers know (mpv v0.41.0 `demux/demux_playlist.c`, `demux/demux_edl.c`, read 2026-10-26) and a DASH
        // manifest FFmpeg's `dash` demuxer would read; none starts with `#EXTM3U`. The relay refuses each at the route's HLS entry.
        let path = format!("/videos/{TRANSCODE_ITEM_PATH_ID}/live");
        run.stand_in.set_transcoding_path(&path);
        for (name, body) in [
            ("plain URL list", format!("{entry}\n")),
            (
                "pls",
                format!("[playlist]\nFile1={entry}\nNumberOfEntries=1\nVersion=2\n"),
            ),
            ("ref ini", format!("[Reference]\nRef1={entry}\n")),
            ("url", format!("[InternetShortcut]\nURL={entry}\n")),
            ("EDL", format!("# mpv EDL v0\n{entry}\n")),
            (
                "DASH MPD",
                format!(
                    "<?xml version=\"1.0\"?>\n<MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\" type=\"static\" \
                     mediaPresentationDuration=\"PT10S\" minBufferTime=\"PT1S\" \
                     profiles=\"urn:mpeg:dash:profile:isoff-on-demand:2011\"><Period><AdaptationSet mimeType=\"audio/wav\">\
                     <Representation id=\"a\" bandwidth=\"128000\"><BaseURL>{entry}</BaseURL></Representation>\
                     </AdaptationSet></Period></MPD>\n"
                ),
            ),
        ] {
            run.stand_in
                .set_stream(&path, "text/plain", body.into_bytes());
            lock(&REPORTED).clear();
            run.play(TRANSCODE_ITEM).await;
            let state = run.finished().await;
            // libmpv opens the next playlist entry on its own: give it time to.
            tokio::time::sleep(Duration::from_secs(1)).await;
            let opened = connections.load(Ordering::SeqCst);
            run.observed(
                &format!("a {name} body as the main file of a relayed HLS route"),
                &format!("{state:?}; connections to the entry's port: {opened}"),
            );
            assert_eq!(opened, 0, "libmpv opened the {name} entry directly");
            // The relay's refusal (`PlaylistUnreadable`), as the controller shows it.
            assert_eq!(
                state,
                PlayerState::Failed {
                    problem: PlaybackProblem::PlaylistElsewhere
                },
                "{name}"
            );
        }
        run.close().await;
    }
}
