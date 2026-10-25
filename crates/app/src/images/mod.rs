//! Images (ADR 0018, section 4): the core fetches every image through the server's session, so the address in use and its gateway
//! credentials apply (ADR 0014, ADR 0017), and hands the UI the encoded bytes. No path and no address crosses the bridge.
//!
//! A request is a box size in physical pixels, rounded to a width bucket ([`bucket`]). Memory first (64 MB of encoded bytes,
//! least recently used out), then the disk cache (500 MB, per server, file names only hashes), then the server, at most six
//! fetches per server, the newest first. Identical requests in flight share one fetch, which stops when every caller has dropped
//! its future. A server's images are deleted when it is signed out of or removed, and at start for servers no longer saved or
//! signed out. BlurHash placeholders are decoded in the core to a small BMP.

mod blurhash;
mod bucket;
mod disk;
mod fnv;
mod limiter;
mod memory;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use current_jellyfin::{AuthError, ImageRequest};
use current_model::{ImageRef, SavedServer, ServerRef};
use tokio::sync::watch;
use tokio::task::AbortHandle;

pub use bucket::{WIDTH_BUCKETS, bucket};

use crate::session::Shared;
use disk::{DISK_LIMIT, DiskCache, FileId};
use fnv::Fnv128;
use limiter::Limiter;
use memory::{MEMORY_LIMIT, MemoryCache};

/// An image the UI asks for, at the size of the box it fills, in physical pixels (view pixels times the display's scale).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImageLoad {
    pub source: ImageSource,
    pub width: u32,
    pub height: u32,
}

/// Where an image comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageSource {
    /// A server's image of an item, as the item's artwork names it.
    Server(ImageRef),
    /// A BlurHash placeholder, decoded to a BMP at most 32 pixels on its long side, at the box's ratio.
    BlurHash(String),
}

/// Why an image could not be loaded. Neither Display nor Debug holds an address, a path or an item's name.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    /// The server's image could not be fetched; not signed in to it is `Fetch(Request(NotSignedIn))`.
    #[error(transparent)]
    Fetch(#[from] current_jellyfin::ImageError),
    /// The BlurHash is not valid.
    #[error("the placeholder is not valid")]
    InvalidBlurHash,
    /// The box has no area.
    #[error("the image's box is empty")]
    EmptyBox,
    /// The fetch ended without an answer (it panicked).
    #[error("the image could not be loaded")]
    Stopped,
}

type Outcome = Result<Arc<[u8]>, ImageError>;

/// One fetch shared by every caller waiting for the same image. Dropped with its last caller, which stops the fetch.
struct Flight {
    id: u64,
    outcome: watch::Receiver<Option<Outcome>>,
    task: AbortHandle,
}

impl Drop for Flight {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Inner {
    memory: Mutex<MemoryCache<ImageRequest>>,
    disk: Option<DiskCache>,
    flights: Mutex<HashMap<ImageRequest, Weak<Flight>>>,
    next_flight: AtomicU64,
    limiters: Mutex<HashMap<ServerRef, Arc<Limiter>>>,
    /// Bumped when a server's images are deleted: a fetch that began before does not store what it got.
    epochs: Mutex<HashMap<ServerRef, u64>>,
}

/// The image cache of an [`App`](crate::App). Clones share it.
#[derive(Clone)]
pub(crate) struct ImageCache {
    inner: Arc<Inner>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every value behind these locks is changed in one step, so a poisoned lock is still consistent.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A server's folder name: the hash of its key, never its name or address.
fn folder(server: &ServerRef) -> String {
    Fnv128::new()
        .field(server.service.as_str().as_bytes())
        .field(server.key.as_str().as_bytes())
        .hex()
}

/// An image's place on disk: its server's folder, and the hash of everything that makes it this image at this size.
fn file_id(request: &ImageRequest) -> FileId {
    let server = &request.item.server;
    let name = Fnv128::new()
        .field(server.service.as_str().as_bytes())
        .field(server.key.as_str().as_bytes())
        .field(request.item.id.as_str().as_bytes())
        .field(request.kind.as_str().as_bytes())
        .field(&request.index.to_le_bytes())
        .field(request.tag.as_bytes())
        .field(&request.width.to_le_bytes())
        .field(&request.height.to_le_bytes())
        .hex();
    FileId {
        folder: folder(server),
        name,
    }
}

impl Inner {
    fn epoch(&self, server: &ServerRef) -> u64 {
        lock(&self.epochs).get(server).copied().unwrap_or(0)
    }

