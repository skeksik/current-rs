//! What the engine sets: the options at start (ADR 0019 "Options at start", ADR 0007's route), the display facts mapping (ADR 0007)
//! and the per-file options of `loadfile` (ADR 0019 "Gateways, tokens and the player").

use crate::error::EngineError;
use crate::node::Node;

/// Where video goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// No video or audio output (`vo=null`, `ao=null`): tests, and probing a build.
    Headless,
    /// Windows: D3D11 composition mode, whose swap chain the UI puts in a `SwapChainPanel` (ADR 0007). The size must be known before
    /// the engine starts.
    Composition {
        size: CompositionSize,
        display: DisplayFacts,
    },
}

/// The panel's size in physical pixels (`ActualSize` x `CompositionScale`, ADR 0007).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompositionSize {
    pub width: u32,
    pub height: u32,
}

impl CompositionSize {
    /// mpv's `WxH` form for `d3d11-composition-size`. Zero would fail the VO, so each side is at least 1.
    pub fn option_value(self) -> String {
        format!("{}x{}", self.width.max(1), self.height.max(1))
    }
}

/// What the UI reports about the monitor the window is on (ADR 0007).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayFacts {
    /// The monitor is in HDR mode (advanced colour).
    pub hdr: bool,
    /// Peak luminance in nits.
    pub peak_nits: f32,
    /// SDR white level in nits.
    pub sdr_white_nits: f32,
    /// Refresh rate in hertz.
    pub refresh_hz: f64,
}

/// ADR 0007's mapping from display facts to mpv options, plus `display-fps-override` (ADR 0019). The values are set as properties at
/// run time and as options at start.
pub fn display_options(facts: &DisplayFacts) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = if facts.hdr {
        vec![
            ("target-colorspace-hint", "yes".into()),
            ("target-colorspace-hint-mode", "target".into()),
            ("target-trc", "pq".into()),
            ("target-prim", "bt.2020".into()),
            ("target-peak", nits(facts.peak_nits)),
            ("hdr-reference-white", nits(facts.sdr_white_nits)),
        ]
    } else {
        vec![
            ("target-colorspace-hint", "no".into()),
            ("target-trc", "auto".into()),
            ("target-prim", "auto".into()),
            ("target-peak", "auto".into()),
            ("hdr-reference-white", "auto".into()),
        ]
    };
    if facts.refresh_hz.is_finite() && facts.refresh_hz > 0.0 {
        out.push(("display-fps-override", format!("{:.3}", facts.refresh_hz)));
    }
    out
}

/// Nits as mpv takes them (`target-peak` from 10, `hdr-reference-white` from 10): whole numbers, clamped to a sane range, with mpv's
/// default (203) for a value the display did not report.
fn nits(value: f32) -> String {
    if !value.is_finite() || value <= 0.0 {
        return "203".into();
    }
    format!("{}", value.round().clamp(10.0, 10_000.0) as u32)
}

/// How much of mpv's log the engine asks for (`mpv_request_log_messages`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogLevel {
    #[default]
    Info,
    /// mpv's `v`, while diagnosing.
    Verbose,
}

impl LogLevel {
    pub fn mpv_name(self) -> &'static str {
        match self {
            LogLevel::Info => "info",
            LogLevel::Verbose => "v",
        }
    }
}

/// How the engine starts.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    pub output: Output,
    /// Sent as the User-Agent of mpv's requests: `Current/<version>`.
    pub user_agent: String,
    pub log_level: LogLevel,
}

/// One option set before `mpv_initialize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartOption {
    pub name: &'static str,
    pub value: String,
    /// Set only where mpv knows the option: a build without it reports `option-not-found`, which is not an error then.
    pub optional: bool,
}

fn required(name: &'static str, value: impl Into<String>) -> StartOption {
    StartOption {
        name,
        value: value.into(),
        optional: false,
    }
}

fn optional(name: &'static str, value: impl Into<String>) -> StartOption {
    StartOption {
        name,
        value: value.into(),
        optional: true,
    }
}

