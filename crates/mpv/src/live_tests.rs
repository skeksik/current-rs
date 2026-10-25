//! Tests against a real libmpv (ADR 0019 task 3's done-when): a headless engine (`vo=null`, `ao=null`), its capabilities against
//! `build/mpv-windows/check.py`, a generated file played to the end, redirects refused on a playlist and on an HLS segment, no gateway
//! header after a refused redirect, the test token never in a log line, and TLS verified against a self-signed stand-in.
//!
//! They need the `libmpv` feature (the "mpv (Windows)" CI job links the D6 build; on Linux the system libmpv works too) and are ignored
//! without it. The TLS test also needs `openssl` on `PATH` (or in `OPENSSL`) to make its throwaway certificate at run time.

/// Declares a test that runs [`live`]'s function of the same name, ignored without libmpv.
macro_rules! libmpv_test {
    ($name:ident) => {
        #[test]
        #[cfg_attr(
            not(feature = "libmpv"),
            ignore = "needs libmpv: run with --features libmpv (the \"mpv (Windows)\" CI job links the D6 build)"
        )]
        fn $name() {
            #[cfg(feature = "libmpv")]
            live::$name();
            #[cfg(not(feature = "libmpv"))]
            panic!("built without the libmpv feature");
        }
    };
}

libmpv_test!(headless_engine_reports_the_capabilities_check_py_requires);
libmpv_test!(plays_a_generated_file_to_the_end);
libmpv_test!(refuses_a_redirected_stream_and_sends_nothing_to_the_target);
libmpv_test!(refuses_a_redirected_hls_segment_and_sends_nothing_to_the_target);
libmpv_test!(log_lines_never_show_the_token);
libmpv_test!(verifies_tls_against_a_self_signed_stand_in);

#[cfg(feature = "libmpv")]
mod live {
    use std::collections::BTreeSet;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{Receiver, channel};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use crate::{
        EndReason, Engine, EngineConfig, EngineEvent, FileRequest, LogLevel, LogLine, Output,
        PropertyChange, REDACTED, StreamCookie,
    };

    /// Fake values: obviously not real credentials.
    const TOKEN: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f0";
    const GATE_HEADER: &str = "X-Stand-In-Gate";
    const GATE_HEADER_VALUE: &str = "not-a-real-gate-header";
    const GATE_COOKIE_VALUE: &str = "not-a-real-gate-cookie";
    const WAIT: Duration = Duration::from_secs(30);

    struct Running {
        engine: Engine,
        events: Receiver<EngineEvent>,
        logs: Vec<LogLine>,
    }

    impl Running {
        fn start() -> Running {
            let (tx, events) = channel();
            let config = EngineConfig {
                output: Output::Headless,
                user_agent: crate::user_agent(),
                // `v` logs more URLs than `info`, so redaction is tested on more lines.
                log_level: LogLevel::Verbose,
            };
            let engine = Engine::start(
                &config,
                Box::new(move |e| {
                    let _ = tx.send(e);
                }),
            )
            .expect("engine starts");
            Running {
                engine,
                events,
                logs: Vec::new(),
            }
        }

        /// Collects events until `done` returns true for one, and returns all events seen (log lines are kept in `logs` as well).
        fn wait_for(
            &mut self,
            what: &str,
            mut done: impl FnMut(&EngineEvent) -> bool,
        ) -> Vec<EngineEvent> {
            let deadline = Instant::now() + WAIT;
            let mut seen = Vec::new();
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                let Ok(event) = self.events.recv_timeout(left) else {
                    panic!("timed out waiting for {what}; events: {seen:#?}");
                };
                if let EngineEvent::Log(line) = &event {
                    self.logs.push(line.clone());
                }
                let finished = done(&event);
                seen.push(event);
                if finished {
                    return seen;
                }
            }
        }

