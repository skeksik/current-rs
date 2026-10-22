//! A minimal hand-written binding to the system libmpv: the client API for commands, properties and events, and the render API
//! (`render.h`, `render_gl.h`) for drawing into the GLArea's framebuffer. Only what the spike uses.
//!
//! `unsafe` is allowed here because libmpv is a C library and there is no other way to call it; this is the prototype's only FFI.
//! M2's `current-mpv` crate replaces it. Types and constants are copied from mpv's public headers as shipped with Ubuntu 24.04's
//! libmpv-dev 0.37.0 (client API 2.2).
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr::{self, NonNull};
use std::sync::Arc;

use tokio::sync::Notify;

#[repr(C)]
struct MpvHandle {
    _private: [u8; 0],
}

#[repr(C)]
struct MpvRenderContext {
    _private: [u8; 0],
}

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEventProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct MpvRenderParam {
    kind: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct MpvOpenGlInitParams {
    get_proc_address: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
struct MpvOpenGlFbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

const MPV_FORMAT_NONE: c_int = 0;
const MPV_FORMAT_FLAG: c_int = 3;
const MPV_FORMAT_DOUBLE: c_int = 5;

const MPV_EVENT_NONE: c_int = 0;
const MPV_EVENT_SHUTDOWN: c_int = 1;
const MPV_EVENT_END_FILE: c_int = 7;
const MPV_EVENT_FILE_LOADED: c_int = 8;
const MPV_EVENT_PLAYBACK_RESTART: c_int = 21;
const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;

const MPV_RENDER_PARAM_INVALID: c_int = 0;
const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const MPV_RENDER_PARAM_OPENGL_FBO: c_int = 3;
const MPV_RENDER_PARAM_FLIP_Y: c_int = 4;
const MPV_RENDER_API_TYPE_OPENGL: &CStr = c"opengl";
const MPV_RENDER_UPDATE_FRAME: u64 = 1;

const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const GL_RENDERER: u32 = 0x1F01;

#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_create() -> *mut MpvHandle;
    fn mpv_initialize(ctx: *mut MpvHandle) -> c_int;
    fn mpv_terminate_destroy(ctx: *mut MpvHandle);
    fn mpv_error_string(error: c_int) -> *const c_char;
    fn mpv_set_option_string(
        ctx: *mut MpvHandle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int;
    fn mpv_set_property_string(
        ctx: *mut MpvHandle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int;
    fn mpv_get_property_string(ctx: *mut MpvHandle, name: *const c_char) -> *mut c_char;
    fn mpv_free(data: *mut c_void);
    fn mpv_command(ctx: *mut MpvHandle, args: *mut *const c_char) -> c_int;
    fn mpv_observe_property(
        ctx: *mut MpvHandle,
        userdata: u64,
        name: *const c_char,
        format: c_int,
    ) -> c_int;
    fn mpv_wait_event(ctx: *mut MpvHandle, timeout: f64) -> *mut MpvEvent;
    fn mpv_set_wakeup_callback(
        ctx: *mut MpvHandle,
        cb: Option<unsafe extern "C" fn(*mut c_void)>,
        d: *mut c_void,
    );

    fn mpv_render_context_create(
        res: *mut *mut MpvRenderContext,
        mpv: *mut MpvHandle,
        params: *mut MpvRenderParam,
    ) -> c_int;
    fn mpv_render_context_set_update_callback(
        ctx: *mut MpvRenderContext,
        cb: Option<unsafe extern "C" fn(*mut c_void)>,
        d: *mut c_void,
    );
    fn mpv_render_context_update(ctx: *mut MpvRenderContext) -> u64;
    fn mpv_render_context_render(ctx: *mut MpvRenderContext, params: *mut MpvRenderParam) -> c_int;
    fn mpv_render_context_free(ctx: *mut MpvRenderContext);
}

// GTK 4.14 creates its GL contexts through EGL on Wayland and, by default, on X11 too (docs/notes/linux-gtk.md), so EGL's loader
// finds every GL function mpv asks for. Mesa returns core functions from it as well (EGL_KHR_get_all_proc_addresses).
#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetProcAddress(name: *const c_char) -> *mut c_void;
}

// glibc's C library; LC_NUMERIC is 1 in glibc's <locale.h>.
unsafe extern "C" {
    fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
}
const LC_NUMERIC: c_int = 1;

/// mpv's client API requires LC_NUMERIC to be "C" (client.h: "The LC_NUMERIC locale category must be set to \"C\""). GTK sets
/// the user's locale at startup, so the numeric part goes back to "C" before mpv is created.
pub fn use_c_numeric_locale() {
    // SAFETY: called on the main thread before any other thread reads the locale; the string is static.
    unsafe { setlocale(LC_NUMERIC, c"C".as_ptr()) };
}

#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn check(what: &str, code: c_int) -> Result<(), Error> {
    if code >= 0 {
        return Ok(());
    }
    // SAFETY: mpv_error_string returns a static string for any code.
    let text = unsafe { CStr::from_ptr(mpv_error_string(code)) }.to_string_lossy();
    Err(Error(format!("{what}: {text}")))
}

fn cstring(value: &str) -> Result<CString, Error> {
    CString::new(value).map_err(|_| Error(format!("NUL in {value:?}")))
}

/// What the player reads from mpv's event queue.
#[derive(Debug)]
pub enum Event {
    Double(&'static str, f64),
    Flag(&'static str, bool),
    /// The property has no value now (no file, or not known yet).
    Unset(&'static str),
    FileLoaded,
    PlaybackRestart,
    EndFile,
    Shutdown,
}

/// The properties the player observes; the index is the observation's reply id.
const OBSERVED: [(&str, c_int); 4] = [
    ("time-pos", MPV_FORMAT_DOUBLE),
    ("duration", MPV_FORMAT_DOUBLE),
    ("pause", MPV_FORMAT_FLAG),
    ("eof-reached", MPV_FORMAT_FLAG),
];

/// One mpv instance. Dropping it destroys mpv, which must happen after its render context is freed.
pub struct Mpv {
    handle: NonNull<MpvHandle>,
    wakeup: *const Notify,
}

impl Mpv {
    /// Creates mpv for the render API (`vo=libmpv`) with the given extra options, and starts observing the player's properties.
    /// `wakeup` is notified (from an mpv thread) whenever events are waiting.
    pub fn new(options: &[(&str, &str)], wakeup: Arc<Notify>) -> Result<Self, Error> {
        // SAFETY: mpv_create has no preconditions; a null result means out of memory.
        let handle = NonNull::new(unsafe { mpv_create() })
            .ok_or_else(|| Error("mpv_create failed".into()))?;
        let mpv = Self {
            handle,
            wakeup: Arc::into_raw(wakeup),
        };
        for (name, value) in [("vo", "libmpv")].iter().chain(options) {
            let (name, value) = (cstring(name)?, cstring(value)?);
            // SAFETY: valid handle and NUL-terminated strings that outlive the call.
            check("set option", unsafe {
                mpv_set_option_string(mpv.raw(), name.as_ptr(), value.as_ptr())
            })?;
        }
        // SAFETY: valid handle, before mpv_initialize as the API requires for options.
        check("initialize", unsafe { mpv_initialize(mpv.raw()) })?;
        for (id, (name, format)) in OBSERVED.iter().enumerate() {
            let name = cstring(name)?;
            // SAFETY: valid handle and string; mpv copies the name.
            check("observe", unsafe {
                mpv_observe_property(mpv.raw(), id as u64, name.as_ptr(), *format)
            })?;
        }
        // SAFETY: `wakeup` points to a Notify kept alive by our Arc until after mpv_terminate_destroy, which stops callbacks.
        unsafe {
            mpv_set_wakeup_callback(
                mpv.raw(),
                Some(notify_trampoline),
                mpv.wakeup as *mut c_void,
            )
        };
        Ok(mpv)
    }

    fn raw(&self) -> *mut MpvHandle {
        self.handle.as_ptr()
    }

    pub fn command(&self, args: &[&str]) -> Result<(), Error> {
        let owned = args
            .iter()
            .map(|arg| cstring(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let mut pointers: Vec<*const c_char> = owned.iter().map(|arg| arg.as_ptr()).collect();
        pointers.push(ptr::null());
        // SAFETY: a NULL-terminated array of valid strings that outlive the call.
        check(args.first().copied().unwrap_or("command"), unsafe {
            mpv_command(self.raw(), pointers.as_mut_ptr())
        })
    }

    pub fn set_property(&self, name: &str, value: &str) -> Result<(), Error> {
        let (name, value) = (cstring(name)?, cstring(value)?);
        // SAFETY: valid handle and strings.
        check("set property", unsafe {
            mpv_set_property_string(self.raw(), name.as_ptr(), value.as_ptr())
        })
    }

    pub fn property(&self, name: &str) -> Option<String> {
        let name = cstring(name).ok()?;
        // SAFETY: valid handle and string; the result is ours to free with mpv_free.
        let value = unsafe { mpv_get_property_string(self.raw(), name.as_ptr()) };
        if value.is_null() {
            return None;
        }
        // SAFETY: mpv returned a NUL-terminated string, freed right after copying.
        let text = unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned();
        unsafe { mpv_free(value.cast()) };
        Some(text)
    }

    /// The next waiting event, without blocking.
    pub fn next_event(&self) -> Option<Event> {
        loop {
            // SAFETY: valid handle; the event stays valid until the next mpv_wait_event call on this handle, and we copy out of it
            // before returning.
            let event = unsafe { &*mpv_wait_event(self.raw(), 0.0) };
            return Some(match event.event_id {
                MPV_EVENT_NONE => return None,
                MPV_EVENT_SHUTDOWN => Event::Shutdown,
                MPV_EVENT_FILE_LOADED => Event::FileLoaded,
                MPV_EVENT_PLAYBACK_RESTART => Event::PlaybackRestart,
                MPV_EVENT_END_FILE => Event::EndFile,
                MPV_EVENT_PROPERTY_CHANGE => {
                    let Some(&(name, _)) = OBSERVED.get(event.reply_userdata as usize) else {
                        continue;
                    };
                    // SAFETY: for MPV_EVENT_PROPERTY_CHANGE, data points to an mpv_event_property whose data matches its format.
                    let property = unsafe { &*(event.data as *const MpvEventProperty) };
                    match property.format {
                        MPV_FORMAT_DOUBLE => {
                            Event::Double(name, unsafe { *(property.data as *const f64) })
                        }
                        MPV_FORMAT_FLAG => {
                            Event::Flag(name, unsafe { *(property.data as *const c_int) } != 0)
                        }
                        MPV_FORMAT_NONE => Event::Unset(name),
                        _ => continue,
                    }
                }
                _ => continue,
            });
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // SAFETY: the handle is valid and not used again; mpv_terminate_destroy returns after mpv's threads (and so the wakeup
        // callback) are gone, after which the Notify can be released.
        unsafe {
            mpv_terminate_destroy(self.raw());
            drop(Arc::from_raw(self.wakeup));
        }
    }
}

unsafe extern "C" fn notify_trampoline(data: *mut c_void) {
    // SAFETY: `data` is the Notify registered with this callback, alive until the callback can no longer run.
    unsafe { &*(data as *const Notify) }.notify_one();
}

unsafe extern "C" fn get_proc_address(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: mpv passes a NUL-terminated function name.
    unsafe { eglGetProcAddress(name) }
}

/// `GL_RENDERER` of the current GL context, such as "llvmpipe (LLVM 20.1.2, 256 bits)". Needs a current context.
pub fn gl_renderer() -> Option<String> {
    // SAFETY: the name is NUL-terminated; glGetString has this signature: const GLubyte *glGetString(GLenum name).
    let get_string = unsafe { eglGetProcAddress(c"glGetString".as_ptr()) };
    if get_string.is_null() {
        return None;
    }
    let get_string: unsafe extern "C" fn(u32) -> *const c_char =
        unsafe { std::mem::transmute(get_string) };
    // SAFETY: GL_RENDERER is a valid name; the result is a static string or null.
    let renderer = unsafe { get_string(GL_RENDERER) };
    (!renderer.is_null()).then(|| {
        unsafe { CStr::from_ptr(renderer) }
            .to_string_lossy()
            .into_owned()
    })
}

/// mpv's OpenGL renderer. Create, render and drop it only while the GLArea's context is current; drop it before its [`Mpv`].
pub struct RenderContext {
    ctx: NonNull<MpvRenderContext>,
    update: *const Notify,
    get_framebuffer: unsafe extern "C" fn(u32, *mut c_int),
}

impl RenderContext {
    /// `update` is notified (from an mpv thread) whenever a new frame should be drawn.
    pub fn new(mpv: &Mpv, update: Arc<Notify>) -> Result<Self, Error> {
        // SAFETY: the GL context is current (the caller's contract); the name is NUL-terminated.
        let get_integer = unsafe { eglGetProcAddress(c"glGetIntegerv".as_ptr()) };
        if get_integer.is_null() {
            return Err(Error(
                "eglGetProcAddress(glGetIntegerv) returned null".into(),
            ));
        }
        // SAFETY: glGetIntegerv has this signature: void glGetIntegerv(GLenum pname, GLint *data).
        let get_framebuffer: unsafe extern "C" fn(u32, *mut c_int) =
            unsafe { std::mem::transmute(get_integer) };

        let mut init = MpvOpenGlInitParams {
            get_proc_address: Some(get_proc_address),
            get_proc_address_ctx: ptr::null_mut(),
        };
        let mut params = [
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_API_TYPE,
                data: MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut c_void,
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: (&raw mut init).cast(),
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        let mut ctx = ptr::null_mut();
        // SAFETY: valid mpv handle, a terminated parameter list whose pointers outlive the call, and a current GL context.
        check("render context", unsafe {
            mpv_render_context_create(&mut ctx, mpv.raw(), params.as_mut_ptr())
        })?;
        let ctx = NonNull::new(ctx).ok_or_else(|| Error("render context is null".into()))?;
        let update = Arc::into_raw(update);
        // SAFETY: as for the wakeup callback: the Notify outlives mpv_render_context_free, which stops callbacks.
        unsafe {
            mpv_render_context_set_update_callback(
                ctx.as_ptr(),
                Some(notify_trampoline),
                update as *mut c_void,
            )
        };
        Ok(Self {
            ctx,
            update,
            get_framebuffer,
        })
    }

    /// Whether mpv has a new frame. Call before rendering, after each update notification.
    pub fn frame_ready(&self) -> bool {
        // SAFETY: valid render context.
        unsafe { mpv_render_context_update(self.ctx.as_ptr()) & MPV_RENDER_UPDATE_FRAME != 0 }
    }

    /// Draws the current frame into the framebuffer bound now (the GLArea's), `width` x `height` device pixels.
    pub fn render(&self, width: i32, height: i32) -> Result<(), Error> {
        let mut fbo: c_int = 0;
        // SAFETY: the GLArea's context is current while its render signal runs.
        unsafe { (self.get_framebuffer)(GL_FRAMEBUFFER_BINDING, &mut fbo) };
        let mut target = MpvOpenGlFbo {
            fbo,
            w: width,
            h: height,
            internal_format: 0,
        };
        // GL's origin is bottom-left; GTK's framebuffer is drawn the other way up, so mpv flips.
        let mut flip: c_int = 1;
        let mut params = [
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_OPENGL_FBO,
                data: (&raw mut target).cast(),
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_FLIP_Y,
                data: (&raw mut flip).cast(),
            },
            MpvRenderParam {
                kind: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        // SAFETY: valid render context, a terminated parameter list, and a current GL context.
        check("render", unsafe {
            mpv_render_context_render(self.ctx.as_ptr(), params.as_mut_ptr())
        })
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        // SAFETY: valid context, not used again; after the free no update callback runs, so the Notify can go.
        unsafe {
            mpv_render_context_free(self.ctx.as_ptr());
            drop(Arc::from_raw(self.update));
        }
    }
}