/// The options at start, in order (ADR 0019 "Options at start"). Never `log-file`: mpv's messages come through the client API and are
/// redacted (ADR 0019).
pub fn start_options(config: &EngineConfig) -> Vec<StartOption> {
    let mut out = vec![
        required("config", "no"),
        required("idle", "yes"),
        // The controller decides what happens at the end.
        required("keep-open", "yes"),
        required("input-default-bindings", "no"),
        required("input-vo-keyboard", "no"),
        required("terminal", "no"),
        // mpv's default is off (ADR 0019, docs/notes/playback.md).
        required("tls-verify", "yes"),
        required("user-agent", config.user_agent.clone()),
        required("network-timeout", "30"),
        // Windows only (`media-controls`, mpv 0.41.0); the app drives the system media controls itself (ADR 0007).
        optional("media-controls", "no"),
        // Present only in builds with Lua, JavaScript or C plugins; ours has none (ADR 0019), a system libmpv on Linux may.
        optional("load-scripts", "no"),
        optional("osc", "no"),
        optional("ytdl", "no"),
    ];
    match &config.output {
        Output::Headless => {
            out.push(required("vo", "null"));
            out.push(required("ao", "null"));
        }
        Output::Composition { size, display } => {
            out.push(required("vo", "gpu-next"));
            out.push(required("gpu-api", "d3d11"));
            out.push(required("gpu-context", "d3d11"));
            out.push(required("d3d11-output-mode", "composition"));
            out.push(required("d3d11-composition-size", size.option_value()));
            out.push(required("hwdec", "auto-safe"));
            out.push(required("force-window", "yes"));
            out.extend(
                display_options(display)
                    .into_iter()
                    .map(|(name, value)| required(name, value)),
            );
        }
    }
    out
}

/// A gateway cookie of the address the stream is on (ADR 0017). It is scoped to the stream URL's host and port by the engine, not by its
/// own domain (see [`FileRequest::lavf_options`]).
#[derive(Clone, PartialEq, Eq)]
pub struct StreamCookie {
    pub name: String,
    pub value: String,
    pub path: String,
    /// Sent only when the stream URL is https.
    pub secure: bool,
}

impl std::fmt::Debug for StreamCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamCookie")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("path", &self.path)
            .field("secure", &self.secure)
            .finish()
    }
}

/// A track choice for `aid`/`sid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackChoice {
    /// mpv's own choice.
    Auto,
    Off,
    /// mpv's track id (`track-list/N/id`).
    Id(i64),
}

impl TrackChoice {
    pub(crate) fn option_value(self) -> String {
        match self {
            TrackChoice::Auto => "auto".into(),
            TrackChoice::Off => "no".into(),
            TrackChoice::Id(id) => id.to_string(),
        }
    }
}

/// One file for `loadfile`. The URL carries the token (`ApiKey`, ADR 0019 choice 3); it stays inside the engine.
#[derive(Clone, PartialEq)]
pub struct FileRequest {
    pub url: String,
    /// Start position in seconds (resume).
    pub start: Option<f64>,
    pub audio: Option<TrackChoice>,
    pub subtitle: Option<TrackChoice>,
    /// Gateway cookies, through FFmpeg's `cookies` (ADR 0019 choice 5).
    pub cookies: Vec<StreamCookie>,
    /// Basic and extra gateway headers, through FFmpeg's `headers`.
    pub headers: Vec<(String, String)>,
    /// Secret values the URL holds (the session token), for log redaction. Cookie and header values are added by the engine.
    pub secrets: Vec<String>,
    /// Set only through [`FileRequest::playlist_checked_same_origin`].
    playlist_checked: bool,
}

impl std::fmt::Debug for FileRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileRequest")
            .field("url", &"<redacted>")
            .field("start", &self.start)
            .field("audio", &self.audio)
            .field("subtitle", &self.subtitle)
            .field("cookies", &self.cookies)
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("secrets", &self.secrets.len())
            .field("playlist_checked", &self.playlist_checked)
            .finish()
    }
}

impl FileRequest {
    pub fn new(url: impl Into<String>) -> FileRequest {
        FileRequest {
            url: url.into(),
            start: None,
            audio: None,
            subtitle: None,
            cookies: Vec::new(),
            headers: Vec::new(),
            secrets: Vec::new(),
            playlist_checked: false,
        }
    }

