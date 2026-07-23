//! Vec-backed replacements for hash maps/sets on SMALL, bounded
//! framework collections.
//!
//! Why: every distinct `HashMap<K, V>` monomorphizes hashbrown's whole
//! probe + growth + rehash machinery — ~1–2 KB of wasm per (K, V)
//! shape counting `insert`/`find`/`remove`/drop glue — **regardless of
//! hasher** (measured: swapping SipHash for FxHash removed the ~11 KB
//! hasher block but left each map's ~800 B `insert` untouched). For
//! collections bounded by a small runtime quantity — concurrent
//! pointers on screen, registered typefaces — a linear-scan `Vec` is
//! ~150 B per instantiation and also simply faster at these sizes: a
//! hash + probe costs more than scanning a few contiguous ids. Same
//! reasoning as [`crate::num::insertion_sort_by`] vs std sort.
//!
//! Do NOT use for unbounded or per-node collections (style registries,
//! class tables) — lookup here is O(n); a hash map remains correct
//! there. Iteration/removal order is unspecified (`swap_remove`).
//!
//! The method names and signatures deliberately mirror
//! `HashSet`/`HashMap` so a collection can move between the two
//! representations with only a type change at the declaration site.

/// A set of small `Copy` ids backed by a `Vec`. See module docs for
/// when (not) to use it.
#[derive(Debug, Default, Clone)]
pub struct SmallIdSet<T>(Vec<T>);

impl<T: Copy + PartialEq> SmallIdSet<T> {
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Insert `v`; returns `true` if it was not already present
    /// (mirrors `HashSet::insert`).
    pub fn insert(&mut self, v: T) -> bool {
        if self.0.contains(&v) {
            false
        } else {
            self.0.push(v);
            true
        }
    }

    pub fn contains(&self, v: &T) -> bool {
        self.0.contains(v)
    }

    /// Remove `v`; returns `true` if it was present (mirrors
    /// `HashSet::remove`). Order of the remaining ids is not preserved.
    pub fn remove(&mut self, v: &T) -> bool {
        match self.0.iter().position(|x| x == v) {
            Some(i) => {
                self.0.swap_remove(i);
                true
            }
            None => false,
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// A map keyed by small `Copy` ids, backed by a `Vec` of pairs. See
/// module docs for when (not) to use it.
#[derive(Debug, Default, Clone)]
pub struct SmallIdMap<K, V>(Vec<(K, V)>);

impl<K: Copy + PartialEq, V> SmallIdMap<K, V> {
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Insert or replace; returns the previous value for `k` if any
    /// (mirrors `HashMap::insert`).
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        match self.0.iter_mut().find(|(ek, _)| *ek == k) {
            Some(entry) => Some(core::mem::replace(&mut entry.1, v)),
            None => {
                self.0.push((k, v));
                None
            }
        }
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        self.0.iter().find(|(ek, _)| ek == k).map(|(_, v)| v)
    }

    pub fn contains_key(&self, k: &K) -> bool {
        self.0.iter().any(|(ek, _)| ek == k)
    }

    /// Remove `k`'s entry, returning its value (mirrors
    /// `HashMap::remove`). Order of remaining entries not preserved.
    pub fn remove(&mut self, k: &K) -> Option<V> {
        match self.0.iter().position(|(ek, _)| ek == k) {
            Some(i) => Some(self.0.swap_remove(i).1),
            None => None,
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{SmallIdMap, SmallIdSet};

    #[test]
    fn set_mirrors_hashset_semantics() {
        let mut s: SmallIdSet<i32> = SmallIdSet::new();
        assert!(s.is_empty());
        assert!(s.insert(3));
        assert!(s.insert(7));
        assert!(!s.insert(3), "duplicate insert returns false");
        assert_eq!(s.len(), 2);
        assert!(s.contains(&3) && s.contains(&7) && !s.contains(&9));
        assert!(s.remove(&3));
        assert!(!s.remove(&3), "second remove returns false");
        assert!(!s.contains(&3) && s.contains(&7));
    }

    #[test]
    fn map_mirrors_hashmap_semantics() {
        let mut m: SmallIdMap<i32, (f64, f64)> = SmallIdMap::new();
        assert!(m.get(&1).is_none());
        assert_eq!(m.insert(1, (1.0, 2.0)), None);
        assert_eq!(m.insert(2, (3.0, 4.0)), None);
        // Replace returns the old value.
        assert_eq!(m.insert(1, (9.0, 9.0)), Some((1.0, 2.0)));
        assert_eq!(m.len(), 2, "replace does not grow the map");
        assert!(m.contains_key(&2) && !m.contains_key(&5));
        assert_eq!(m.get(&1), Some(&(9.0, 9.0)));
        assert_eq!(m.remove(&1), Some((9.0, 9.0)));
        assert_eq!(m.remove(&1), None);
        assert_eq!(m.get(&2).copied(), Some((3.0, 4.0)));
    }
}
