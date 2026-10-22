//! The stand-in as a process, for the C# and Swift tests. It prints `listening <address>` on its first line, then reads commands
//! from stdin, one per line: `revoke` (end every session), `quick-connect off`, `quick-connect on`. It exits when stdin closes, so
//! a test that dies cannot leave it running.

use std::io::{BufRead, Write};

use jellyfin_stand_in::StandIn;

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
        match line?.trim() {
            "revoke" => controls.revoke_all(),
            "quick-connect off" => controls.set_quick_connect(false),
            "quick-connect on" => controls.set_quick_connect(true),
            _ => {}
        }
    }
    let _ = stop.send(());
    let _ = server.join();
    Ok(())
}