    /// Marks an HLS or DASH stream whose playlists the caller has checked: it fetched the master playlist and every playlist it names,
    /// and found every URI (variants, segments, keys, maps) relative or on the stream's own origin. Only then may a playlist load carry
    /// gateway headers, because FFmpeg copies `headers` into every playlist, segment and key request, to whatever host it names
    /// (FFmpeg 9.0.2 `libavformat/aviobuf.c`, `ffio_copy_url_options`; ADR 0019, 2026-10-25).
    ///
    /// Not enforced by this: mpv fetches the playlists again itself, so a playlist that changes after the check (a live one) could
    /// still name another host and get the headers. The loopback relay planned for M3 enforces it (roadmap). Cookies are not affected:
    /// they carry the stream's host and port as their domain.
    pub fn playlist_checked_same_origin(mut self) -> FileRequest {
        self.playlist_checked = true;
        self
    }

    /// Whether the URL's path names an HLS or DASH playlist (`.m3u8`, `.m3u`, `.mpd`).
    pub fn looks_like_playlist(&self) -> bool {
        let path = self
            .url
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        [".m3u8", ".m3u", ".mpd"]
            .iter()
            .any(|ext| path.ends_with(ext))
    }

    /// Refuses a playlist URL that carries gateway credentials (headers or cookies) without
    /// [`FileRequest::playlist_checked_same_origin`]. Any other load with credentials and no mark gets mpv's `access-references=no`
    /// ([`FileRequest::file_options`]), so a stream that turns out to be a playlist cannot open its segments with them either.
    pub fn check_header_scope(&self) -> Result<(), EngineError> {
        if self.has_credentials() && !self.playlist_checked && self.looks_like_playlist() {
            return Err(EngineError::UncheckedPlaylist);
        }
        Ok(())
    }

    /// Whether the request carries gateway credentials (cookies or headers).
    pub fn has_credentials(&self) -> bool {
        !self.cookies.is_empty() || !self.headers.is_empty()
    }

    /// Checks cookies and headers for characters that would change what FFmpeg sends: control characters (CR and LF would start a new
    /// header line, a newline a new cookie), `;` and `=` in a cookie name, `;` in a cookie value or path (a new attribute), and `:` in
    /// a header name.
    pub fn validate(&self) -> Result<(), EngineError> {
        let bad = |s: &str, extra: &[char]| s.chars().any(|c| c.is_control() || extra.contains(&c));
        for c in &self.cookies {
            if c.name.is_empty() || bad(&c.name, &[';', '=', ' ']) {
                return Err(EngineError::InvalidCredential {
                    what: "cookie name",
                });
            }
            if bad(&c.value, &[';']) {
                return Err(EngineError::InvalidCredential {
                    what: "cookie value",
                });
            }
            if bad(&c.path, &[';']) {
                return Err(EngineError::InvalidCredential {
                    what: "cookie path",
                });
            }
        }
        for (name, value) in &self.headers {
            if name.is_empty() || bad(name, &[':', ' ']) {
                return Err(EngineError::InvalidCredential {
                    what: "header name",
                });
            }
            if bad(value, &[]) {
                return Err(EngineError::InvalidCredential {
                    what: "header value",
                });
            }
        }
        Ok(())
    }

    /// Every secret this request holds: the URL's, the cookie values and the header values.
    pub fn secret_values(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.secrets.iter().map(String::as_str).collect();
        out.extend(self.cookies.iter().map(|c| c.value.as_str()));
        out.extend(self.headers.iter().map(|(_, v)| v.as_str()));
        out
    }

