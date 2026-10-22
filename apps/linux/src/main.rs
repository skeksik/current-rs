//! D8 spike: a GTK4 + libadwaita window on the core, with libmpv in a GtkGLArea. Throwaway; see docs/notes/linux-gtk.md.
//!
//! Usage: `current-linux-spike [--clip <file>] [--no-audio] [--mpv <name>=<value>]... [--screenshot <dir>]`.
//! Without `--clip`, it plays `test-clip.mkv` from this crate's folder (`make-test-clip.sh` writes it). `--mpv` passes an option to
//! mpv. `--screenshot` walks through the pages, saves PNGs into `<dir>` and quits.

mod core;
mod mpv;
mod pages;
mod player;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::core::Core;
use crate::player::Player;

const APP_ID: &str = "com.example.CurrentSpike";

const CSS: &str = "
.player-video { background-color: black; }
.player-info { padding: 4px 8px; border-radius: 6px; font-size: smaller; }
";

struct Args {
    clip: String,
    no_audio: bool,
    mpv_options: Vec<(String, String)>,
    screenshot: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        clip: concat!(env!("CARGO_MANIFEST_DIR"), "/test-clip.mkv").to_owned(),
        no_audio: false,
        mpv_options: Vec::new(),
        screenshot: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--clip" => args.clip = iter.next().ok_or("--clip needs a file")?,
            "--no-audio" => args.no_audio = true,
            "--mpv" => {
                let option = iter.next().ok_or("--mpv needs name=value")?;
                let (name, value) = option.split_once('=').ok_or("--mpv needs name=value")?;
                args.mpv_options.push((name.to_owned(), value.to_owned()));
            }
            "--screenshot" => {
                args.screenshot = Some(iter.next().ok_or("--screenshot needs a folder")?.into())
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn main() -> glib::ExitCode {
    let args = match parse_args() {
        Ok(args) => Rc::new(args),
        Err(error) => {
            eprintln!("{error}");
            return glib::ExitCode::FAILURE;
        }
    };
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.set_accels_for_action("win.toggle-fullscreen", &["F11"]);
    app.set_accels_for_action("win.preferences", &["<Control>comma"]);
    app.set_accels_for_action("window.close", &["<Control>w"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    let quit = gio::ActionEntry::builder("quit")
        .activate(|app: &adw::Application, _, _| app.quit())
        .build();
    app.add_action_entries([quit]);

    app.connect_startup(|_| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
    app.connect_activate(move |app| {
        if let Err(error) = build(app, &args) {
            eprintln!("{error}");
            app.quit();
        }
    });
    // Our arguments are parsed above; GApplication gets none.
    app.run_with_args::<&str>(&[])
}

fn build(app: &adw::Application, args: &Rc<Args>) -> Result<(), String> {
    let core = Rc::new(Core::start().map_err(|error| format!("core runtime: {error}"))?);

    // GTK has set the user's locale by now; mpv needs the C numeric locale.
    mpv::use_c_numeric_locale();
    let mut options = vec![("hwdec", "no"), ("keep-open", "yes"), ("terminal", "no")];
    if args.no_audio {
        options.push(("ao", "null"));
    }
    for (name, value) in &args.mpv_options {
        options.push((name, value));
    }
    let player = Player::new(&options);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Current")
        .default_width(1100)
        .default_height(700)
        .width_request(360)
        .height_request(294)
        .build();

    // Sidebar: the app's sections. Settings and About live in the primary menu, as GNOME apps place them.
    let sidebar_list = gtk::ListBox::new();
    sidebar_list.add_css_class("navigation-sidebar");
    for (icon, title) in [
        ("go-home-symbolic", "Home"),
        ("media-playback-start-symbolic", "Player"),
    ] {
        let row = gtk::Box::builder().spacing(12).build();
        row.append(&gtk::Image::from_icon_name(icon));
        row.append(&gtk::Label::new(Some(title)));
        sidebar_list.append(&row);
    }
    let menu = gio::Menu::new();
    menu.append(Some("_Preferences"), Some("win.preferences"));
    menu.append(Some("_About Current"), Some("win.about"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .primary(true)
        .tooltip_text("Main Menu")
        .build();
    let sidebar_header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("Current", ""))
        .build();
    sidebar_header.pack_end(&menu_button);
    let sidebar = adw::ToolbarView::builder()
        .content(&gtk::ScrolledWindow::builder().child(&sidebar_list).build())
        .build();
    sidebar.add_top_bar(&sidebar_header);

    // Content: one page per section.
    let stack = adw::ViewStack::new();
    stack.add_named(&pages::home(&core), Some("home"));
    stack.add_named(&player.widget, Some("player"));
    let content_title = adw::WindowTitle::new("Home", "");
    let sidebar_toggle = gtk::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text("Toggle Sidebar")
        .build();
    let content_header = adw::HeaderBar::builder()
        .title_widget(&content_title)
        .build();
    content_header.pack_start(&sidebar_toggle);
    let content = adw::ToolbarView::builder().content(&stack).build();
    content.add_top_bar(&content_header);

    let split = adw::OverlaySplitView::builder()
        .sidebar(&sidebar)
        .content(&content)
        .min_sidebar_width(200.0)
        .build();
    split
        .bind_property("show-sidebar", &sidebar_toggle, "active")
        .bidirectional()
        .sync_create()
        .build();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&split));
    window.set_content(Some(&toasts));

    let narrow = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 600sp").map_err(|error| error.to_string())?,
    );
    narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(narrow);

    {
        let stack = stack.clone();
        let player = Rc::clone(&player);
        let split = split.clone();
        let args = Rc::clone(args);
        sidebar_list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let (name, title) = if row.index() == 1 {
                ("player", "Player")
            } else {
                ("home", "Home")
            };
            stack.set_visible_child_name(name);
            content_title.set_title(title);
            if name == "player" {
                player.load_once(&args.clip);
                player.widget.grab_focus();
            }
            if split.is_collapsed() {
                split.set_show_sidebar(false);
            }
        });
    }
    sidebar_list.select_row(sidebar_list.row_at_index(0).as_ref());

    add_window_actions(&window, &core, &split, &content);

    {
        let player = Rc::clone(&player);
        window.connect_close_request(move |_| {
            player.shutdown();
            glib::Propagation::Proceed
        });
    }

    window.present();

    if let Some(folder) = args.screenshot.clone() {
        glib::spawn_future_local(screenshot_tour(window, core, player, sidebar_list, folder));
    }
    Ok(())
}

fn add_window_actions(
    window: &adw::ApplicationWindow,
    core: &Rc<Core>,
    split: &adw::OverlaySplitView,
    content: &adw::ToolbarView,
) {
    // Full screen hides the sidebar and the header bar, and gives both back on leaving.
    let sidebar_before = Rc::new(Cell::new(true));
    {
        let (split, content, sidebar_before) =
            (split.clone(), content.clone(), Rc::clone(&sidebar_before));
        window.connect_fullscreened_notify(move |window| {
            if window.is_fullscreen() {
                sidebar_before.set(split.shows_sidebar());
                split.set_show_sidebar(false);
                content.set_reveal_top_bars(false);
            } else {
                split.set_show_sidebar(sidebar_before.get());
                content.set_reveal_top_bars(true);
            }
        });
    }
    let toggle = gio::ActionEntry::builder("toggle-fullscreen")
        .activate(|window: &adw::ApplicationWindow, _, _| {
            window.set_fullscreened(!window.is_fullscreen())
        })
        .build();
    let leave = gio::ActionEntry::builder("leave-fullscreen")
        .activate(|window: &adw::ApplicationWindow, _, _| window.unfullscreen())
        .build();
    let preferences_core = Rc::clone(core);
    let preferences = gio::ActionEntry::builder("preferences")
        .activate(move |window: &adw::ApplicationWindow, _, _| {
            let dialog = adw::PreferencesDialog::new();
            if let Ok(page) =
                pages::settings(&preferences_core, window).downcast::<adw::PreferencesPage>()
            {
                dialog.add(&page);
            }
            dialog.present(Some(window));
        })
        .build();
    let about_core = Rc::clone(core);
    let about = gio::ActionEntry::builder("about")
        .activate(move |window: &adw::ApplicationWindow, _, _| {
            adw::AboutDialog::builder()
                .application_name("Current")
                .application_icon("video-x-generic")
                .version(about_core.app.info().version)
                .copyright("© 2026 Skeksik")
                .build()
                .present(Some(window));
        })
        .build();
    window.add_action_entries([toggle, leave, preferences, about]);
}

/// Screenshot mode: shows each state, saves the window to `<folder>/<name>.png`, then quits. Synthetic content only.
async fn screenshot_tour(
    window: adw::ApplicationWindow,
    core: Rc<Core>,
    player: Rc<Player>,
    sidebar: gtk::ListBox,
    folder: PathBuf,
) {
    let _ = std::fs::create_dir_all(&folder);
    let save = |name: &str| match save_png(&window, &folder.join(format!("{name}.png"))) {
        Ok(()) => println!("screenshot {name}: ok"),
        Err(error) => println!("screenshot {name}: {error}"),
    };
    let wait = |ms| glib::timeout_future(Duration::from_millis(ms));

    wait(1500).await;
    save("home");

    core.simulate_server();
    let _ = WidgetExt::activate_action(&window, "win.preferences", None);
    wait(1500).await;
    save("preferences");
    if let Some(dialog) = window.visible_dialog() {
        dialog.close();
    }
    wait(500).await;
    save("home-with-server");

    sidebar.select_row(sidebar.row_at_index(1).as_ref());
    let mut waited = 0;
    while (player.frames() < 2 || player.position() < 2.0) && waited < 20_000 {
        wait(100).await;
        waited += 100;
    }
    println!(
        "player: {} frames rendered, position {:.2} s after {waited} ms",
        player.frames(),
        player.position()
    );
    player.set_pause(true);
    wait(500).await;
    save("player");

    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    wait(800).await;
    save("player-dark");

    window.set_default_size(480, 600);
    wait(1000).await;
    save("player-narrow");
    wait(2000).await;
    save("player-narrow-later");

    window.fullscreen();
    wait(1500).await;
    println!("fullscreen: {}", window.is_fullscreen());
    save("player-fullscreen");

    player.shutdown();
    window.close();
}

fn save_png(window: &adw::ApplicationWindow, path: &std::path::Path) -> Result<(), String> {
    let (width, height) = (window.width(), window.height());
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let node = snapshot.to_node().ok_or("nothing drawn")?;
    let renderer = window.renderer().ok_or("no renderer")?;
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    renderer
        .render_texture(&node, Some(&bounds))
        .save_to_png(path)
        .map_err(|error| error.to_string())
}
