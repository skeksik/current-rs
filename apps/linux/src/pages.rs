//! Home and Settings, mirroring apps/windows' pages with libadwaita's own widgets.

use std::rc::Rc;

use adw::prelude::*;
use current_model::{Capabilities, Reachability};

use crate::core::{Core, watch_on_main};

/// The state before any server exists, as on Windows.
pub fn home(core: &Rc<Core>) -> gtk::Widget {
    let page = adw::StatusPage::builder()
        .icon_name("network-server-symbolic")
        .title("No servers yet")
        .description("Setting up a server is not available in this version.")
        .vexpand(true)
        .build();
    let status = page.clone();
    // Dropping a glib JoinHandle detaches the task: the subscription lives as long as the app, which outlives no page.
    let _subscription = watch_on_main(
        core.app.capabilities(),
        move |capabilities: Capabilities| {
            let count = capabilities.servers.len();
            if count == 0 {
                status.set_title("No servers yet");
                status.set_description(Some(
                    "Setting up a server is not available in this version.",
                ));
            } else {
                status.set_title("Servers");
                status.set_description(Some(&format!(
                    "{count} server(s) from the core's capabilities state."
                )));
            }
        },
    );
    page.upcast()
}

pub fn settings(core: &Rc<Core>, window: &adw::ApplicationWindow) -> gtk::Widget {
    let page = adw::PreferencesPage::new();

    let about = adw::PreferencesGroup::builder().title("About").build();
    let info = core.app.info();
    about.add(
        &adw::ActionRow::builder()
            .title("Current")
            .subtitle(format!("Version {}\n© 2026 Skeksik", info.version))
            .build(),
    );
    page.add(&about);

    let servers = adw::PreferencesGroup::builder()
        .title("Servers")
        .description("Live from the core's capabilities state.")
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.add_css_class("boxed-list");
    list.set_placeholder(Some(
        &gtk::Label::builder()
            .label("No servers are set up.")
            .css_classes(["dim-label"])
            .margin_top(12)
            .margin_bottom(12)
            .build(),
    ));
    servers.add(&list);
    let _subscription = watch_on_main(
        core.app.capabilities(),
        move |capabilities: Capabilities| {
            list.remove_all();
            for server in &capabilities.servers {
                let reachability = match server.reachability {
                    Reachability::Unknown => "not checked",
                    Reachability::Reachable => "reachable",
                    Reachability::Unreachable => "unreachable",
                };
                let version = server.version.as_deref().unwrap_or("unknown version");
                list.append(
                    &adw::ActionRow::builder()
                        .title(server.server.service.as_str())
                        .subtitle(format!(
                            "Server {} · {version} · {reachability}",
                            server.server.key.as_str()
                        ))
                        .build(),
                );
            }
        },
    );
    page.add(&servers);

    let diagnostics = adw::PreferencesGroup::builder()
        .title("Diagnostics")
        .build();
    let copy = gtk::Button::builder()
        .label("Copy")
        .valign(gtk::Align::Center)
        .build();
    let diagnostics_core = Rc::clone(core);
    let toast_target = window.clone();
    copy.connect_clicked(move |button| {
        button
            .clipboard()
            .set_text(&diagnostics_core.app.diagnostics_text());
        if let Some(overlay) = toast_target.content().and_downcast::<adw::ToastOverlay>() {
            overlay.add_toast(adw::Toast::new("Diagnostics copied"));
        }
    });
    let row = adw::ActionRow::builder()
        .title("Diagnostics")
        .subtitle("The request log, without secrets")
        .build();
    row.add_suffix(&copy);
    diagnostics.add(&row);
    page.add(&diagnostics);

    // Prototype only: drives the capabilities state from a core worker thread to show live updates.
    let prototype = adw::PreferencesGroup::builder()
        .title("Prototype")
        .description("Spike controls; not part of the app.")
        .build();
    let simulate = gtk::Button::builder()
        .label("Add")
        .valign(gtk::Align::Center)
        .build();
    let clear = gtk::Button::builder()
        .label("Clear")
        .valign(gtk::Align::Center)
        .build();
    let simulate_core = Rc::clone(core);
    simulate.connect_clicked(move |_| simulate_core.simulate_server());
    let clear_core = Rc::clone(core);
    clear.connect_clicked(move |_| clear_core.clear_servers());
    let row = adw::ActionRow::builder()
        .title("Simulated server")
        .subtitle("Set by a core worker thread after 300 ms")
        .build();
    row.add_suffix(&simulate);
    row.add_suffix(&clear);
    prototype.add(&row);
    page.add(&prototype);

    page.upcast()
}
