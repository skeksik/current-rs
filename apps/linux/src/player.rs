//! The player page: mpv draws into a `GtkGLArea` through its OpenGL render API, with GTK controls over the video in a `GtkOverlay`.
//!
//! Threads: mpv calls its wakeup and update callbacks on its own threads; they only notify a `tokio::sync::Notify`. Two futures on
//! the GLib main context await those and do the work there: draining mpv's events into the controls, and queueing a GLArea render.
//! Nothing on the main loop waits for mpv.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tokio::sync::Notify;

use crate::mpv::{self, Event, Mpv, RenderContext};

pub struct Player {
    pub widget: gtk::Overlay,
    area: gtk::GLArea,
    mpv: RefCell<Option<Mpv>>,
    render: RefCell<Option<RenderContext>>,
    render_update: Arc<Notify>,
    frames: Cell<u64>,
    position: Cell<f64>,
    playing: Cell<bool>,
    loaded: Cell<bool>,
    controls: gtk::Revealer,
    info: gtk::Label,
    play: gtk::Button,
    seek: gtk::Scale,
    time: gtk::Label,
    length: gtk::Label,
    hide_generation: Cell<u64>,
    error: RefCell<Option<String>>,
    /// What the GLArea's context is, for the info line: API and `GL_RENDERER`.
    gl: RefCell<String>,
    /// Whether to switch mpv to 8-bit intermediate framebuffers on a software rasteriser (see `connect_gl`).
    software_fallback: bool,
}

impl Player {
    pub fn new(options: &[(&str, &str)]) -> Rc<Self> {
        let software_fallback = !options.iter().any(|(name, _)| *name == "fbo-format");
        let area = gtk::GLArea::builder()
            .hexpand(true)
            .vexpand(true)
            .auto_render(false)
            .build();
        // GTK picks the API (GTK 4.14 prefers OpenGL ES); CURRENT_SPIKE_GL=gl asks for desktop GL instead, for comparison.
        if std::env::var_os("CURRENT_SPIKE_GL").is_some_and(|api| api == "gl") {
            area.set_allowed_apis(gtk::gdk::GLAPI::GL);
        }
        // The video's surroundings stay black, as in other players, whatever the style.
        area.add_css_class("player-video");

        let play = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text("Play")
            .build();
        play.add_css_class("flat");
        let time = gtk::Label::builder().label("0:00").width_chars(5).build();
        time.add_css_class("numeric");
        let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
        seek.set_hexpand(true);
        seek.set_draw_value(false);
        seek.update_property(&[gtk::accessible::Property::Label("Position")]);
        let length = gtk::Label::builder().label("0:00").width_chars(5).build();
        length.add_css_class("numeric");
        let fullscreen = gtk::Button::builder()
            .icon_name("view-fullscreen-symbolic")
            .tooltip_text("Full Screen")
            .action_name("win.toggle-fullscreen")
            .build();
        fullscreen.add_css_class("flat");

        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(12)
            .build();
        bar.add_css_class("toolbar");
        bar.add_css_class("osd");
        bar.add_css_class("player-bar");
        bar.append(&play);
        bar.append(&time);
        bar.append(&seek);
        bar.append(&length);
        bar.append(&fullscreen);
        let controls = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .valign(gtk::Align::End)
            .reveal_child(true)
            .child(&bar)
            .build();

        let info = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_start(12)
            .margin_top(12)
            .build();
        info.add_css_class("osd");
        info.add_css_class("player-info");

        let widget = gtk::Overlay::builder().child(&area).focusable(true).build();
        widget.add_overlay(&info);
        widget.add_overlay(&controls);

        let wakeup = Arc::new(Notify::new());
        let render_update = Arc::new(Notify::new());
        let (mpv, error) = match Mpv::new(options, Arc::clone(&wakeup)) {
            Ok(mpv) => (Some(mpv), None),
            Err(error) => (None, Some(error.to_string())),
        };

        let player = Rc::new(Self {
            widget,
            area,
            mpv: RefCell::new(mpv),
            render: RefCell::new(None),
            render_update,
            frames: Cell::new(0),
            position: Cell::new(0.0),
            playing: Cell::new(false),
            loaded: Cell::new(false),
            controls,
            info,
            play,
            seek,
            time,
            length,
            hide_generation: Cell::new(0),
            error: RefCell::new(error),
            gl: RefCell::new(String::new()),
            software_fallback,
        });
        player.show_info();
        player.connect_gl();
        player.connect_controls();
        player.pump_events(wakeup);
        player.pump_renders();
        player
    }

