//! The stand-in as a process, for the C# and Swift tests. It prints `listening <address>` on its first line, then reads commands
//! from stdin, one per line:
//! - `revoke`: end every session;
//! - `quick-connect off`, `quick-connect on`, `quick-connect hold` (requests stay unapproved; `on` approves them again);
//! - `hold <path>`, `release <path>`: requests to the path wait, unanswered, until released;
//! - `count <path>`: prints `count <n>`, how many requests to the path arrived;
//! - `fail <path or page>`, `unfail <path or page>`: requests to the path, or to a page of the generated library (`/Items@<start>`,
//!   `paging::page_key`), answer 500 until unfailed;
//! - `cancelled <path or page>`: prints `cancelled <n>`, how many held requests to it their client gave up;
//! - `gate off`, `gate basic`, `gate portal`, `gate header`: the gateway in front of it (ADR 0017; credentials in `lib.rs`);
//! - `gate-cookie <value>`: the portal takes this cookie value from now on;
//! - `library add <id> <collection type> <name>`, `library remove <id>` (items under it answer 401, as for a library the user
//!   cannot see), `library forget <id>` (items under it answer 400, as for an unknown parent), `library rename <id> <name>`,
//!   `library order <id>,<id>,...`: the user's libraries.
//!
//! It exits when stdin closes, so a test that dies cannot leave it running.

use std::io::{BufRead, Write};

use jellyfin_stand_in::{Gate, StandIn};

fn main() -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?;
    let stand_in = runtime.block_on(StandIn::start())?;
    let controls = stand_in.controls();
    // Serve on a thread of its own while this one reads stdin.
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = std::thread::spawn(move || {
        runtime.block_on(async move {
            let _stand_in = stand_in;
            let _ = stopped.await;
        });
    });

    let mut out = std::io::stdout().lock();
    writeln!(out, "listening {}", controls.address())?;
    out.flush()?;
    drop(out);

    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let line = line.trim();
        match line {
            "revoke" => controls.revoke_all(),
            "quick-connect off" => controls.set_quick_connect(false),
            "quick-connect on" => {
                controls.set_quick_connect(true);
                controls.set_quick_connect_pending(false);
            }
            "quick-connect hold" => controls.set_quick_connect_pending(true),
            "gate off" => controls.set_gate(Gate::Off),
            "gate basic" => controls.set_gate(Gate::Basic),
            "gate portal" => controls.set_gate(Gate::Portal),
            "gate header" => controls.set_gate(Gate::Header),
            _ => {
                if let Some(path) = line.strip_prefix("hold ") {
                    controls.hold(path);
                } else if let Some(path) = line.strip_prefix("release ") {
                    controls.release(path);
                } else if let Some(path) = line.strip_prefix("fail ") {
                    controls.fail(path);
                } else if let Some(path) = line.strip_prefix("unfail ") {
                    controls.unfail(path);
                } else if let Some(path) = line.strip_prefix("cancelled ") {
                    let mut out = std::io::stdout().lock();
                    writeln!(out, "cancelled {}", controls.cancelled(path))?;
                    out.flush()?;
                } else if let Some(command) = line.strip_prefix("library ") {
                    library(&controls, command);
                } else if let Some(value) = line.strip_prefix("gate-cookie ") {
                    controls.set_gate_cookie(value);
                } else if let Some(path) = line.strip_prefix("count ") {
                    let mut out = std::io::stdout().lock();
                    writeln!(out, "count {}", controls.count(path))?;
                    out.flush()?;
                }
            }
        }
    }
    let _ = stop.send(());
    let _ = server.join();
    Ok(())
}

/// A `library ...` command, without its first word. A malformed one does nothing.
fn library(controls: &jellyfin_stand_in::Controls, command: &str) {
    let mut words = command.splitn(3, ' ');
    match (words.next(), words.next(), words.next()) {
        (Some("add"), Some(id), Some(rest)) => {
            if let Some((collection, name)) = rest.split_once(' ') {
                controls.add_library(id, collection, name);
            }
        }
        (Some("remove"), Some(id), None) => controls.remove_library(id),
        (Some("forget"), Some(id), None) => controls.forget_library(id),
        (Some("rename"), Some(id), Some(name)) => controls.rename_library(id, name),
        (Some("order"), Some(ids), None) => {
            controls.order_libraries(&ids.split(',').collect::<Vec<_>>());
        }
        _ => {}
    }
}