    /// FFmpeg's options for this stream, as mpv's `stream-lavf-o` key-value list: `max_redirects=0` always (choice 4), then `cookies`
    /// and `headers` when there are any. Each value is written in mpv's `%<length>%<value>` form (`read_subparam` in mpv's
    /// `options/m_option.c`), which takes any bytes, so commas, `=` and newlines in a value cannot break the list.
    ///
    /// Cookies: FFmpeg 9.0.2 (`libavformat/http.c`, `get_cookies`) matches a cookie's `domain` as a suffix of the request's Host value,
    /// which carries the port whenever the URL names one, and it ignores `secure`. So each cookie gets the stream URL's own host and
    /// port as its domain (a cookie for `media.example.com` would never match `media.example.com:8096`), and a Secure cookie is left
    /// out unless the URL is https.
    pub fn lavf_options(&self) -> String {
        let mut pairs = vec![("max_redirects".to_owned(), "0".to_owned())];
        if let Some(cookies) = ffmpeg_cookies(&self.url, &self.cookies) {
            pairs.push(("cookies".into(), cookies));
        }
        if !self.headers.is_empty() {
            pairs.push(("headers".into(), ffmpeg_headers(&self.headers)));
        }
        pairs
            .iter()
            .map(|(k, v)| format!("{k}=%{}%{v}", v.len()))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// `loadfile`'s per-file options as a map of strings ("the values themselves must be strings currently", mpv's `input.rst`).
    pub(crate) fn file_options(&self) -> Node {
        let mut pairs = Vec::new();
        if let Some(start) = self.start.filter(|s| s.is_finite() && *s > 0.0) {
            pairs.push(("start".to_owned(), format!("{start:.3}")));
        }
        if let Some(aid) = self.audio {
            pairs.push(("aid".into(), aid.option_value()));
        }
        if let Some(sid) = self.subtitle {
            pairs.push(("sid".into(), sid.option_value()));
        }
        pairs.push(("stream-lavf-o".into(), self.lavf_options()));
        if self.has_credentials() && !self.playlist_checked {
            // No nested opens (playlist entries, segments, references): they would carry the headers to any host, and the cookies
            // to any host whose name ends in the stream's (FFmpeg matches a cookie domain as a suffix). mpv's
            // `block_io_open` refuses them ("Not opening '%s' due to --access-references=no", `demux/demux_lavf.c`).
            pairs.push(("access-references".into(), "no".into()));
        }
        Node::string_map(pairs)
    }

    /// The `loadfile` command with named arguments (mpv's `input.rst`: `loadfile <url> [<flags> [<index> [<options>]]]`), replacing
    /// whatever plays.
    pub(crate) fn loadfile_command(&self) -> Node {
        Node::Map(vec![
            ("name".into(), Node::String("loadfile".into())),
            ("url".into(), Node::String(self.url.clone())),
            ("flags".into(), Node::String("replace".into())),
            ("options".into(), self.file_options()),
        ])
    }
}

/// An external subtitle the core fetched itself (ADR 0019, "External subtitles"), added to the file playing with `sub-add` from memory,
/// so mpv never fetches a subtitle: UTF-8 text without NUL as `memory://`, anything else (PGS, VobSub) as `hex://`. mpv's `memory://`
/// "use[s] the data part as source data" and `hex://` reads it as a hex dump (mpv v0.41.0 `DOCS/man/mpv.rst`, "Protocols"; ADR 0019).
/// The command's named arguments (`url`, `flags`, `title`, `lang`) follow `sub-add`'s synopsis in `DOCS/man/input.rst`; not verified
/// against a live libmpv here.
#[derive(Clone, PartialEq, Eq)]
pub struct SubtitleFile {
    pub data: Vec<u8>,
    pub title: Option<String>,
    pub language: Option<String>,
    /// Show it at once (`select`), else only add it (`auto`).
    pub select: bool,
}

impl std::fmt::Debug for SubtitleFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubtitleFile")
            .field("bytes", &self.data.len())
            .field("title", &self.title)
            .field("language", &self.language)
            .field("select", &self.select)
            .finish()
    }
}

impl SubtitleFile {
    /// The `memory://` or `hex://` source.
    pub fn source(&self) -> String {
        match std::str::from_utf8(&self.data) {
            Ok(text) if !text.contains('\0') => format!("memory://{text}"),
            _ => {
                let mut hex = String::with_capacity(6 + self.data.len() * 2);
                hex.push_str("hex://");
                for byte in &self.data {
                    hex.push_str(&format!("{byte:02x}"));
                }
                hex
            }
        }
    }

