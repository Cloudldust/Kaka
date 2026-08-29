//! In-memory LRU cache with byte-cost accounting (PRD 7.4 / 9.5).
//!
//! The Z-key full-resolution RAW texture cache uses this with a 2 GB cap
//! (PRD 7.4: 缓存在内存中，LRU 策略，总上限 2GB); the UI-side preview texture
//! cache uses a smaller cap so long browsing sessions don't grow GPU memory
//! without bound.

use std::collections::HashMap;
use std::hash::Hash;

/// A least-recently-used cache with a total byte-cost limit.
///
/// Entries carry an explicit `cost` (bytes). Inserting evicts the
/// least-recently-used entries until the total fits the capacity. An entry
/// whose own cost exceeds the capacity is still kept (so the photo currently
/// being viewed always renders) — it simply evicts everything else.
pub struct MemLru<K: Eq + Hash + Clone, V> {
    cap_bytes: u64,
    entries: HashMap<K, (V, u64, u64)>, // value, cost, recency seq
    seq: u64,
    total: u64,
}

impl<K: Eq + Hash + Clone, V> MemLru<K, V> {
    pub fn new(cap_bytes: u64) -> Self {
        MemLru {
            cap_bytes,
            entries: HashMap::new(),
            seq: 0,
            total: 0,
        }
    }

    /// Cached value for `key`, refreshing its recency.
    pub fn get(&mut self, key: &K) -> Option<V>
    where
        V: Clone,
    {
        self.seq += 1;
        let seq = self.seq;
        self.entries.get_mut(key).map(|(v, _, s)| {
            *s = seq;
            v.clone()
        })
    }

    /// Insert (or replace) an entry with its byte cost, evicting LRU entries
    /// as needed to stay within the capacity.
    pub fn insert(&mut self, key: K, value: V, cost: u64) {
        self.seq += 1;
        let seq = self.seq;
        if let Some((_, old_cost, _)) = self.entries.insert(key, (value, cost, seq)) {
            self.total = self.total.saturating_sub(old_cost);
        }
        self.total = self.total.saturating_add(cost);
        self.evict_overflow();
    }

    fn evict_overflow(&mut self) {
        while self.total > self.cap_bytes {
            // Never evict the last remaining entry: an oversized entry (cost >
            // cap, e.g. a 100MP frame) must stay so the photo being viewed
            // still renders.
            if self.entries.len() <= 1 {
                break;
            }
            let Some(victim) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, _, s))| *s)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            match self.entries.remove(&victim) {
                Some((_, evicted_cost, _)) => {
                    self.total = self.total.saturating_sub(evicted_cost);
                }
                None => break,
            }
        }
    }

    /// Remove an entry, returning its value if present.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.entries.remove(key).map(|(v, cost, _)| {
            self.total = self.total.saturating_sub(cost);
            v
        })
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Sum of all entry costs (bytes).
    pub fn total_cost(&self) -> u64 {
        self.total
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.total = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used() {
        let mut c = MemLru::new(100);
        c.insert("a", 1, 40);
        c.insert("b", 2, 40);
        assert_eq!(c.total_cost(), 80);
        // Refresh "a" so "b" becomes the LRU victim.
        assert_eq!(c.get(&"a"), Some(1));
        c.insert("c", 3, 40);
        assert_eq!(c.len(), 2, "total 120 > cap 100: one entry evicted");
        assert_eq!(c.get(&"b"), None, "b was least recently used");
        assert!(c.contains(&"a") && c.contains(&"c"));
    }

    #[test]
    fn replace_updates_cost() {
        let mut c = MemLru::new(100);
        c.insert("a", 1, 40);
        c.insert("a", 1, 90); // replaced: cost now 90
        assert_eq!(c.total_cost(), 90);
        assert_eq!(c.len(), 1);
        c.insert("b", 2, 20); // 90 + 20 = 110 > 100 → LRU victim ("a", older than b) evicted
        assert!(c.contains(&"b"));
        assert!(!c.contains(&"a"));
        assert_eq!(c.total_cost(), 20);
    }

    #[test]
    fn oversized_entry_kept_but_evicts_everything_else() {
        let mut c = MemLru::new(100);
        c.insert("a", 1, 40);
        c.insert("big", 2, 500);
        assert!(c.contains(&"big"), "the just-inserted entry is never evicted");
        assert_eq!(c.get(&"a"), None);
        assert_eq!(c.total_cost(), 500);
    }

    #[test]
    fn remove_and_clear() {
        let mut c = MemLru::new(100);
        c.insert("a", vec![1u8], 40);
        assert_eq!(c.remove(&"a"), Some(vec![1u8]));
        assert_eq!(c.total_cost(), 0);
        c.insert("b", vec![2u8], 40);
        c.clear();
        assert!(c.is_empty());
    }
}
