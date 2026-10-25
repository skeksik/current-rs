//! The disk cache (ADR 0018, section 4): `<cache folder>/images/<server hash>/<image hash>`, file and folder names only hashes. A
//! file is written to a temporary name and renamed, so a reader never sees half an image. The index of sizes and last-access
//! times is built from the folder at start; a hit refreshes the file's modification time, which stands for its last access, at
//! most once a minute. Beyond the limit, the least recently used files go until the total is 90% of it.
//!
//! Blocking file I/O throughout: callers run it on the blocking pool.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

/// The default limit: 500 MB (decided by the owner, 2026-10-25).
pub(crate) const DISK_LIMIT: u64 = 500 * 1000 * 1000;
/// A hit writes the file's modification time at most this often.
const REFRESH_EVERY: Duration = Duration::from_secs(60);
/// What a temporary file's name ends with; one left by a crash is deleted at start.
const TEMPORARY: &str = ".tmp";

/// Where an image lives: its server's folder and its own file name, both hashes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FileId {
    pub(crate) folder: String,
    pub(crate) name: String,
}

struct FileEntry {
    size: u64,
    /// Last access as the index knows it, with a sequence number that orders accesses within one clock tick.
    accessed: (SystemTime, u64),
    /// The file's modification time as last written or read.
    stamped: SystemTime,
}

#[derive(Default)]
struct Index {
    files: HashMap<FileId, FileEntry>,
    total: u64,
    sequence: u64,
}