    pub(crate) fn sub_add_command(&self) -> Node {
        let mut args = vec![
            ("name".into(), Node::String("sub-add".into())),
            ("url".into(), Node::String(self.source())),
            (
                "flags".into(),
                Node::String(if self.select { "select" } else { "auto" }.into()),
            ),
        ];
        if let Some(title) = &self.title {
            args.push(("title".into(), Node::String(title.clone())));
        }
        if let Some(language) = &self.language {
            args.push(("lang".into(), Node::String(language.clone())));
        }
        Node::Map(args)
    }
}

/// Whether an FFmpeg version string (`av_version_info()`, mpv's `ffmpeg-version`) names FFmpeg 9.0 or later, the first with the http
/// protocol's `max_redirects` (in FFmpeg 9.0.2's `libavformat/http.c`, not in n8.0's; docs/notes/mpv-engine.md). Release builds read
/// `9.0.2` or `n9.0.2`, distribution builds `6.1.1-3ubuntu5`; a git build (`N-12345-g...`) or anything else unreadable counts as no.
pub fn ffmpeg_has_max_redirects(version: &str) -> bool {
    let digits = version.strip_prefix('n').unwrap_or(version);
    let major: String = digits.chars().take_while(char::is_ascii_digit).collect();
    major.parse::<u32>().is_ok_and(|m| m >= 9)
}

