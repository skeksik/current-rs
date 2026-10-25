//! The memory cache: encoded image bytes, least recently used first out, bounded by their total size (ADR 0018, section 4).

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::sync::Arc;

/// The default bound: 64 MB.
pub(crate) const MEMORY_LIMIT: u64 = 64 * 1024 * 1024;

struct Entry {
    bytes: Arc<[u8]>,
    used: u64,
}

/// A least-recently-used map of byte buffers, bounded by their total length.
pub(crate) struct MemoryCache<K> {
    limit: u64,
    total: u64,
    clock: u64,
    entries: HashMap<K, Entry>,
    /// Last use to key, oldest first.
    order: BTreeMap<u64, K>,
}

impl<K: Clone + Eq + Hash> MemoryCache<K> {
    pub(crate) fn new(limit: u64) -> Self {
        Self {
            limit,
            total: 0,
            clock: 0,
            entries: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// The bytes for `key`, now the most recently used.
    pub(crate) fn get(&mut self, key: &K) -> Option<Arc<[u8]>> {
        let now = self.tick();
        let entry = self.entries.get_mut(key)?;
        self.order.remove(&entry.used);
        entry.used = now;
        self.order.insert(now, key.clone());
        Some(entry.bytes.clone())
    }

    /// Keeps `bytes` for `key`, dropping the least recently used entries until the total fits. Bytes larger than the whole bound
    /// are not kept.
    pub(crate) fn insert(&mut self, key: K, bytes: Arc<[u8]>) {
        self.remove(&key);
        let size = bytes.len() as u64;
        if size > self.limit {
            return;
        }
        while self.total + size > self.limit {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.total -= entry.bytes.len() as u64;
            }
        }
        let used = self.tick();
        self.order.insert(used, key.clone());
        self.entries.insert(key, Entry { bytes, used });
        self.total += size;
    }

    pub(crate) fn remove(&mut self, key: &K) {
        if let Some(entry) = self.entries.remove(key) {
            self.order.remove(&entry.used);
            self.total -= entry.bytes.len() as u64;
        }
    }

    /// Drops every entry `matches` selects.
    pub(crate) fn remove_where(&mut self, matches: impl Fn(&K) -> bool) {
        let doomed: Vec<K> = self
            .entries
            .keys()
            .filter(|key| matches(key))
            .cloned()
            .collect();
        for key in doomed {
            self.remove(&key);
        }
    }

    #[cfg(test)]
    pub(crate) fn total(&self) -> u64 {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(size: usize) -> Arc<[u8]> {
        vec![7u8; size].into()
    }

    #[test]
    fn the_least_recently_used_goes_first() {
        let mut cache = MemoryCache::new(30);
        cache.insert("a", bytes(10));
        cache.insert("b", bytes(10));
        cache.insert("c", bytes(10));
        assert!(cache.get(&"a").is_some());
        cache.insert("d", bytes(10));
        assert!(cache.get(&"b").is_none(), "b was used least recently");
        assert!(cache.get(&"a").is_some());
        assert!(cache.get(&"c").is_some());
        assert!(cache.get(&"d").is_some());
        assert_eq!(cache.total(), 30);
    }

    #[test]
    fn a_large_entry_evicts_several_and_one_too_large_is_not_kept() {
        let mut cache = MemoryCache::new(30);
        cache.insert("a", bytes(10));
        cache.insert("b", bytes(10));
        cache.insert("c", bytes(25));
        assert!(cache.get(&"a").is_none() && cache.get(&"b").is_none());
        assert_eq!(cache.total(), 25);
        cache.insert("d", bytes(31));
        assert!(cache.get(&"d").is_none());
        assert!(cache.get(&"c").is_some());
    }

    #[test]
    fn replacing_and_removing_keep_the_total() {
        let mut cache = MemoryCache::new(100);
        cache.insert(("s1", 1), bytes(10));
        cache.insert(("s1", 1), bytes(20));
        cache.insert(("s1", 2), bytes(5));
        cache.insert(("s2", 1), bytes(5));
        assert_eq!(cache.total(), 30);
        cache.remove_where(|(server, _)| *server == "s1");
        assert_eq!(cache.total(), 5);
        assert!(cache.get(&("s2", 1)).is_some());
    }
}
