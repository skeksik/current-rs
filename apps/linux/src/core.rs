//! The core as the Linux app uses it: directly, with no bridge (ADR 0002). The app owns one `current_app::App` and one tokio
//! runtime, as `current-ffi` does (ADR 0004); GTK widgets are only touched on the GLib main loop.
//!
//! Subscriptions follow ADR 0004's value watch: the current value first, then every change, a slow consumer skipping intermediate
//! values. tokio's `watch` channel needs no tokio runtime to be awaited, so the GTK side awaits it in a future on the GLib main
//! context: the main loop never blocks, and no extra channel or thread hop is needed.

use std::sync::Arc;
use std::time::Duration;

use current_app::App;
use current_model::{Capabilities, ServerKey, ServerRef, ServerStatus, ServiceType};
use gtk::glib;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::watch;

pub struct Core {
    pub app: Arc<App>,
    runtime: Runtime,
}

impl Core {
    pub fn start() -> std::io::Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("core-worker")
            .enable_time()
            .build()?;
        Ok(Self {
            app: Arc::new(App::new()),
            runtime,
        })
    }

    /// Simulates a server appearing after a short delay, set from a core worker thread: what server setup (M1) will do.
    pub fn simulate_server(&self) {
        let app = Arc::clone(&self.app);
        self.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let mut status =
                ServerStatus::new(ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")));
            status.version = Some("12.0.0".into());
            app.set_capabilities(Capabilities {
                servers: vec![status],
            });
        });
    }

    pub fn clear_servers(&self) {
        self.app.set_capabilities(Capabilities::default());
    }
}

/// Calls `deliver` on the main loop with the current value, then with every change, until the sender goes away or the returned
/// handle is aborted.
pub fn watch_on_main<T, F>(mut rx: watch::Receiver<T>, mut deliver: F) -> glib::JoinHandle<()>
where
    T: Clone + 'static,
    F: FnMut(T) + 'static,
{
    glib::spawn_future_local(async move {
        loop {
            // Clone first so `deliver` never runs while the channel's lock is held.
            let value = rx.borrow_and_update().clone();
            deliver(value);
            if rx.changed().await.is_err() {
                break;
            }
        }
    })
}