    fn connect_gl(self: &Rc<Self>) {
        let this = Rc::downgrade(self);
        self.area.connect_realize(move |area| {
            let Some(this) = this.upgrade() else { return };
            area.make_current();
            if let Some(error) = area.error() {
                this.fail(format!("GL context: {error}"));
                return;
            }
            let renderer = mpv::gl_renderer().unwrap_or_default();
            let api = match area.context().map(|context| context.api()) {
                Some(api) if api == gtk::gdk::GLAPI::GLES => "GLES",
                Some(_) => "GL",
                None => "?",
            };
            *this.gl.borrow_mut() = format!("{api} on {renderer}");
            let created = match this.mpv.borrow().as_ref() {
                Some(mpv) => {
                    // On Mesa's software rasterisers, mpv's default 16-bit float intermediate framebuffers (rgba16f) left the video
                    // black in about half of all runs, decided at start (docs/notes/linux-gtk.md). 8-bit ones were reliable.
                    let software = ["llvmpipe", "softpipe", "swrast"]
                        .iter()
                        .any(|name| renderer.contains(name));
                    if software && this.software_fallback {
                        let _ = mpv.set_property("fbo-format", "rgba8");
                    }
                    RenderContext::new(mpv, Arc::clone(&this.render_update))
                }
                None => return,
            };
            match created {
                Ok(render) => *this.render.borrow_mut() = Some(render),
                Err(error) => this.fail(error.to_string()),
            }
        });
        let this = Rc::downgrade(self);
        self.area.connect_unrealize(move |area| {
            let Some(this) = this.upgrade() else { return };
            // mpv frees its GL objects in the render context's free, which needs this context current.
            area.make_current();
            this.render.borrow_mut().take();
        });
        let this = Rc::downgrade(self);
        self.area.connect_render(move |area, _context| {
            let Some(this) = this.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if let Some(render) = this.render.borrow().as_ref() {
                render.frame_ready();
                let scale = area.scale_factor();
                match render.render(area.width() * scale, area.height() * scale) {
                    Ok(()) => this.frames.set(this.frames.get() + 1),
                    Err(error) => eprintln!("mpv render: {error}"),
                }
            }
            glib::Propagation::Stop
        });
    }

