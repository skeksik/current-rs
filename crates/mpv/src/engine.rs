//! The engine: one libmpv player, its event thread and its commands (ADR 0019 "`current-mpv`", "Threads and event rate").
//!
//! - Commands go from any thread, always asynchronously (`mpv_command_node_async`, `mpv_set_property_async`), so no caller waits on
//!   mpv's lock; a failure arrives later as [`EngineEvent::CommandFailed`]. The client API is "generally fully thread-safe"
//!   (`client.h`).
//! - One OS thread per engine (`mpv-events`) blocks in `mpv_wait_event` and hands each event to the sink. It never calls a UI listener
//!   (the sink is the controller's channel). No wakeup callback is used.
//! - Dropping the engine sends `quit`; the event thread then calls `mpv_terminate_destroy` (which blocks until mpv's VO thread has
//!   ended, ADR 0007) on itself, off any core worker, and reports [`EngineEvent::Shutdown`] last. [`Engine::shutdown`] does the same and
//!   waits for it.
//! - mpv's messages arrive at `info` (or `v`) through `mpv_request_log_messages` and are redacted before they reach the sink. The engine
//!   never sets `log-file` and never reads a URL-valued property.

// Calls into libmpv need `unsafe`; this module and `sys` are the only places allowed (crate manifest).
#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use crate::caps::BuildCapabilities;
use crate::error::{EngineError, MpvError};
use crate::event::{self, EndReason, EngineEvent, LogLine, OBSERVED, Observed};
use crate::node::Node;
use crate::options::{
    self, CompositionSize, DisplayFacts, EngineConfig, FileRequest, Output, TrackChoice,
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
    Quit,
}

impl Request {
    const ALL: [Request; 11] = [
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
    quitting: AtomicBool,
}

// SAFETY: libmpv's client API may be called on one handle from any thread ("generally fully thread-safe", `client.h`), except
// `mpv_terminate_destroy`, which only the event thread calls, after the `Engine` (the only other user) is gone or has asked to quit and
// stopped issuing calls.
unsafe impl Send for Shared {}
// SAFETY: as above; the redactor is behind a mutex and the flag is atomic.
unsafe impl Sync for Shared {}

/// A libmpv player.
pub struct Engine {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    capabilities: BuildCapabilities,
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

impl Engine {
    /// Creates and initialises a player with the options at start, observes its properties, reads its capabilities and starts the event
    /// thread. Blocking (mpv starts its threads and, in composition mode, its VO): call it on a blocking thread, not a core worker.
    pub fn start(config: &EngineConfig, sink: EventSink) -> Result<Engine, EngineError> {
        // SAFETY: no arguments; returns a number.
        let version = unsafe { sys::mpv_client_api_version() } as u64;
        let (major, minor) = ((version >> 16) as u32, (version & 0xffff) as u32);
        if major != API_MAJOR {
            return Err(EngineError::ApiVersion { major, minor });
        }
        // SAFETY: no arguments; NULL on failure.
        let handle = unsafe { sys::mpv_create() };
        if handle.is_null() {
            return Err(EngineError::Create);
        }
        let shared = Arc::new(Shared {
            handle,
            redactor: Mutex::new(Redactor::new()),
            quitting: AtomicBool::new(false),
        });
        match Engine::init(&shared, config) {
            Ok(capabilities) => {
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

    fn init(shared: &Shared, config: &EngineConfig) -> Result<BuildCapabilities, EngineError> {
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
        Ok(BuildCapabilities::from_nodes(&decoders, &demuxers))
    }

    /// What this build decodes and demuxes, read after `mpv_initialize`.
    pub fn capabilities(&self) -> &BuildCapabilities {
        &self.capabilities
    }

    /// Loads a file, replacing whatever plays, with its per-file options (`start`, `aid`, `sid`, `stream-lavf-o`). Its secrets join the
    /// redaction set first, so no log line about it shows them.
    pub fn load(&self, file: &FileRequest) -> Result<(), EngineError> {
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

    fn request_quit(&self) {
        if self.shared.quitting.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.command(Request::Quit, &command(&["quit"])).is_err() {
            // The event thread checks the flag whenever it wakes.
            // SAFETY: valid handle; destroyed only by the event thread, after this.
            unsafe { sys::mpv_wakeup(self.shared.handle) };
        }
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
    // SAFETY: mpv filled `raw` with a valid tree, which is copied and then freed exactly once.
    let node = unsafe { sys::read_node(&raw) };
    unsafe { sys::mpv_free_node_contents(&mut raw) };
    Ok(node)
}

fn destroy(shared: &Shared) {
    // SAFETY: called once, by whoever holds the handle last: `start` on failure (no thread exists) or the event thread at its end.
    unsafe { sys::mpv_terminate_destroy(shared.handle) };
}

fn event_loop(shared: &Shared, mut sink: EventSink, composition: bool) {
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
            sys::EVENT_SHUTDOWN => break,
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
                    Observed::SwapChain(ptr)
                        if composition && ptr != 0 && ptr != swap_chain_ptr =>
                    {
                        swap_chain_ptr = ptr;
                        generation += 1;
                        // SAFETY: mpv just reported this address as its live swap chain (`display-swapchain`, an `IDXGISwapChain*`);
                        // the reference is taken before this thread waits again.
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
    destroy(shared);
    sink(EngineEvent::Shutdown);
}
