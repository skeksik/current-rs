//! Keeps a freed port refused while a test relies on it.
//!
//! The tests in a binary run in parallel, and the OS hands a port freed by one test (a dropped or stopped listener) to the next
//! bind of port 0, possibly another test's stand-in, which then answers where the first test expects a refusal. So every
//! stand-in binds under [`bind`], and a test that frees a port holds [`Ports`] from before it frees it until it ends: no
//! stand-in is created meanwhile. A test creates its own stand-ins before taking [`Ports`]. A socket bound but not listening
//! would reserve a port without the lock, but connections to one hung on the macOS runner instead of being refused.

use tokio::net::TcpListener;
use tokio::sync::{Mutex, MutexGuard};

static LOCK: Mutex<()> = Mutex::const_new(());

/// Held while a test relies on a port it freed staying refused.
pub struct Ports {
    _guard: MutexGuard<'static, ()>,
}

impl Ports {
    /// Waits until no other test relies on a freed port, then blocks new stand-ins until dropped.
    pub async fn hold() -> Self {
        Self {
            _guard: LOCK.lock().await,
        }
    }

    /// A port where nothing listens, so connections are refused: a listener's port after it is dropped.
    pub async fn refused(&self) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    }
}

/// Binds a stand-in's listener on a free port of 127.0.0.1, unless a test holds [`Ports`].
pub async fn bind() -> TcpListener {
    let _lock = LOCK.lock().await;
    TcpListener::bind("127.0.0.1:0").await.unwrap()
}