    fn connect_controls(self: &Rc<Self>) {
        let this = Rc::downgrade(self);
        self.play.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.toggle_pause();
            }
        });
        let this = Rc::downgrade(self);
        // change-value fires for the user's changes only, so the position updates from mpv never seek.
        self.seek.connect_change_value(move |_, _, value| {
            if let Some(this) = this.upgrade() {
                this.command(&["seek", &format!("{value:.3}"), "absolute"]);
            }
            glib::Propagation::Proceed
        });

        let keys = gtk::EventControllerKey::new();
        let this = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(this) = this.upgrade() else {
                return glib::Propagation::Proceed;
            };
            this.show_controls();
            match key {
                gtk::gdk::Key::space | gtk::gdk::Key::k => this.toggle_pause(),
                gtk::gdk::Key::Left => this.command(&["seek", "-5", "relative"]),
                gtk::gdk::Key::Right => this.command(&["seek", "5", "relative"]),
                gtk::gdk::Key::f => {
                    let _ = this.widget.activate_action("win.toggle-fullscreen", None);
                }
                gtk::gdk::Key::Escape => {
                    let _ = this.widget.activate_action("win.leave-fullscreen", None);
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.widget.add_controller(keys);

        let clicks = gtk::GestureClick::new();
        let this = Rc::downgrade(self);
        clicks.connect_pressed(move |_, presses, _, _| {
            let Some(this) = this.upgrade() else { return };
            this.widget.grab_focus();
            if presses == 2 {
                let _ = this.widget.activate_action("win.toggle-fullscreen", None);
            }
        });
        self.area.add_controller(clicks);

        let motion = gtk::EventControllerMotion::new();
        let this = Rc::downgrade(self);
        motion.connect_motion(move |_, _, _| {
            if let Some(this) = this.upgrade() {
                this.show_controls();
            }
        });
        self.widget.add_controller(motion);
    }

    /// Shows the controls and the pointer, and hides both again after a few idle seconds while playing.
    fn show_controls(self: &Rc<Self>) {
        self.controls.set_reveal_child(true);
        self.info.set_visible(true);
        self.widget.set_cursor(None);
        let generation = self.hide_generation.get() + 1;
        self.hide_generation.set(generation);
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(3), move || {
            let Some(this) = this.upgrade() else { return };
            if this.hide_generation.get() == generation
                && this.playing.get()
                && !this.controls_hovered()
            {
                this.controls.set_reveal_child(false);
                this.info.set_visible(false);
                this.widget.set_cursor_from_name(Some("none"));
            }
        });
    }

    fn controls_hovered(&self) -> bool {
        self.controls
            .child()
            .is_some_and(|bar| bar.state_flags().contains(gtk::StateFlags::PRELIGHT))
    }

    fn pump_events(self: &Rc<Self>, wakeup: Arc<Notify>) {
        let this = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            loop {
                wakeup.notified().await;
                let Some(this) = this.upgrade() else { break };
                // Collect first: handling an event may call into mpv, which must not happen while it is borrowed here.
                let events: Vec<Event> = match this.mpv.borrow().as_ref() {
                    Some(mpv) => std::iter::from_fn(|| mpv.next_event()).collect(),
                    None => break,
                };
                for event in events {
                    this.handle(event);
                }
            }
        });
    }

    fn pump_renders(self: &Rc<Self>) {
        let this = Rc::downgrade(self);
        let update = Arc::clone(&self.render_update);
        glib::spawn_future_local(async move {
            loop {
                update.notified().await;
                let Some(this) = this.upgrade() else { break };
                this.area.queue_render();
            }
        });
    }

    fn handle(self: &Rc<Self>, event: Event) {
        match event {
            Event::Double("time-pos", position) => {
                self.position.set(position);
                self.seek.set_value(position);
                self.time.set_label(&format_time(position));
            }
            Event::Double("duration", duration) => {
                self.seek.set_range(0.0, duration.max(0.001));
                self.length.set_label(&format_time(duration));
            }
            Event::Flag("pause", paused) => {
                self.playing.set(!paused);
                let (icon, tooltip) = if paused {
                    ("media-playback-start-symbolic", "Play")
                } else {
                    ("media-playback-pause-symbolic", "Pause")
                };
                self.play.set_icon_name(icon);
                self.play.set_tooltip_text(Some(tooltip));
                self.show_controls();
            }
            Event::Unset("time-pos") => {
                self.position.set(0.0);
                self.seek.set_value(0.0);
                self.time.set_label("0:00");
            }
            Event::Unset("duration") => self.length.set_label("0:00"),
            Event::FileLoaded | Event::PlaybackRestart => self.show_info(),
            Event::Shutdown => eprintln!("mpv shut down"),
            _ => {}
        }
    }

    /// What is decoding and how, for the hands-on checks: nothing from the file system.
    fn show_info(&self) {
        let text = if let Some(error) = self.error.borrow().as_ref() {
            format!("mpv: {error}")
        } else if let Some(mpv) = self.mpv.borrow().as_ref() {
            let get = |name| mpv.property(name).unwrap_or_else(|| "-".into());
            format!(
                "mpv {} · OpenGL render API, {} · fbo-format {}\n{}x{} {} · hwdec {}",
                get("mpv-version").trim_start_matches("mpv "),
                self.gl.borrow(),
                get("fbo-format"),
                get("video-params/w"),
                get("video-params/h"),
                get("video-format"),
                get("hwdec-current"),
            )
        } else {
            String::new()
        };
        self.info.set_label(&text);
    }

    fn fail(&self, error: String) {
        eprintln!("player: {error}");
        *self.error.borrow_mut() = Some(error);
        self.show_info();
    }

    fn command(&self, args: &[&str]) {
        if let Some(mpv) = self.mpv.borrow().as_ref()
            && let Err(error) = mpv.command(args)
        {
            eprintln!("mpv {}: {error}", args.join(" "));
        }
    }

    fn toggle_pause(&self) {
        self.command(&["cycle", "pause"]);
    }

    /// Loads the file once; later calls do nothing.
    pub fn load_once(&self, path: &str) {
        if !self.loaded.replace(true) {
            self.command(&["loadfile", path]);
        }
    }

    pub fn set_pause(&self, paused: bool) {
        if let Some(mpv) = self.mpv.borrow().as_ref() {
            let _ = mpv.set_property("pause", if paused { "yes" } else { "no" });
        }
    }

    pub fn frames(&self) -> u64 {
        self.frames.get()
    }

    pub fn position(&self) -> f64 {
        self.position.get()
    }

    /// Frees the render context (with its GL context current), then mpv. Call once, before the window goes away.
    pub fn shutdown(&self) {
        if self.area.is_realized() {
            self.area.make_current();
        }
        self.render.borrow_mut().take();
        self.mpv.borrow_mut().take();
    }
}

fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::format_time;

    #[test]
    fn times_read_like_a_player() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(65.9), "1:05");
        assert_eq!(format_time(3725.0), "1:02:05");
    }
}
