//! The engine: one libmpv player, its event thread and its commands (ADR 0019 "`current-mpv`", "Threads and event rate").
//!
//! - Commands go from any thread, always asynchronously (`mpv_command_node_async`, `mpv_set_property_async`), so no caller waits on
//!   mpv's lock; a failure arrives later as [`EngineEvent::CommandFailed`]. The client API is "generally fully thread-safe"
//!   (`client.h`).
//! - One OS thread per engine (`mpv-events`) blocks in `mpv_wait_event` and hands each event to the sink. It never calls a UI listener
//!   (the sink is the controller's channel). No wakeup callback is used.
//! - Dropping the engine sends `quit`; the event thread then calls `mpv_terminate_destroy` (which blocks until mpv's VO thread has
//!   ended, ADR 0007) on itself, off any core worker, and reports [`EngineEvent::Shutdown`] last. [`Engine::shutdown`] does the same and
//!   waits for it. The handle is destroyed only after the `Engine` has made its last call (`released`): if mpv shuts down on its own,
//!   the thread reports [`EngineEvent::Terminated`] and waits for the drop. A panicking sink stops delivery but not that cleanup.
//! - mpv's messages arrive at `info` (or `v`) through `mpv_request_log_messages` and are redacted before they reach the sink. The engine
//!   never sets `log-file` and never reads a URL-valued property.
//! - **One engine per process at a time** (ADR 0019, 2026-10-25). mpv routes FFmpeg's log to the first mpv instance of the process only,
//!   and to stderr once that instance is destroyed (mpv v0.41.0 `common/av_log.c`: `init_libav` sets `log_mpv_instance` only `if
//!   (!log_mpv_instance)`, `uninit_libav` clears it, and `mp_msg_av_log_callback` then falls back to `vfprintf(stderr, fmt, vl)`). Two
//!   engines alive at once would send FFmpeg's lines, URLs included, past this engine's redaction. So [`Engine::start`] waits for an
//!   earlier engine to be destroyed and fails with [`EngineError::Busy`] if it is not gone in time.
//! - Hence the engine lives as long as the player: the next episode, a re-plan and any other change of file reuse it through
//!   [`Engine::load`] (`loadfile` replaces the file); it is never destroyed and created again between files.

// Calls into libmpv need `unsafe`; this module and `sys` are the only places allowed (crate manifest).
#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::caps::BuildCapabilities;
use crate::error::{EngineError, MpvError};
use crate::event::{self, EndReason, EngineEvent, LogLine, OBSERVED, Observed};
use crate::node::Node;
use crate::options::{
    self, CompositionSize, DisplayFacts, EngineConfig, FileRequest, Output, SubtitleFile,
    TrackChoice,
};
use crate::redact::Redactor;
use crate::swapchain::SwapChain;
use crate::sys;

/// Where engine events go. Called on the event thread, so it must not block for long: a channel send.
pub type EventSink = Box<dyn FnMut(EngineEvent) + Send>;

/// The asynchronous requests, by reply id. Ids start above the observed properties' only for readability; replies and property changes
/// are different event types.
#[derive(Debug, Clone, Copy)]
enum Request {
    Load = 101,
    Stop,
    Pause,
    Seek,
    Audio,
    Subtitle,
    Volume,
    Mute,
    Display,
    CompositionSize,
    SubAdd,
    Quit,
}

impl Request {
    const ALL: [Request; 12] = [
        Request::Load,
        Request::Stop,
        Request::Pause,
        Request::Seek,
        Request::Audio,
        Request::Subtitle,
        Request::Volume,
        Request::Mute,
        Request::Display,
        Request::CompositionSize,
        Request::SubAdd,
        Request::Quit,
    ];

    fn what(self) -> &'static str {
        match self {
            Request::Load => "loadfile",
            Request::Stop => "stop",
            Request::Pause => "pause",
            Request::Seek => "seek",
            Request::Audio => "aid",
            Request::Subtitle => "sid",
            Request::Volume => "volume",
            Request::Mute => "mute",
            Request::Display => "display facts",
            Request::CompositionSize => "d3d11-composition-size",
            Request::SubAdd => "sub-add",
            Request::Quit => "quit",
        }
    }

    fn from_reply(id: u64) -> &'static str {
        Request::ALL
            .iter()
            .find(|r| **r as u64 == id)
            .map_or("request", |r| r.what())
    }
}