    /// Keeps `bytes` in memory unless the server's images were deleted since `epoch`.
    fn remember(&self, request: &ImageRequest, epoch: u64, bytes: &Arc<[u8]>) {
        let epochs = lock(&self.epochs);
        if epochs.get(&request.item.server).copied().unwrap_or(0) == epoch {
            lock(&self.memory).insert(request.clone(), bytes.clone());
        }
    }

    fn limiter(&self, server: &ServerRef) -> Arc<Limiter> {
        lock(&self.limiters)
            .entry(server.clone())
            .or_default()
            .clone()
    }

    /// Disk, then the server. Runs in its own task, aborted when every caller has gone.
    async fn fetch(
        self: Arc<Self>,
        shared: Arc<Shared>,
        request: ImageRequest,
        epoch: u64,
    ) -> Outcome {
        if self.disk.is_some() {
            let inner = self.clone();
            let id = file_id(&request);
            let found = tokio::task::spawn_blocking(move || inner.disk.as_ref()?.read(&id))
                .await
                .ok()
                .flatten();
            if let Some(bytes) = found {
                let bytes: Arc<[u8]> = bytes.into();
                self.remember(&request, epoch, &bytes);
                return Ok(bytes);
            }
        }
        let client =
            shared
                .session(&request.item.server)
                .ok_or(current_jellyfin::ImageError::Request(
                    AuthError::NotSignedIn,
                ))?;
        let permit = self.limiter(&request.item.server).acquire().await;
        let bytes: Arc<[u8]> = client.image(&request).await?.into();
        drop(permit);
        self.remember(&request, epoch, &bytes);
        if self.disk.is_some() {
            let inner = self.clone();
            let stored = bytes.clone();
            // Not awaited: the callers have their bytes, and the write completes even if this task is aborted.
            tokio::task::spawn_blocking(move || {
                if let Some(disk) = &inner.disk {
                    let server = request.item.server.clone();
                    disk.write(&file_id(&request), &stored, || {
                        inner.epoch(&server) == epoch
                    });
                }
            });
        }
        Ok(bytes)
    }
}

impl ImageCache {
    /// A cache in memory only, for a core that stores nothing ([`App::new`](crate::App::new)).
    pub(crate) fn in_memory() -> Self {
        Self::with(None)
    }

    /// A cache with its disk part in `<cache_dir>/images`, keeping only the folders of `servers` that have a signed-in user (ADR
    /// 0018: images are deleted on sign-out and removal, so a folder of any other server is a leftover). Blocking file I/O. A
    /// folder that cannot be used leaves the cache in memory only: images are then fetched again at each start.
    pub(crate) fn open(cache_dir: &std::path::Path, servers: &[SavedServer]) -> Self {
        let keep: HashSet<String> = servers
            .iter()
            .filter(|saved| saved.active_user.is_some())
            .map(|saved| folder(&saved.server))
            .collect();
        let root: PathBuf = cache_dir.join("images");
        Self::with(DiskCache::open(root, DISK_LIMIT, &keep).ok())
    }

