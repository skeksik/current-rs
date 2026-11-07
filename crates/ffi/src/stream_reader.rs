//! The core's stream reader across the bridge (ADR 0026, "Over FFI"): a platform engine that gets a [`StreamReader`] with its load
//! ([`crate::PlatformFile::reader`]) reads the file through the core by byte ranges instead of fetching a URL. The C# adapter wraps
//! it as a `Stream` for `MediaSource.CreateFromStream`.
//!
//! Opening is async: the file's size and type are known only from the server's first answer, so [`StreamReader::open`] gives them,
//! and every later call returns the same at once. Reads and opens take a [`CancelToken`] (ADR 0004); the UI's wrapper cancels it
//! when its own task is cancelled. Nothing here carries a URL, a status or a server's text.

use std::sync::Arc;

use current_app::{PlatformReader, ReadError};
use tokio::sync::watch;

use crate::cancel::{AbortOnDrop, CancelToken};

/// Why a read or an open failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum StreamReadError {
    /// The call's token was cancelled, or the stream ended: the player stopped it, moved on to another item or route, or closed.
    #[error("the read was cancelled")]
    Cancelled,
    /// The server refused or failed; the player shows what happened.
    #[error("the stream could not be read")]
    Failed,
    /// [`StreamReader::close`] was called.
    #[error("the stream was closed")]
    Closed,
}

impl From<ReadError> for StreamReadError {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::Cancelled => Self::Cancelled,
            ReadError::Closed => Self::Closed,
            ReadError::Failed | ReadError::NotAllowed => Self::Failed,
        }
    }
}

/// The file's size in bytes and its content type, as the server named them.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StreamFileInfo {
    pub size: u64,
    pub content_type: String,
}

/// One file read through the core for the platform engine. Calls may come from any thread, reads at once.
#[derive(uniffi::Object)]
pub struct StreamReader {
    reader: Arc<PlatformReader>,
    closed: watch::Sender<bool>,
}

impl std::fmt::Debug for StreamReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamReader").finish_non_exhaustive()
    }
}

impl StreamReader {
    pub(crate) fn new(reader: Arc<PlatformReader>) -> Self {
        Self {
            reader,
            closed: watch::Sender::new(false),
        }
    }

    /// Runs `work` on the core's runtime until it ends, `token` is cancelled or the reader is closed.
    async fn run<T, F>(&self, token: Arc<CancelToken>, work: F) -> Result<T, StreamReadError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, ReadError>> + Send + 'static,
    {
        let mut closed = self.closed.subscribe();
        if *closed.borrow() {
            return Err(StreamReadError::Closed);
        }
        let runtime = crate::runtime::handle().map_err(|_| StreamReadError::Failed)?;
        let mut task = AbortOnDrop(runtime.spawn(async move {
            tokio::select! {
                biased;
                () = token.cancelled() => Err(StreamReadError::Cancelled),
                _ = closed.wait_for(|closed| *closed) => Err(StreamReadError::Closed),
                result = work => result.map_err(StreamReadError::from),
            }
        }));
        (&mut task.0)
            .await
            .unwrap_or(Err(StreamReadError::Cancelled))
    }
}

#[uniffi::export]
impl StreamReader {
    /// Opens the file on the first call, with one request for it from its first byte; later calls answer at once.
    ///
    /// # Errors
    /// [`StreamReadError::Failed`] when the server refused or failed, [`StreamReadError::Cancelled`] when `token` is cancelled or
    /// the stream ended, [`StreamReadError::Closed`] after [`StreamReader::close`].
    pub async fn open(&self, token: Arc<CancelToken>) -> Result<StreamFileInfo, StreamReadError> {
        let reader = self.reader.clone();
        let (size, content_type) = self.run(token, async move { reader.open().await }).await?;
        Ok(StreamFileInfo { size, content_type })
    }

    /// Up to `max_len` bytes at `offset` (opening the file first if needed): fewer at the end of what the core holds, none at or past
    /// the end of the file. Waits while the bytes are not fetched yet or are further ahead of playback than the core lets a player
    /// read (150 MiB, as libmpv).
    ///
    /// # Errors
    /// As [`StreamReader::open`].
    pub async fn read(
        &self,
        offset: u64,
        max_len: u32,
        token: Arc<CancelToken>,
    ) -> Result<Vec<u8>, StreamReadError> {
        let reader = self.reader.clone();
        self.run(token, async move {
            reader
                .read(offset, max_len)
                .await
                .map(|bytes| bytes.to_vec())
        })
        .await
    }

    /// The platform is done with the stream: pending and later calls end with [`StreamReadError::Closed`]. The file's requests end
    /// with the playback, not here.
    pub fn close(&self) {
        self.closed.send_replace(true);
    }
}