/// The handle, shared by the engine (commands) and its event thread.
struct Shared {
    handle: *mut sys::mpv_handle,
    redactor: Mutex<Redactor>,
    /// The engine asked mpv to quit (set before the `quit` command, so a `SHUTDOWN` that follows is not reported as unrequested).
    quitting: AtomicBool,
    /// The engine made its last call on the handle (set after `quit`); only then may the event thread destroy it.
    released: Mutex<bool>,
    released_cv: Condvar,
}

impl Shared {
    fn release(&self) {
        *self.released.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.released_cv.notify_all();
    }

    fn wait_released(&self) {
        let mut released = self.released.lock().unwrap_or_else(PoisonError::into_inner);
        while !*released {
            released = self
                .released_cv
                .wait(released)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

// SAFETY: libmpv's client API may be called on one handle from any thread ("generally fully thread-safe", `client.h`), except
// `mpv_terminate_destroy`, which only the event thread calls, after the `Engine` (the only other user) has set `released` and makes no
// further call.
unsafe impl Send for Shared {}
// SAFETY: as above; the redactor is behind a mutex and the flag is atomic.
unsafe impl Sync for Shared {}

/// A libmpv player.
pub struct Engine {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    capabilities: BuildCapabilities,
    /// FFmpeg honours `max_redirects` (9.0 or later): without it, files with gateway credentials are refused.
    refuses_redirects: bool,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("capabilities", &self.capabilities)
            .finish_non_exhaustive()
    }
}

/// The client API major version the declarations in `sys` follow.
const API_MAJOR: u32 = 2;

/// How long [`Engine::start`] waits for an earlier engine's destroy (`mpv_terminate_destroy` waits for mpv's threads, ADR 0007).
const START_WAIT: Duration = Duration::from_secs(5);

/// Whether an engine exists in this process: taken before `mpv_create`, given back after `mpv_terminate_destroy` (module doc).
static SLOT: Mutex<bool> = Mutex::new(false);
static SLOT_FREED: Condvar = Condvar::new();

/// Takes the process's engine slot, waiting up to `wait` for it to be given back.
fn take_slot(wait: Duration) -> Result<(), EngineError> {
    let deadline = Instant::now() + wait;
    let mut taken = SLOT.lock().unwrap_or_else(PoisonError::into_inner);
    while *taken {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(EngineError::Busy);
        }
        taken = SLOT_FREED
            .wait_timeout(taken, left)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
    *taken = true;
    Ok(())
}

fn give_back_slot() {
    *SLOT.lock().unwrap_or_else(PoisonError::into_inner) = false;
    SLOT_FREED.notify_all();
}

impl Engine {
    /// Creates and initialises a player with the options at start, observes its properties, reads its capabilities and starts the event
    /// thread. Blocking (mpv starts its threads and, in composition mode, its VO): call it on a blocking thread, not a core worker.
    ///
    /// Only one engine exists per process (module doc): this waits up to 5 s for an earlier one's destroy to finish, then fails with
    /// [`EngineError::Busy`].
    pub fn start(config: &EngineConfig, sink: EventSink) -> Result<Engine, EngineError> {
        Engine::start_waiting(config, sink, START_WAIT)
    }

    pub(crate) fn start_waiting(
        config: &EngineConfig,
        sink: EventSink,
        wait: Duration,
    ) -> Result<Engine, EngineError> {
        // SAFETY: no arguments; returns a number.
        let version = unsafe { sys::mpv_client_api_version() } as u64;
        let (major, minor) = ((version >> 16) as u32, (version & 0xffff) as u32);
        if major != API_MAJOR {
            return Err(EngineError::ApiVersion { major, minor });
        }
        take_slot(wait)?;
        // SAFETY: no arguments; NULL on failure.
        let handle = unsafe { sys::mpv_create() };
        if handle.is_null() {
            give_back_slot();
            return Err(EngineError::Create);
        }
        let shared = Arc::new(Shared {
            handle,
            redactor: Mutex::new(Redactor::new()),
            quitting: AtomicBool::new(false),
            released: Mutex::new(false),
            released_cv: Condvar::new(),
        });
        match Engine::init(&shared, config) {
            Ok((capabilities, refuses_redirects)) => {
                let thread_shared = Arc::clone(&shared);
                let composition = matches!(config.output, Output::Composition { .. });
                let spawned = std::thread::Builder::new()
                    .name("mpv-events".into())
                    .spawn(move || event_loop(&thread_shared, sink, composition));
                match spawned {
                    Ok(thread) => Ok(Engine {
                        shared,
                        thread: Some(thread),
                        capabilities,
                        refuses_redirects,
                    }),
                    Err(e) => {
                        destroy(&shared);
                        Err(EngineError::Thread(e))
                    }
                }
            }
            Err(e) => {
                destroy(&shared);
                Err(e)
            }
        }
    }

    /// The capabilities, and whether FFmpeg refuses redirects.
    fn init(
        shared: &Shared,
        config: &EngineConfig,
    ) -> Result<(BuildCapabilities, bool), EngineError> {
        let h = shared.handle;
        for opt in options::start_options(config) {
            let name = cstring(opt.name, "option name")?;
            let value = cstring(&opt.value, opt.name)?;
            // SAFETY: valid handle and NUL-terminated strings that outlive the call; mpv copies them.
            let status = unsafe { sys::mpv_set_option_string(h, name.as_ptr(), value.as_ptr()) };
            match MpvError::from_status(status) {
                None => {}
                Some(MpvError::OPTION_NOT_FOUND) if opt.optional => {}
                Some(error) => {
                    return Err(EngineError::Mpv {
                        what: opt.name,
                        error,
                    });
                }
            }
        }
        let level = cstring(config.log_level.mpv_name(), "log level")?;
        // SAFETY: valid handle and string.
        check("log messages", unsafe {
            sys::mpv_request_log_messages(h, level.as_ptr())
        })?;
        // SAFETY: valid, not yet initialised handle.
        check("initialize", unsafe { sys::mpv_initialize(h) })?;
        let composition = matches!(config.output, Output::Composition { .. });
        for (i, name) in OBSERVED.iter().enumerate() {
            // `display-swapchain` exists only in composition mode.
            if *name == "display-swapchain" && !composition {
                continue;
            }
            let cname = cstring(name, "property")?;
            // SAFETY: valid handle and string; the reply id is the index + 1 (`event::observe_id`).
            check(name, unsafe {
                sys::mpv_observe_property(h, i as u64 + 1, cname.as_ptr(), sys::FORMAT_NODE)
            })?;
        }
        let decoders = get_node(h, "decoder-list")?;
        let demuxers = get_node(h, "demuxer-lavf-list")?;
        // `ffmpeg-version` is FFmpeg's `av_version_info()` (mpv's `input.rst`).
        let ffmpeg = get_node(h, "ffmpeg-version")?;
        let refuses_redirects = ffmpeg
            .as_str()
            .is_some_and(options::ffmpeg_has_max_redirects);
        Ok((
            BuildCapabilities::from_nodes(&decoders, &demuxers),
            refuses_redirects,
        ))
    }

    /// What this build decodes and demuxes, read after `mpv_initialize`.
    pub fn capabilities(&self) -> &BuildCapabilities {
        &self.capabilities
    }

    /// Loads a file, replacing whatever plays, with its per-file options (`start`, `aid`, `sid`, `stream-lavf-o`). Its secrets join the
    /// redaction set first, so no log line about it shows them.
    ///
    /// Refused when a cookie or header is malformed, when the file carries gateway credentials but FFmpeg cannot refuse redirects
    /// (`max_redirects` needs FFmpeg 9.0 or later: FFmpeg keeps headers on a redirect to another host, ADR 0019 choice 4), and when a
    /// playlist carries headers without [`FileRequest::playlist_checked_same_origin`].
    pub fn load(&self, file: &FileRequest) -> Result<(), EngineError> {
        file.validate()?;
        file.check_header_scope()?;
        if file.has_credentials() && !self.refuses_redirects {
            return Err(EngineError::RedirectsNotRefused);
        }
        {
            let mut redactor = self
                .shared
                .redactor
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for secret in file.secret_values() {
                redactor.add(secret);
            }
        }
        self.command(Request::Load, &file.loadfile_command())
    }

    /// Adds an external subtitle the core fetched to the file playing (`sub-add` from memory, [`SubtitleFile`]).
    pub fn add_subtitle(&self, file: &SubtitleFile) -> Result<(), EngineError> {
        self.command(Request::SubAdd, &file.sub_add_command())
    }

    /// Stops and unloads the file (`stop`); the engine stays for the next one.
    pub fn stop(&self) -> Result<(), EngineError> {
        self.command(Request::Stop, &command(&["stop"]))
    }

    pub fn set_paused(&self, paused: bool) -> Result<(), EngineError> {
        self.set(Request::Pause, "pause", if paused { "yes" } else { "no" })
    }

    /// Seeks to an absolute position in seconds.
    pub fn seek_to(&self, seconds: f64) -> Result<(), EngineError> {
        self.command(
            Request::Seek,
            &command(&["seek", &format!("{seconds:.3}"), "absolute"]),
        )
    }

    /// Seeks by a number of seconds, back when negative.
    pub fn seek_by(&self, seconds: f64) -> Result<(), EngineError> {
        self.command(
            Request::Seek,
            &command(&["seek", &format!("{seconds:.3}"), "relative"]),
        )
    }

    pub fn set_audio(&self, track: TrackChoice) -> Result<(), EngineError> {
        self.set(Request::Audio, "aid", &track.option_value())
    }

    pub fn set_subtitle(&self, track: TrackChoice) -> Result<(), EngineError> {
        self.set(Request::Subtitle, "sid", &track.option_value())
    }

    /// Volume in percent (100 is unchanged).
    pub fn set_volume(&self, percent: f64) -> Result<(), EngineError> {
        self.set(
            Request::Volume,
            "volume",
            &format!("{:.1}", percent.clamp(0.0, 100.0)),
        )
    }

    pub fn set_muted(&self, muted: bool) -> Result<(), EngineError> {
        self.set(Request::Mute, "mute", if muted { "yes" } else { "no" })
    }

    /// Applies new display facts (ADR 0007's mapping); on a monitor change or `AdvancedColorInfoChanged`.
    pub fn set_display_facts(&self, facts: &DisplayFacts) -> Result<(), EngineError> {
        for (name, value) in options::display_options(facts) {
            self.set(Request::Display, name, &value)?;
        }
        Ok(())
    }

    /// The panel's new size in physical pixels; mpv resizes on its VO thread (ADR 0007).
    pub fn set_composition_size(&self, size: CompositionSize) -> Result<(), EngineError> {
        self.set(
            Request::CompositionSize,
            "d3d11-composition-size",
            &size.option_value(),
        )
    }

    /// Quits mpv and waits until the event thread has destroyed it (it reports [`EngineEvent::Shutdown`] last). Blocking: run it on a
    /// blocking thread. The UI must have released the swap chain first (ADR 0007's order).
    pub fn shutdown(mut self) {
        self.request_quit();
        if let Some(thread) = self.thread.take() {
            // A panic on the event thread has nothing left to clean up here.
            let _ = thread.join();
        }
    }

    /// Sets any property: tests only (the TLS test's control case turns `tls-verify` off).
    #[cfg(test)]
    pub(crate) fn set_for_test(&self, name: &'static str, value: &str) -> Result<(), EngineError> {
        self.set(Request::Display, name, value)
    }

    /// Runs any command: tests only (mpv quitting on its own).
    #[cfg(test)]
    pub(crate) fn command_for_test(&self, args: &[&str]) -> Result<(), EngineError> {
        self.command(Request::Stop, &command(args))
    }

    fn request_quit(&self) {
        if self.shared.quitting.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.command(Request::Quit, &command(&["quit"])).is_err() {
            // The event thread checks the flag whenever it wakes.
            // SAFETY: valid handle; destroyed only after `release` below.
            unsafe { sys::mpv_wakeup(self.shared.handle) };
        }
        // The last call on the handle from this side: the event thread may destroy it now.
        self.shared.release();
    }

    fn command(&self, request: Request, cmd: &Node) -> Result<(), EngineError> {
        let mut node = sys::CNode::new(cmd).ok_or(EngineError::Nul {
            what: request.what(),
        })?;
        // SAFETY: valid handle; `node` owns a valid tree for the duration of the call, and mpv copies the command before returning.
        let status = unsafe {
            sys::mpv_command_node_async(self.shared.handle, request as u64, node.as_mut_ptr())
        };
        check(request.what(), status)
    }

    fn set(&self, request: Request, name: &'static str, value: &str) -> Result<(), EngineError> {
        let cname = cstring(name, "property")?;
        let cvalue = cstring(value, name)?;
        let mut ptr: *const c_char = cvalue.as_ptr();
        // SAFETY: valid handle; for `MPV_FORMAT_STRING` the data is a `char**`, which `ptr` is; "the value will be copied by the
        // function" (`client.h`), so both strings only need to outlive the call.
        let status = unsafe {
            sys::mpv_set_property_async(
                self.shared.handle,
                request as u64,
                cname.as_ptr(),
                sys::FORMAT_STRING,
                &mut ptr as *mut *const c_char as *mut c_void,
            )
        };
        check(name, status)
    }
}

impl Drop for Engine {
    /// Asks mpv to quit without waiting; the event thread destroys it.
    fn drop(&mut self) {
        self.request_quit();
    }
}

fn command(args: &[&str]) -> Node {
    Node::Array(args.iter().map(|a| Node::String((*a).to_owned())).collect())
}

fn cstring(s: &str, what: &'static str) -> Result<CString, EngineError> {
    CString::new(s).map_err(|_| EngineError::Nul { what })
}

fn check(what: &'static str, status: i32) -> Result<(), EngineError> {
    match MpvError::from_status(status) {
        None => Ok(()),
        Some(error) => Err(EngineError::Mpv { what, error }),
    }
}

fn get_node(h: *mut sys::mpv_handle, name: &'static str) -> Result<Node, EngineError> {
    let cname = cstring(name, "property")?;
    let mut raw = sys::mpv_node {
        u: sys::mpv_node_u { int64: 0 },
        format: sys::FORMAT_NONE,
    };
    // SAFETY: valid handle and string; for `MPV_FORMAT_NODE` the data is an `mpv_node*` that mpv fills.
    let status = unsafe {
        sys::mpv_get_property(
            h,
            cname.as_ptr(),
            sys::FORMAT_NODE,
            &mut raw as *mut _ as *mut c_void,
        )
    };
    check(name, status)?;
    // SAFETY: mpv filled `raw` with a valid tree, which is only read here.
    let node = unsafe { sys::read_node(&raw) };
    // SAFETY: `raw` holds what mpv allocated for this call; it is freed exactly once and not used after.
    unsafe { sys::mpv_free_node_contents(&mut raw) };
    Ok(node)
}

fn destroy(shared: &Shared) {
    // SAFETY: called once, by whoever holds the handle last: `start` on failure (no thread exists) or the event thread at its end.
    unsafe { sys::mpv_terminate_destroy(shared.handle) };
    // mpv is gone, FFmpeg's log callback with it: the next engine may start.
    give_back_slot();
}

/// The `mpv-events` thread: pumps events, then destroys mpv once the engine has released the handle, whatever happened in between (a
/// panicking sink included), and reports [`EngineEvent::Shutdown`] last.
fn event_loop(shared: &Shared, mut sink: EventSink, composition: bool) {
    let pumped = catch_unwind(AssertUnwindSafe(|| pump(shared, &mut sink, composition)));
    shared.wait_released();
    destroy(shared);
    if pumped.is_ok() {
        // A sink that panicked before gets nothing more.
        let _ = catch_unwind(AssertUnwindSafe(|| sink(EngineEvent::Shutdown)));
    }
}

/// Hands mpv's events to the sink until mpv shuts down or the engine quits. A panic in the sink unwinds out of here.
fn pump(shared: &Shared, sink: &mut EventSink, composition: bool) {
    let mut swap_chain_ptr: u64 = 0;
    let mut generation: u64 = 0;
    loop {
        // SAFETY: valid handle; only this thread waits on it. The event and everything it points to stay valid until the next
        // `mpv_wait_event` on this thread, and are only read before that.
        let ev = unsafe { &*sys::mpv_wait_event(shared.handle, -1.0) };
        let out = match ev.event_id {
            sys::EVENT_NONE => {
                if shared.quitting.load(Ordering::SeqCst) {
                    break;
                }
                None
            }
            sys::EVENT_SHUTDOWN => {
                if !shared.quitting.load(Ordering::SeqCst) {
                    // mpv quit on its own; the handle stays valid until the engine is dropped (`event_loop`).
                    sink(EngineEvent::Terminated);
                }
                break;
            }
            sys::EVENT_LOG_MESSAGE if !ev.data.is_null() => {
                // SAFETY: a log message event's data is an `mpv_event_log_message`.
                let msg = unsafe { &*(ev.data as *const sys::mpv_event_log_message) };
                // SAFETY: mpv's strings, valid until the next wait.
                let (level, prefix, text) = unsafe {
                    (
                        sys::text(msg.level),
                        sys::text(msg.prefix),
                        sys::text(msg.text),
                    )
                };
                let redactor = shared
                    .redactor
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                Some(EngineEvent::Log(LogLine {
                    level,
                    prefix: redactor.redact(&prefix),
                    text: redactor.redact(text.trim_end_matches('\n')),
                }))
            }
            sys::EVENT_COMMAND_REPLY | sys::EVENT_SET_PROPERTY_REPLY => {
                MpvError::from_status(ev.error).map(|error| EngineEvent::CommandFailed {
                    what: Request::from_reply(ev.reply_userdata),
                    error,
                })
            }
            sys::EVENT_START_FILE => Some(EngineEvent::StartFile),
            sys::EVENT_FILE_LOADED => Some(EngineEvent::FileLoaded),
            sys::EVENT_PLAYBACK_RESTART => Some(EngineEvent::PlaybackRestart),
            sys::EVENT_SEEK => Some(EngineEvent::Seek),
            sys::EVENT_IDLE => Some(EngineEvent::Idle),
            sys::EVENT_QUEUE_OVERFLOW => Some(EngineEvent::Overflow),
            sys::EVENT_END_FILE if !ev.data.is_null() => {
                // SAFETY: an end-file event's data is an `mpv_event_end_file`.
                let end = unsafe { &*(ev.data as *const sys::mpv_event_end_file) };
                Some(EngineEvent::EndFile {
                    reason: EndReason::from_raw(end.reason),
                    error: MpvError::from_status(end.error),
                })
            }
            sys::EVENT_PROPERTY_CHANGE if !ev.data.is_null() => {
                // SAFETY: a property change's data is an `mpv_event_property`.
                let prop = unsafe { &*(ev.data as *const sys::mpv_event_property) };
                let value = if prop.format == sys::FORMAT_NODE && !prop.data.is_null() {
                    // SAFETY: observed with `MPV_FORMAT_NODE`, so the data is an `mpv_node*` valid until the next wait.
                    unsafe { sys::read_node(&*(prop.data as *const sys::mpv_node)) }
                } else {
                    // `MPV_FORMAT_NONE`: the property is unavailable.
                    Node::None
                };
                match event::observed(ev.reply_userdata, &value) {
                    Observed::Change(change) => Some(EngineEvent::Property(change)),
                    // Once quitting, mpv is tearing its VO down: no new reference.
                    Observed::SwapChain(ptr)
                        if composition
                            && ptr != 0
                            && ptr != swap_chain_ptr
                            && !shared.quitting.load(Ordering::SeqCst) =>
                    {
                        swap_chain_ptr = ptr;
                        generation += 1;
                        // SAFETY: mpv reported this address as its swap chain (`display-swapchain`, an `IDXGISwapChain*`), and the
                        // reference is taken before this thread waits again. Not guaranteed: mpv reads the value on its own thread
                        // and its VO thread may release that swap chain (a VO recreation) between that read and this `AddRef`;
                        // mpv offers no way to hold it across the notice (docs/notes/mpv-engine.md, "Not verified").
                        Some(EngineEvent::SwapChain(unsafe {
                            SwapChain::add_ref(ptr as usize as *mut c_void, generation)
                        }))
                    }
                    Observed::SwapChain(ptr) => {
                        if ptr == 0 {
                            swap_chain_ptr = 0;
                        }
                        None
                    }
                    Observed::Ignore => None,
                }
            }
            _ => None,
        };
        if let Some(event) = out {
            sink(event);
        }
    }
}