        /// Shuts the engine down and drains the remaining events, so every log line is checked.
        fn finish(self) -> Vec<LogLine> {
            let Running {
                engine,
                events,
                mut logs,
            } = self;
            engine.shutdown();
            // The event thread has ended and dropped its sender, so this ends after the last event.
            for e in events.iter() {
                if let EngineEvent::Log(line) = e {
                    logs.push(line);
                }
            }
            logs
        }
    }

    fn assert_no_secret(logs: &[LogLine]) {
        assert!(!logs.is_empty(), "no log lines were captured");
        for line in logs {
            for secret in [TOKEN, GATE_HEADER_VALUE, GATE_COOKIE_VALUE] {
                assert!(
                    !line.text.contains(secret) && !line.prefix.contains(secret),
                    "secret in a log line: {line:?}"
                );
            }
        }
    }

    /// The file ended, or (with `keep-open`) played to its end, which a refused stream must not do.
    fn ended(e: &EngineEvent) -> bool {
        end_file(e).is_some()
            || matches!(e, EngineEvent::Property(PropertyChange::EofReached(true)))
    }

    fn end_file(e: &EngineEvent) -> Option<EndReason> {
        match e {
            EngineEvent::EndFile { reason, .. } => Some(*reason),
            _ => None,
        }
    }

    /// One second of a 440 Hz tone, 8 kHz mono 16-bit PCM in a WAV container: FFmpeg's built-in `wav` demuxer and `pcm_s16le`
    /// decoder, so any build plays it.
    fn wav() -> Vec<u8> {
        let rate: u32 = 8000;
        let samples: Vec<i16> = (0..rate)
            .map(|i| {
                ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 8000.0) as i16
            })
            .collect();
        let data_len = (samples.len() * 2) as u32;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    // A minimal HTTP/1.1 stand-in on 127.0.0.1 that records every request it receives.

    #[derive(Debug, Clone)]
    struct Seen {
        path: String,
        /// Header names lowercased.
        headers: Vec<(String, String)>,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<&str> {
            let name = name.to_ascii_lowercase();
            self.headers
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.as_str())
        }
    }

    enum Reply {
        Body(&'static str, Vec<u8>),
        Redirect(String),
        NotFound,
    }

    type Handler = Arc<dyn Fn(&Seen) -> Reply + Send + Sync>;

    struct StandIn {
        port: u16,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl StandIn {
        fn start(handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> StandIn {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().expect("address").port();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let handler: Handler = Arc::new(handler);
            let seen2 = Arc::clone(&seen);
            // The thread lives until the test binary exits.
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (seen, handler) = (Arc::clone(&seen2), Arc::clone(&handler));
                    std::thread::spawn(move || serve(stream, &seen, &handler));
                }
            });
            StandIn { port, seen }
        }

        fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{path}", self.port)
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    /// Reads one request head; `None` at the end of the stream or on an error (a refused TLS handshake ends here).
    fn read_head(reader: &mut impl BufRead) -> Option<Seen> {
        let mut line = String::new();
        reader.read_line(&mut line).ok().filter(|n| *n > 0)?;
        let path = line.split_whitespace().nth(1)?.to_owned();
        let mut headers = Vec::new();
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).ok().filter(|n| *n > 0)?;
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((n, v)) = h.split_once(':') {
                headers.push((n.trim().to_ascii_lowercase(), v.trim().to_owned()));
            }
        }
        Some(Seen { path, headers })
    }

    fn respond(out: &mut impl Write, reply: Reply) {
        let head = match &reply {
            Reply::Body(kind, body) => format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ),
            Reply::Redirect(to) => {
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
            }
            Reply::NotFound => {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_owned()
            }
        };
        let _ = out.write_all(head.as_bytes());
        if let Reply::Body(_, body) = reply {
            let _ = out.write_all(&body);
        }
        let _ = out.flush();
    }

    fn serve(stream: TcpStream, seen: &Mutex<Vec<Seen>>, handler: &Handler) {
        let mut writer = match stream.try_clone() {
            Ok(w) => w,
            Err(_) => return,
        };
        let mut reader = BufReader::new(stream);
        if let Some(request) = read_head(&mut reader) {
            seen.lock().unwrap().push(request.clone());
            respond(&mut writer, handler(&request));
        }
    }

    fn gated(url: String) -> FileRequest {
        let mut file = FileRequest::new(url);
        file.secrets.push(TOKEN.into());
        file.headers
            .push((GATE_HEADER.into(), GATE_HEADER_VALUE.into()));
        file.cookies.push(StreamCookie {
            name: "stand_in_gate".into(),
            value: GATE_COOKIE_VALUE.into(),
            path: "/".into(),
            secure: false,
        });
        file
    }

    /// `check.py`'s set literal `NAME = {"a", "b", ...}`, read from the build script so the list is kept in one place.
    fn check_py_set(source: &str, name: &str) -> BTreeSet<String> {
        let start = source
            .find(&format!("{name} = {{"))
            .unwrap_or_else(|| panic!("{name} not in check.py"));
        let body = &source[start..];
        let body = &body[body.find('{').unwrap() + 1..body.find('}').unwrap()];
        // The quoted names: every second piece between quotes (names like "matroska,webm" contain commas).
        body.split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    pub(super) fn headless_engine_reports_the_capabilities_check_py_requires() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../build/mpv-windows/check.py"
        );
        let source = std::fs::read_to_string(path).expect("check.py");
        let decoders = check_py_set(&source, "REQUIRED_DECODERS");
        let demuxers = check_py_set(&source, "REQUIRED_DEMUXERS");
        assert!(
            decoders.contains("hevc") && demuxers.contains("matroska,webm"),
            "check.py lists not parsed"
        );

        let running = Running::start();
        let caps = running.engine.capabilities().clone();
        let missing: Vec<_> = decoders.iter().filter(|d| !caps.has_decoder(d)).collect();
        assert!(missing.is_empty(), "decoders missing: {missing:?}");
        // Our build also lists the subtitle text formats as demuxers; matroska and mp4 under FFmpeg's combined names.
        let missing: Vec<_> = demuxers.iter().filter(|d| !caps.has_demuxer(d)).collect();
        assert!(missing.is_empty(), "demuxers missing: {missing:?}");
        running.finish();
    }

    pub(super) fn plays_a_generated_file_to_the_end() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("tone.wav");
        std::fs::write(&file, wav()).expect("write wav");

        let mut running = Running::start();
        running
            .engine
            .load(&FileRequest::new(file.to_string_lossy()))
            .expect("load");
        let mut positions = Vec::new();
        let seen = running.wait_for("eof-reached", |e| {
            if let EngineEvent::Property(PropertyChange::TimePos(Some(t))) = e {
                positions.push(*t);
            }
            matches!(e, EngineEvent::Property(PropertyChange::EofReached(true)))
        });
        assert!(
            seen.iter().any(|e| matches!(e, EngineEvent::FileLoaded)),
            "{seen:#?}"
        );
        assert!(
            seen.iter()
                .any(|e| matches!(e, EngineEvent::PlaybackRestart)),
            "{seen:#?}"
        );
        assert!(
            seen.iter().any(|e| matches!(e, EngineEvent::Property(PropertyChange::Duration(Some(d))) if (*d - 1.0).abs() < 0.1)),
            "{seen:#?}"
        );
        assert!(
            positions.iter().any(|t| *t > 0.5),
            "positions: {positions:?}"
        );
        assert!(
            positions.windows(2).all(|w| w[1] >= w[0]),
            "positions go back: {positions:?}"
        );

        // `keep-open=yes` holds the file at its end; stopping ends it.
        running.engine.stop().expect("stop");
        running.wait_for("end-file", |e| end_file(e) == Some(EndReason::Stop));
        running.finish();
    }

    pub(super) fn refuses_a_redirected_stream_and_sends_nothing_to_the_target() {
        let target = StandIn::start(|_| Reply::Body("audio/wav", wav()));
        let to = target.url("/elsewhere.wav");
        let origin = StandIn::start(move |_| Reply::Redirect(to.clone()));

        let mut running = Running::start();
        running
            .engine
            .load(&gated(
                origin.url(&format!("/Videos/1/stream?static=true&ApiKey={TOKEN}")),
            ))
            .expect("load");
        let seen = running.wait_for("end-file", ended);
        let asked = origin.seen();
        assert!(!asked.is_empty(), "the origin got no request");
        assert_eq!(
            asked[0].header(GATE_HEADER),
            Some(GATE_HEADER_VALUE),
            "the gateway header was not sent: {asked:?}"
        );
        assert!(
            asked[0]
                .header("cookie")
                .is_some_and(|c| c.contains(GATE_COOKIE_VALUE)),
            "the gateway cookie was not sent: {asked:?}"
        );
        assert!(
            target.seen().is_empty(),
            "the redirect was followed: {:?}",
            target.seen()
        );
        assert_eq!(
            seen.iter().find_map(end_file),
            Some(EndReason::Error),
            "{seen:#?}"
        );
        assert_no_secret(&running.finish());
    }

    pub(super) fn refuses_a_redirected_hls_segment_and_sends_nothing_to_the_target() {
        let target = StandIn::start(|_| Reply::Body("video/mp2t", vec![0x47; 188 * 8]));
        let to = target.url("/elsewhere.ts");
        let origin = StandIn::start(move |req| {
            if req.path.starts_with("/master.m3u8") {
                let playlist = format!(
                    "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n\
                     #EXTINF:2.0,\nsegment0.ts?ApiKey={TOKEN}\n#EXTINF:2.0,\nsegment1.ts?ApiKey={TOKEN}\n#EXT-X-ENDLIST\n"
                );
                Reply::Body("application/vnd.apple.mpegurl", playlist.into_bytes())
            } else if req.path.starts_with("/segment") {
                Reply::Redirect(to.clone())
            } else {
                Reply::NotFound
            }
        });

        let mut running = Running::start();
        running
            .engine
            .load(&gated(origin.url(&format!("/master.m3u8?ApiKey={TOKEN}"))))
            .expect("load");
        let seen = running.wait_for("end-file", ended);

        let asked = origin.seen();
        let segments: Vec<_> = asked
            .iter()
            .filter(|s| s.path.starts_with("/segment"))
            .collect();
        assert!(
            !segments.is_empty(),
            "no segment was requested: {asked:?}; events: {seen:#?}"
        );
        assert!(
            segments
                .iter()
                .all(|s| s.header(GATE_HEADER) == Some(GATE_HEADER_VALUE)),
            "a segment request lacks the gateway header: {segments:?}"
        );
        assert!(
            segments.iter().all(|s| s
                .header("cookie")
                .is_some_and(|c| c.contains(GATE_COOKIE_VALUE))),
            "a segment request lacks the gateway cookie: {segments:?}"
        );
        assert!(
            target.seen().is_empty(),
            "a segment redirect was followed: {:?}",
            target.seen()
        );
        assert_no_secret(&running.finish());
    }

    pub(super) fn log_lines_never_show_the_token() {
        let origin = StandIn::start(|_| Reply::NotFound);
        let mut running = Running::start();
        running
            .engine
            .load(&gated(
                origin.url(&format!("/Videos/1/stream?static=true&ApiKey={TOKEN}")),
            ))
            .expect("load");
        running.wait_for("end-file", |e| end_file(e).is_some());
        let logs = running.finish();
        assert_no_secret(&logs);
        // mpv logs the URL it plays ("Playing: %s", `player/loadfile.c`), so a redacted line must be there.
        assert!(
            logs.iter().any(|l| l.text.contains(REDACTED)),
            "no redacted line: {logs:#?}"
        );
    }

    /// Makes a throwaway self-signed certificate for 127.0.0.1 with `openssl` (DER certificate, PKCS#8 DER key).
    fn self_signed(dir: &std::path::Path) -> (Vec<u8>, Vec<u8>) {
        let openssl = std::env::var("OPENSSL").unwrap_or_else(|_| "openssl".into());
        let run = |args: &[&str]| {
            let status = std::process::Command::new(&openssl)
                .args(args)
                .current_dir(dir)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap_or_else(|e| {
                    panic!("this test needs openssl on PATH or in OPENSSL ({openssl}: {e})")
                });
            assert!(status.success(), "openssl {args:?} failed");
        };
        run(&[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "key.pem",
            "-out",
            "cert.der",
            "-outform",
            "DER",
            "-days",
            "2",
            "-subj",
            "/CN=127.0.0.1",
            "-addext",
            "subjectAltName=IP:127.0.0.1",
        ]);
        run(&[
            "pkcs8", "-topk8", "-nocrypt", "-in", "key.pem", "-outform", "DER", "-out", "key.der",
        ]);
        (
            std::fs::read(dir.join("cert.der")).unwrap(),
            std::fs::read(dir.join("key.der")).unwrap(),
        )
    }

    /// An HTTPS stand-in with the given certificate; counts the requests that got past the handshake.
    fn tls_stand_in(cert: Vec<u8>, key: Vec<u8>) -> (u16, Arc<Mutex<Vec<Seen>>>) {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("protocol versions")
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
            )
            .expect("certificate");
        let config = Arc::new(config);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        std::thread::spawn(move || {
            for tcp in listener.incoming().flatten() {
                let (config, seen) = (Arc::clone(&config), Arc::clone(&seen2));
                std::thread::spawn(move || {
                    let Ok(conn) = rustls::ServerConnection::new(config) else {
                        return;
                    };
                    let mut tls = rustls::StreamOwned::new(conn, tcp);
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    // Reading drives the handshake; a client that refuses the certificate ends it here.
                    while !head.ends_with(b"\r\n\r\n") {
                        match tls.read(&mut byte) {
                            Ok(1) => head.push(byte[0]),
                            _ => return,
                        }
                    }
                    let Some(request) = read_head(&mut BufReader::new(&head[..])) else {
                        return;
                    };
                    seen.lock().unwrap().push(request);
                    respond(&mut tls, Reply::Body("audio/wav", wav()));
                    tls.conn.send_close_notify();
                    let _ = tls.flush();
                });
            }
        });
        (port, seen)
    }

    pub(super) fn verifies_tls_against_a_self_signed_stand_in() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (cert, key) = self_signed(dir.path());
        let (port, seen) = tls_stand_in(cert, key);
        let url = format!("https://127.0.0.1:{port}/tone.wav");

        let mut running = Running::start();
        running
            .engine
            .load(&FileRequest::new(url.clone()))
            .expect("load");
        let events = running.wait_for("end-file", |e| end_file(e).is_some());
        assert_eq!(
            events.iter().find_map(end_file),
            Some(EndReason::Error),
            "{events:#?}"
        );
        assert!(
            seen.lock().unwrap().is_empty(),
            "a request passed an unverified certificate"
        );

        // Control: the same stand-in opens with verification off, so the failure above was the certificate.
        running
            .engine
            .set_for_test("tls-verify", "no")
            .expect("tls-verify=no");
        running.engine.load(&FileRequest::new(url)).expect("load");
        let events = running.wait_for("file-loaded", |e| {
            matches!(e, EngineEvent::FileLoaded) || end_file(e).is_some()
        });
        assert!(
            matches!(events.last(), Some(EngineEvent::FileLoaded)),
            "{events:#?}"
        );
        assert!(
            !seen.lock().unwrap().is_empty(),
            "the control request did not arrive"
        );
        running.finish();
    }
}
