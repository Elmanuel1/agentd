//! A bounded per-replica map whose entries expire: the egress clients
//! ([`crate::net::Guard`]) and the API key cache. When it is full, expired
//! entries are cleared; if it is still full, the new entry is not kept.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

/// Entries kept for `ttl` after their insert, at most `cap` of them.
#[derive(Debug)]
pub struct TtlMap<K, V> {
    ttl: Duration,
    cap: usize,
    entries: Mutex<HashMap<K, (Instant, V)>>,
}

impl<K: Eq + Hash, V: Clone> TtlMap<K, V> {
    /// An empty map.
    pub fn new(ttl: Duration, cap: usize) -> Self {
        Self { ttl, cap, entries: Mutex::new(HashMap::new()) }
    }

    /// `key`'s value, while it is younger than the map's `ttl`.
    pub fn get(&self, key: &K) -> Option<V> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let (at, value) = entries.get(key)?;
        if at.elapsed() < self.ttl {
            return Some(value.clone());
        }
        entries.remove(key);
        None
    }

    /// Keep `value` under `key` from now on.
    pub fn insert(&self, key: K, value: V) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if entries.len() >= self.cap && !entries.contains_key(&key) {
            entries.retain(|_, (at, _)| at.elapsed() < self.ttl);
        }
        if entries.len() < self.cap || entries.contains_key(&key) {
            entries.insert(key, (Instant::now(), value));
        }
    }

    /// Drop every entry whose value `matches`.
    pub fn remove_where(&self, matches: impl Fn(&V) -> bool) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|_, (_, value)| !matches(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn an_entry_expires_after_its_ttl() {
        let map = TtlMap::new(Duration::from_secs(60), 10);
        map.insert("a", 1);
        tokio::time::advance(Duration::from_secs(59)).await;
        assert_eq!(map.get(&"a"), Some(1));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(map.get(&"a"), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_map_clears_expired_entries_and_otherwise_keeps_none() {
        let map = TtlMap::new(Duration::from_secs(60), 2);
        map.insert("a", 1);
        tokio::time::advance(Duration::from_secs(61)).await;
        map.insert("b", 2);
        map.insert("c", 3);
        assert_eq!((map.get(&"a"), map.get(&"b"), map.get(&"c")), (None, Some(2), Some(3)));
        map.insert("d", 4);
        assert_eq!(map.get(&"d"), None, "full of live entries, so not kept");
        map.insert("b", 5);
        assert_eq!(map.get(&"b"), Some(5), "a key already held is replaced");
        map.remove_where(|v| *v == 3);
        assert_eq!(map.get(&"c"), None);
    }
}