/// FFmpeg's `cookies`: "newline delimited Set-Cookie HTTP field value syntax" (FFmpeg `libavformat/http.c`), each with the URL's host
/// and port as its domain and its path. `None` when no cookie applies or the URL has no host.
fn ffmpeg_cookies(url: &str, cookies: &[StreamCookie]) -> Option<String> {
    let (scheme, host) = scheme_and_host(url)?;
    let https = scheme.eq_ignore_ascii_case("https");
    let lines: Vec<String> = cookies
        .iter()
        .filter(|c| https || !c.secure)
        .map(|c| {
            let path = if c.path.is_empty() { "/" } else { &c.path };
            format!("{}={}; domain={host}; path={path}", c.name, c.value)
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// The scheme and the host with its port as the URL writes them (`https://user@media.example.com:8096/x` gives `https` and
/// `media.example.com:8096`), which is the Host value FFmpeg compares cookie domains with.
fn scheme_and_host(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    (!host.is_empty()).then_some((scheme, host))
}

/// FFmpeg's `headers`: header lines, each ending in CRLF.
fn ffmpeg_headers(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .map(|(n, v)| format!("{n}: {v}\r\n"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(opts: &[StartOption]) -> Vec<&str> {
        opts.iter().map(|o| o.name).collect()
    }

    fn value<'a>(opts: &'a [StartOption], name: &str) -> &'a str {
        &opts
            .iter()
            .find(|o| o.name == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .value
    }

    #[test]
    fn headless_start_options() {
        let config = EngineConfig {
            output: Output::Headless,
            user_agent: "Current/0.1.0".into(),
            log_level: LogLevel::Info,
        };
        let opts = start_options(&config);
        assert_eq!(value(&opts, "vo"), "null");
        assert_eq!(value(&opts, "ao"), "null");
        assert_eq!(value(&opts, "tls-verify"), "yes");
        assert_eq!(value(&opts, "keep-open"), "yes");
        assert_eq!(value(&opts, "user-agent"), "Current/0.1.0");
        assert!(!names(&opts).contains(&"log-file"));
        assert!(!names(&opts).contains(&"d3d11-output-mode"));
        assert!(
            opts.iter().filter(|o| o.optional).all(|o| [
                "media-controls",
                "load-scripts",
                "osc",
                "ytdl"
            ]
            .contains(&o.name))
        );
    }

    #[test]
    fn composition_start_options_carry_the_route_size_and_display() {
        let display = DisplayFacts {
            hdr: true,
            peak_nits: 1015.4,
            sdr_white_nits: 240.0,
            refresh_hz: 143.856,
        };
        let config = EngineConfig {
            output: Output::Composition {
                size: CompositionSize {
                    width: 1920,
                    height: 1080,
                },
                display,
            },
            user_agent: "Current/0.1.0".into(),
            log_level: LogLevel::Verbose,
        };
        let opts = start_options(&config);
        for (n, v) in [
            ("vo", "gpu-next"),
            ("gpu-api", "d3d11"),
            ("gpu-context", "d3d11"),
            ("d3d11-output-mode", "composition"),
            ("d3d11-composition-size", "1920x1080"),
            ("hwdec", "auto-safe"),
            ("force-window", "yes"),
            ("target-peak", "1015"),
            ("hdr-reference-white", "240"),
            ("display-fps-override", "143.856"),
        ] {
            assert_eq!(value(&opts, n), v, "{n}");
        }
        assert_eq!(LogLevel::Verbose.mpv_name(), "v");
    }

    #[test]
    fn display_mapping_hdr_and_sdr() {
        let hdr = display_options(&DisplayFacts {
            hdr: true,
            peak_nits: 600.0,
            sdr_white_nits: 80.0,
            refresh_hz: 60.0,
        });
        assert_eq!(
            hdr,
            vec![
                ("target-colorspace-hint", "yes".to_owned()),
                ("target-colorspace-hint-mode", "target".into()),
                ("target-trc", "pq".into()),
                ("target-prim", "bt.2020".into()),
                ("target-peak", "600".into()),
                ("hdr-reference-white", "80".into()),
                ("display-fps-override", "60.000".into()),
            ]
        );
        let sdr = display_options(&DisplayFacts {
            hdr: false,
            peak_nits: 0.0,
            sdr_white_nits: 0.0,
            refresh_hz: 0.0,
        });
        assert_eq!(sdr[0], ("target-colorspace-hint", "no".to_owned()));
        assert!(sdr.iter().skip(1).all(|(_, v)| v == "auto"));
        assert!(!sdr.iter().any(|(n, _)| *n == "display-fps-override"));
        // A display that reports nothing usable falls back to mpv's default white.
        let odd = display_options(&DisplayFacts {
            hdr: true,
            peak_nits: f32::NAN,
            sdr_white_nits: 3.0,
            refresh_hz: f64::NAN,
        });
        assert!(odd.contains(&("target-peak", "203".into())));
        assert!(odd.contains(&("hdr-reference-white", "10".into())));
        assert_eq!(
            CompositionSize {
                width: 0,
                height: 5
            }
            .option_value(),
            "1x5"
        );
    }

    #[test]
    fn lavf_options_always_refuse_redirects_and_escape_values() {
        let plain = FileRequest::new("https://media.example.com/x");
        assert_eq!(plain.lavf_options(), "max_redirects=%1%0");

        let mut req = FileRequest::new("https://media.example.com:8920/x?ApiKey=t");
        req.cookies.push(StreamCookie {
            name: "gate".into(),
            value: "a,b=c".into(),
            path: String::new(),
            secure: true,
        });
        req.cookies.push(StreamCookie {
            name: "two".into(),
            value: "v".into(),
            path: "/web".into(),
            secure: false,
        });
        req.headers
            .push(("Authorization".into(), "Basic eDp5".into()));
        let cookies = "gate=a,b=c; domain=media.example.com:8920; path=/\ntwo=v; domain=media.example.com:8920; path=/web";
        let headers = "Authorization: Basic eDp5\r\n";
        assert_eq!(
            req.lavf_options(),
            format!(
                "max_redirects=%1%0,cookies=%{}%{cookies},headers=%{}%{headers}",
                cookies.len(),
                headers.len()
            )
        );
        assert_eq!(req.secret_values(), vec!["a,b=c", "v", "Basic eDp5"]);
    }

    #[test]
    fn secure_cookies_only_over_https_and_none_without_a_host() {
        let cookie = |secure| StreamCookie {
            name: "c".into(),
            value: "v".into(),
            path: "/".into(),
            secure,
        };
        let mut req = FileRequest::new("http://user@192.168.1.10/Videos/1/stream");
        req.cookies = vec![cookie(true), cookie(false)];
        assert_eq!(
            req.lavf_options(),
            format!(
                "max_redirects=%1%0,cookies=%{}%c=v; domain=192.168.1.10; path=/",
                32
            )
        );
        req.cookies = vec![cookie(true)];
        assert_eq!(req.lavf_options(), "max_redirects=%1%0");
        let mut file = FileRequest::new("/tmp/tone.wav");
        file.cookies = vec![cookie(false)];
        assert_eq!(file.lavf_options(), "max_redirects=%1%0");
        assert_eq!(
            scheme_and_host("HTTPS://[::1]:8920?x"),
            Some(("HTTPS", "[::1]:8920"))
        );
    }

    #[test]
    fn headers_on_a_playlist_need_the_same_origin_check() {
        let with_header = |url: &str| {
            let mut r = FileRequest::new(url);
            r.headers.push(("X-Gate".into(), "v".into()));
            r
        };
        for playlist in [
            "https://media.example.com/videos/1/master.m3u8?ApiKey=t",
            "https://media.example.com/a/MAIN.M3U8",
            "https://media.example.com/x.mpd#t=1",
        ] {
            let unchecked = with_header(playlist);
            assert!(unchecked.looks_like_playlist(), "{playlist}");
            assert!(matches!(
                unchecked.check_header_scope(),
                Err(EngineError::UncheckedPlaylist)
            ));
            let checked = with_header(playlist).playlist_checked_same_origin();
            assert!(checked.check_header_scope().is_ok());
            assert!(checked.file_options().get("access-references").is_none());
        }
        // Not a playlist by its URL: allowed, but with nested opens blocked.
        let direct = with_header("https://media.example.com/Videos/1/stream?static=true&x=.m3u8");
        assert!(direct.check_header_scope().is_ok());
        assert_eq!(
            direct
                .file_options()
                .get("access-references")
                .and_then(Node::as_str),
            Some("no")
        );
        // Cookies alone need the check too: FFmpeg matches their domain as a suffix of the Host, so a look-alike host would get them.
        let mut cookie_only = FileRequest::new("https://media.example.com/videos/1/master.m3u8");
        cookie_only.cookies.push(StreamCookie {
            name: "gate".into(),
            value: "v".into(),
            path: "/".into(),
            secure: true,
        });
        assert!(matches!(
            cookie_only.check_header_scope(),
            Err(EngineError::UncheckedPlaylist)
        ));
        assert_eq!(
            cookie_only
                .file_options()
                .get("access-references")
                .and_then(Node::as_str),
            Some("no")
        );
        let cookie_only = cookie_only.playlist_checked_same_origin();
        assert!(cookie_only.check_header_scope().is_ok());
        assert!(
            cookie_only
                .file_options()
                .get("access-references")
                .is_none()
        );
        // Without credentials nothing changes.
        let plain = FileRequest::new("https://media.example.com/master.m3u8");
        assert!(plain.check_header_scope().is_ok());
        assert!(plain.file_options().get("access-references").is_none());
    }

    #[test]
    fn ffmpeg_versions() {
        for yes in ["9.0.2", "n9.0.2", "10.1", "n12.0-dev"] {
            assert!(ffmpeg_has_max_redirects(yes), "{yes}");
        }
        for no in [
            "8.0",
            "n8.0.1",
            "6.1.1-3ubuntu5",
            "N-12345-gabcdef",
            "",
            "git-2026",
        ] {
            assert!(!ffmpeg_has_max_redirects(no), "{no}");
        }
    }

    #[test]
    fn credentials_that_would_change_the_request_are_refused() {
        let cookie = |name: &str, value: &str, path: &str| StreamCookie {
            name: name.into(),
            value: value.into(),
            path: path.into(),
            secure: false,
        };
        let with = |cookies: Vec<StreamCookie>, headers: Vec<(&str, &str)>| {
            let mut r = FileRequest::new("https://media.example.com/x");
            r.cookies = cookies;
            r.headers = headers
                .into_iter()
                .map(|(n, v)| (n.into(), v.into()))
                .collect();
            r.validate().err().map(|e| e.to_string())
        };
        assert_eq!(
            with(
                vec![cookie("gate", "a=b,c", "/")],
                vec![("X-Gate", "v=1; x")]
            ),
            None
        );
        assert!(!FileRequest::new("https://media.example.com/x").has_credentials());
        for (cookies, headers, what) in [
            (vec![cookie("ga;te", "v", "/")], vec![], "cookie name"),
            (vec![cookie("", "v", "/")], vec![], "cookie name"),
            (
                vec![cookie("gate", "v; domain=evil", "/")],
                vec![],
                "cookie value",
            ),
            (
                vec![cookie("gate", "v\nother=1", "/")],
                vec![],
                "cookie value",
            ),
            (
                vec![cookie("gate", "v", "/; secure")],
                vec![],
                "cookie path",
            ),
            (vec![], vec![("X-Gate", "v\r\nHost: evil")], "header value"),
            (vec![], vec![("X-Gate: a", "v")], "header name"),
        ] {
            assert_eq!(
                with(cookies, headers),
                Some(format!("invalid gateway credential: {what}"))
            );
        }
    }

    #[test]
    fn loadfile_command_with_per_file_options() {
        let mut req = FileRequest::new("https://media.example.com/v?ApiKey=t0ken");
        req.start = Some(83.5);
        req.audio = Some(TrackChoice::Id(2));
        req.subtitle = Some(TrackChoice::Off);
        let cmd = req.loadfile_command();
        assert_eq!(cmd.get("name").and_then(Node::as_str), Some("loadfile"));
        assert_eq!(cmd.get("flags").and_then(Node::as_str), Some("replace"));
        let opts = cmd.get("options").unwrap();
        assert_eq!(opts.get("start").and_then(Node::as_str), Some("83.500"));
        assert_eq!(opts.get("aid").and_then(Node::as_str), Some("2"));
        assert_eq!(opts.get("sid").and_then(Node::as_str), Some("no"));
        assert_eq!(
            opts.get("stream-lavf-o").and_then(Node::as_str),
            Some("max_redirects=%1%0")
        );
        // No start for zero, so a start-over plays from the beginning without an option.
        req.start = Some(0.0);
        assert!(req.file_options().get("start").is_none());
    }

    #[test]
    fn subtitles_are_added_from_memory_text_or_hex() {
        let text = SubtitleFile {
            data: b"1\n00:00:01,000 --> 00:00:02,000\nHello\n".to_vec(),
            title: Some("English".into()),
            language: Some("eng".into()),
            select: true,
        };
        let cmd = text.sub_add_command();
        assert_eq!(cmd.get("name").and_then(Node::as_str), Some("sub-add"));
        assert_eq!(
            cmd.get("url").and_then(Node::as_str),
            Some("memory://1\n00:00:01,000 --> 00:00:02,000\nHello\n")
        );
        assert_eq!(cmd.get("flags").and_then(Node::as_str), Some("select"));
        assert_eq!(cmd.get("title").and_then(Node::as_str), Some("English"));
        assert_eq!(cmd.get("lang").and_then(Node::as_str), Some("eng"));
        let binary = SubtitleFile {
            data: vec![0x50, 0x47, 0x00, 0xff],
            title: None,
            language: None,
            select: false,
        };
        assert_eq!(binary.source(), "hex://504700ff");
        let cmd = binary.sub_add_command();
        assert_eq!(cmd.get("flags").and_then(Node::as_str), Some("auto"));
        assert!(cmd.get("title").is_none());
        assert!(!format!("{text:?}").contains("Hello"));
    }

    #[test]
    fn debug_never_shows_the_url_or_secret_values() {
        let mut req = FileRequest::new("https://media.example.com/v?ApiKey=t0ken-value");
        req.secrets.push("t0ken-value".into());
        req.cookies.push(StreamCookie {
            name: "gate".into(),
            value: "cookie-secret".into(),
            path: "/".into(),
            secure: true,
        });
        req.headers.push(("X-Gate".into(), "header-secret".into()));
        let text = format!("{req:?}");
        for secret in [
            "t0ken-value",
            "cookie-secret",
            "header-secret",
            "media.example.com/v",
        ] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}