    fn with(disk: Option<DiskCache>) -> Self {
        Self {
            inner: Arc::new(Inner {
                memory: Mutex::new(MemoryCache::new(MEMORY_LIMIT)),
                disk,
                flights: Mutex::new(HashMap::new()),
                next_flight: AtomicU64::new(1),
                limiters: Mutex::new(HashMap::new()),
                epochs: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// See [`App::load_image`](crate::App::load_image).
    pub(crate) async fn load(&self, shared: &Arc<Shared>, load: &ImageLoad) -> Outcome {
        let (width, height) = bucket(load.width, load.height).ok_or(ImageError::EmptyBox)?;
        let image = match &load.source {
            ImageSource::BlurHash(hash) => {
                return blurhash::placeholder(hash, load.width, load.height)
                    .map(Into::into)
                    .ok_or(ImageError::InvalidBlurHash);
            }
            ImageSource::Server(image) => image,
        };
        let request = ImageRequest {
            item: image.item.clone(),
            kind: image.kind.clone(),
            index: image.index,
            tag: image.tag.clone(),
            width,
            height,
        };
        if let Some(bytes) = lock(&self.inner.memory).get(&request) {
            return Ok(bytes);
        }
        let flight = self.flight(shared, request);
        let mut outcome = flight.outcome.clone();
        match outcome.wait_for(Option::is_some).await {
            Ok(done) => done.clone().unwrap_or(Err(ImageError::Stopped)),
            Err(_) => Err(ImageError::Stopped),
        }
    }

    /// The fetch in flight for `request`, or a new one.
    fn flight(&self, shared: &Arc<Shared>, request: ImageRequest) -> Arc<Flight> {
        let mut flights = lock(&self.inner.flights);
        if let Some(flight) = flights.get(&request).and_then(Weak::upgrade) {
            return flight;
        }
        flights.retain(|_, flight| flight.strong_count() > 0);
        let id = self.inner.next_flight.fetch_add(1, Ordering::Relaxed);
        let (sender, outcome) = watch::channel(None);
        let epoch = self.inner.epoch(&request.item.server);
        let inner = self.inner.clone();
        let fetching = inner.clone().fetch(shared.clone(), request.clone(), epoch);
        let key = request.clone();
        let task = tokio::spawn(async move {
            let result = fetching.await;
            // Gone from the table before anyone sees the outcome, so a later request after a failure tries again.
            let mut flights = lock(&inner.flights);
            if flights
                .get(&key)
                .and_then(Weak::upgrade)
                .is_some_and(|flight| flight.id == id)
            {
                flights.remove(&key);
            }
            drop(flights);
            sender.send_replace(Some(result));
        });
        let flight = Arc::new(Flight {
            id,
            outcome,
            task: task.abort_handle(),
        });
        flights.insert(request, Arc::downgrade(&flight));
        flight
    }

    /// The disk cache's size in bytes, every server's images together (Settings → Storage). 0 for a cache in memory only.
    pub(crate) async fn disk_size(&self) -> u64 {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.disk.as_ref().map_or(0, DiskCache::total))
            .await
            .unwrap_or(0)
    }

    /// Deletes every cached image, in memory and on disk, of every server (Settings → Storage, "Clear"). Images load again from
    /// the servers afterwards. A fetch in flight is not stopped: what it brings is stored after the clear, as any later image is.
    pub(crate) async fn clear(&self) {
        lock(&self.inner.memory).remove_where(|_| true);
        if self.inner.disk.is_some() {
            let inner = self.inner.clone();
            // Runs to the end even if the caller stops waiting.
            let _ = tokio::task::spawn_blocking(move || {
                if let Some(disk) = &inner.disk {
                    disk.clear();
                }
            })
            .await;
        }
    }

    /// Deletes everything cached for `server`, in memory and on disk. A fetch for it that is in flight stores nothing.
    pub(crate) async fn forget_server(&self, server: &ServerRef) {
        {
            let mut epochs = lock(&self.inner.epochs);
            *epochs.entry(server.clone()).or_default() += 1;
            lock(&self.inner.memory).remove_where(|request| request.item.server == *server);
        }
        if self.inner.disk.is_some() {
            let inner = self.inner.clone();
            let folder = folder(server);
            // Runs to the end even if the caller stops waiting.
            let _ = tokio::task::spawn_blocking(move || {
                if let Some(disk) = &inner.disk {
                    disk.delete_folder(&folder);
                }
            })
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use current_model::{ImageType, ItemId, ItemRef, ServerKey, ServiceType};

    use super::*;

    fn request(server: &str, tag: &str) -> ImageRequest {
        ImageRequest {
            item: ItemRef::new(
                ServerRef::new(ServiceType::JELLYFIN, ServerKey::new(server)),
                ItemId::new("0123456789abcdef0123456789abcdef"),
            ),
            kind: ImageType::PRIMARY,
            index: 0,
            tag: tag.into(),
            width: 320,
            height: 480,
        }
    }

    #[test]
    fn file_names_are_hashes_of_everything_that_tells_images_apart() {
        let a = file_id(&request("1", "t1"));
        assert_eq!(a.folder.len(), 32);
        assert_eq!(a.name.len(), 32);
        assert!(
            format!("{}{}", a.folder, a.name)
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        assert_eq!(a, file_id(&request("1", "t1")));
        assert_ne!(a.name, file_id(&request("1", "t2")).name);
        let other_server = file_id(&request("2", "t1"));
        assert_ne!(a.folder, other_server.folder);
        assert_ne!(a.name, other_server.name);
        let mut wider = request("1", "t1");
        wider.width = 480;
        assert_ne!(a.name, file_id(&wider).name);
        assert_eq!(a.folder, folder(&request("1", "x").item.server));
    }
}