impl Index {
    fn next(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    fn remove(&mut self, id: &FileId) {
        if let Some(entry) = self.files.remove(id) {
            self.total -= entry.size;
        }
    }
}

pub(crate) struct DiskCache {
    root: PathBuf,
    limit: u64,
    /// Held for index changes and for the renames and deletions that must agree with them; never across a whole file's read or
    /// write.
    index: Mutex<Index>,
}

impl std::fmt::Debug for DiskCache {
    /// Leaves out the folder: its path holds the user's account name.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskCache")
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

fn is_hash(name: &str) -> bool {
    name.len() == 32
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn modified(path: &Path) -> io::Result<SystemTime> {
    fs::metadata(path)?.modified()
}

fn set_modified(path: &Path, time: SystemTime) -> io::Result<()> {
    fs::File::options()
        .write(true)
        .open(path)?
        .set_modified(time)
}

impl DiskCache {
    /// The cache in `root` (created if missing), its index read from the folder. Folders whose name is not in `keep` are deleted:
    /// servers no longer saved, or signed out (ADR 0018, section 4). Temporary files a crash left are deleted too, and anything
    /// over `limit` is evicted.
    ///
    /// # Errors
    /// `root` could not be created or read.
    pub(crate) fn open(root: PathBuf, limit: u64, keep: &HashSet<String>) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        let mut index = Index::default();
        for folder in fs::read_dir(&root)?.flatten() {
            let Ok(folder_name) = folder.file_name().into_string() else {
                continue;
            };
            if !folder.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            if !keep.contains(&folder_name) {
                // Best effort: whatever is left is deleted at the next start.
                let _ = fs::remove_dir_all(folder.path());
                continue;
            }
            let Ok(files) = fs::read_dir(folder.path()) else {
                continue;
            };
            for file in files.flatten() {
                let Ok(name) = file.file_name().into_string() else {
                    continue;
                };
                if name.ends_with(TEMPORARY) {
                    let _ = fs::remove_file(file.path());
                    continue;
                }
                let Ok(metadata) = file.metadata() else {
                    continue;
                };
                if !is_hash(&name) || !metadata.is_file() {
                    continue;
                }
                let stamped = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                index.total += metadata.len();
                index.files.insert(
                    FileId {
                        folder: folder_name.clone(),
                        name,
                    },
                    FileEntry {
                        size: metadata.len(),
                        accessed: (stamped, 0),
                        stamped,
                    },
                );
            }
        }
        let cache = Self {
            root,
            limit,
            index: Mutex::new(index),
        };
        cache.evict();
        Ok(cache)
    }

    fn lock(&self) -> MutexGuard<'_, Index> {
        // Every change leaves the index consistent (a file it lists may be gone, which a read handles), so a poisoned lock is usable.
        self.index.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn path(&self, id: &FileId) -> PathBuf {
        self.root.join(&id.folder).join(&id.name)
    }

    /// The bytes of `id`, if cached. A hit is now the most recently used.
    pub(crate) fn read(&self, id: &FileId) -> Option<Vec<u8>> {
        if !self.lock().files.contains_key(id) {
            return None;
        }
        let path = self.path(id);
        let Ok(bytes) = fs::read(&path) else {
            // Deleted behind our back (or by an eviction meanwhile): forget it.
            self.lock().remove(id);
            return None;
        };
        let now = SystemTime::now();
        let mut index = self.lock();
        let sequence = index.next();
        let entry = index.files.get_mut(id)?;
        entry.accessed = (now, sequence);
        let stale = now
            .duration_since(entry.stamped)
            .map_or(true, |age| age >= REFRESH_EVERY);
        if stale && set_modified(&path, now).is_ok() {
            entry.stamped = now;
        }
        Some(bytes)
    }

    /// Stores `bytes` as `id`: written to a temporary file, then renamed, if `wanted` still says so at that moment (it is checked
    /// under the index lock, which [`DiskCache::delete_folder`] takes too, so a server's folder deleted meanwhile is not brought
    /// back). Then evicts what is over the limit. Failures leave nothing behind and are not reported: the image is fetched again.
    pub(crate) fn write(&self, id: &FileId, bytes: &[u8], wanted: impl FnOnce() -> bool) {
        let folder = self.root.join(&id.folder);
        let temporary = {
            let sequence = self.lock().next();
            folder.join(format!("{}.{sequence}{TEMPORARY}", id.name))
        };
        if fs::create_dir_all(&folder).is_err() || fs::write(&temporary, bytes).is_err() {
            let _ = fs::remove_file(&temporary);
            return;
        }
        let stamped = modified(&temporary).unwrap_or_else(|_| SystemTime::now());
        let renamed = {
            let mut index = self.lock();
            if wanted() && fs::rename(&temporary, self.path(id)).is_ok() {
                index.remove(id);
                let sequence = index.next();
                index.total += bytes.len() as u64;
                index.files.insert(
                    id.clone(),
                    FileEntry {
                        size: bytes.len() as u64,
                        // The clock, not the file's time, which a coarse file system may round down below an earlier read's.
                        accessed: (SystemTime::now(), sequence),
                        stamped,
                    },
                );
                true
            } else {
                false
            }
        };
        if renamed {
            self.evict();
        } else {
            let _ = fs::remove_file(&temporary);
        }
    }

    /// Over the limit, deletes the least recently used files until the total is 90% of it.
    fn evict(&self) {
        let mut index = self.lock();
        if index.total <= self.limit {
            return;
        }
        let target = self.limit / 10 * 9;
        let mut by_age: Vec<(FileId, (SystemTime, u64))> = index
            .files
            .iter()
            .map(|(id, entry)| (id.clone(), entry.accessed))
            .collect();
        by_age.sort_by_key(|(_, accessed)| *accessed);
        for (id, _) in by_age {
            if index.total <= target {
                break;
            }
            match fs::remove_file(self.path(&id)) {
                Ok(()) => index.remove(&id),
                Err(error) if error.kind() == io::ErrorKind::NotFound => index.remove(&id),
                // In use elsewhere (Windows) or not ours to delete: kept, and tried again at the next eviction.
                Err(_) => {}
            }
        }
    }

    /// Deletes a server's folder and everything in it.
    pub(crate) fn delete_folder(&self, folder: &str) {
        let mut index = self.lock();
        let doomed: Vec<FileId> = index
            .files
            .keys()
            .filter(|id| id.folder == folder)
            .cloned()
            .collect();
        for id in doomed {
            index.remove(&id);
        }
        // Best effort: what is left is deleted at the next start, since the folder is no longer kept.
        let _ = fs::remove_dir_all(self.root.join(folder));
    }

    /// The indexed total, in bytes.
    #[cfg(test)]
    pub(crate) fn total(&self) -> u64 {
        self.lock().total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOLDER: &str = "0123456789abcdef0123456789abcdef";
    const OTHER: &str = "fedcba9876543210fedcba9876543210";

    fn id(folder: &str, n: u8) -> FileId {
        FileId {
            folder: folder.into(),
            name: format!("{n:032x}"),
        }
    }

    fn keep(folders: &[&str]) -> HashSet<String> {
        folders.iter().map(|&folder| folder.to_owned()).collect()
    }

    fn open(root: &Path, limit: u64) -> DiskCache {
        DiskCache::open(root.to_owned(), limit, &keep(&[FOLDER, OTHER])).unwrap()
    }

    #[test]
    fn written_files_are_read_back_after_a_restart() {
        let temp = tempfile::tempdir().unwrap();
        let cache = open(temp.path(), 1000);
        cache.write(&id(FOLDER, 1), b"image one", || true);
        assert_eq!(
            cache.read(&id(FOLDER, 1)).as_deref(),
            Some(&b"image one"[..])
        );
        assert_eq!(cache.read(&id(FOLDER, 2)), None);
        drop(cache);
        let cache = open(temp.path(), 1000);
        assert_eq!(cache.total(), 9);
        assert_eq!(
            cache.read(&id(FOLDER, 1)).as_deref(),
            Some(&b"image one"[..])
        );
        // Nothing but the hashed name is left in the folder.
        let names: Vec<String> = fs::read_dir(temp.path().join(FOLDER))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [id(FOLDER, 1).name]);
    }

    #[test]
    fn eviction_keeps_the_most_recently_used_under_90_percent() {
        let temp = tempfile::tempdir().unwrap();
        let cache = open(temp.path(), 100);
        for n in 1..=4 {
            cache.write(&id(FOLDER, n), &[n; 25], || true);
        }
        // Used again: now newer than 2, 3 and 4.
        assert!(cache.read(&id(FOLDER, 1)).is_some());
        cache.write(&id(FOLDER, 5), &[5; 25], || true);
        // 125 > 100: down to at most 90, oldest first: 2 and 3 go.
        assert_eq!(cache.total(), 75);
        for (n, kept) in [(1, true), (2, false), (3, false), (4, true), (5, true)] {
            assert_eq!(cache.read(&id(FOLDER, n)).is_some(), kept, "image {n}");
            assert_eq!(
                temp.path().join(FOLDER).join(id(FOLDER, n).name).exists(),
                kept
            );
        }
    }

    #[test]
    fn a_start_over_the_limit_evicts_the_oldest() {
        let temp = tempfile::tempdir().unwrap();
        let cache = open(temp.path(), 1000);
        for n in 1..=3 {
            cache.write(&id(FOLDER, n), &[n; 40], || true);
        }
        drop(cache);
        let hour_ago = SystemTime::now() - Duration::from_secs(3600);
        set_modified(&temp.path().join(FOLDER).join(id(FOLDER, 2).name), hour_ago).unwrap();
        let cache = open(temp.path(), 100);
        assert_eq!(cache.total(), 80);
        assert!(cache.read(&id(FOLDER, 2)).is_none());
    }

    #[test]
    fn a_hit_refreshes_the_access_time_at_most_once_a_minute() {
        let temp = tempfile::tempdir().unwrap();
        let cache = open(temp.path(), 1000);
        cache.write(&id(FOLDER, 1), b"x", || true);
        drop(cache);
        let path = temp.path().join(FOLDER).join(id(FOLDER, 1).name);
        let hour_ago = SystemTime::now() - Duration::from_secs(3600);
        set_modified(&path, hour_ago).unwrap();

        let cache = open(temp.path(), 1000);
        assert!(cache.read(&id(FOLDER, 1)).is_some());
        let refreshed = modified(&path).unwrap();
        assert!(
            refreshed > hour_ago + Duration::from_secs(3000),
            "refreshed on the first hit"
        );
        // Set back by hand: the index knows it was refreshed just now, so the next hit leaves the file alone.
        set_modified(&path, hour_ago).unwrap();
        assert!(cache.read(&id(FOLDER, 1)).is_some());
        assert_eq!(modified(&path).unwrap(), hour_ago);
    }

    #[test]
    fn folders_not_kept_and_temporary_files_go_at_start() {
        let temp = tempfile::tempdir().unwrap();
        let cache = open(temp.path(), 1000);
        cache.write(&id(FOLDER, 1), b"one", || true);
        cache.write(&id(OTHER, 1), b"other", || true);
        drop(cache);
        let leftover = temp
            .path()
            .join(FOLDER)
            .join(format!("{}.7.tmp", id(FOLDER, 2).name));
        fs::write(&leftover, b"half").unwrap();

        let cache = DiskCache::open(temp.path().to_owned(), 1000, &keep(&[FOLDER])).unwrap();
        assert!(!temp.path().join(OTHER).exists());
        assert!(!leftover.exists());
        assert_eq!(cache.total(), 3);
        assert!(cache.read(&id(FOLDER, 1)).is_some());
    }

    #[test]
    fn deleting_a_folder_forgets_its_files_and_an_unwanted_write_leaves_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let cache = open(temp.path(), 1000);
        cache.write(&id(FOLDER, 1), b"one", || true);
        cache.write(&id(OTHER, 1), b"other", || true);
        cache.delete_folder(FOLDER);
        assert!(!temp.path().join(FOLDER).exists());
        assert_eq!(cache.read(&id(FOLDER, 1)), None);
        assert_eq!(cache.total(), 5);

        cache.write(&id(OTHER, 2), b"late", || false);
        assert_eq!(cache.read(&id(OTHER, 2)), None);
        assert_eq!(fs::read_dir(temp.path().join(OTHER)).unwrap().count(), 1);
    }
}
