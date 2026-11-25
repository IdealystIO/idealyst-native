//! Keyed diff for virtualizer data changes: old item keys → new item keys
//! → the inserts, deletes and moves a collection view applies as one
//! batch (`performBatchUpdates` on AppKit's `NSCollectionView` and
//! UIKit's `UICollectionView`).
//!
//! The ops follow the batch-update contract both toolkits document:
//! deletes name OLD indices, inserts name NEW indices, a move names its
//! old and its new index, and no index is deleted and moved from, or
//! inserted and moved to, in one batch. A row that is neither deleted nor
//! moved keeps its cell; the toolkit shifts it past the inserts and
//! deletes around it.
//!
//! Moves are minimal: the surviving keys whose old indices form a longest
//! increasing subsequence in new order stay put, and only the others
//! move. That is the reorder rule keyed DOM reconcilers use (Vue 3's
//! `getSequence`, Inferno, ivi): any increasing run keeps its relative
//! order across the change, so the longest one is the largest set that
//! needs no move.

use std::collections::HashMap;

use super::virtualizer::ItemKey;

/// A batch above this many ops is not applied as a diff: the toolkit's
/// per-op bookkeeping (index-path translation, layout attributes for
/// each moved, inserted and deleted item) grows with the batch, while a
/// full reload only rebuilds the rows on screen. A Kanban move is one
/// insert or one delete; re-sorting a long list is hundreds of moves.
pub const MAX_BATCH_OPS: usize = 128;

/// The ops that turn the old key list into the new one.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KeyedUpdate {
    /// Old indices, ascending.
    pub deletes: Vec<usize>,
    /// New indices, ascending.
    pub inserts: Vec<usize>,
    /// `(old, new)`, ascending by new index.
    pub moves: Vec<(usize, usize)>,
}

impl KeyedUpdate {
    /// No structural change: same keys, same order, no row to refresh.
    pub fn is_empty(&self) -> bool {
        self.deletes.is_empty() && self.inserts.is_empty() && self.moves.is_empty()
    }

    /// Op count, for the [`MAX_BATCH_OPS`] bound.
    pub fn len(&self) -> usize {
        self.deletes.len() + self.inserts.len() + self.moves.len()
    }
}

/// Diff `old` against `new`. `changed(new_idx)` says whether the live
/// row of a surviving key must be rebuilt from its new item
/// (`VirtualizerCallbacks::item_changed`); such a key is deleted at its
/// old index and inserted at its new one, which releases its row and
/// mounts it afresh. Unchanged survivors keep their rows.
///
/// `None` when the diff is unusable and the caller should reload
/// everything instead:
/// - a key appears twice in `old` or in `new` (identity is ambiguous, and
///   a batch naming one key twice is rejected by the toolkit);
/// - `old` is empty (a first load: there is no row to keep);
/// - no unchanged survivor is left (nothing to keep either);
/// - the batch would exceed `max_ops`.
pub fn diff_keys(
    old: &[ItemKey],
    new: &[ItemKey],
    mut changed: impl FnMut(usize) -> bool,
    max_ops: usize,
) -> Option<KeyedUpdate> {
    if old.is_empty() {
        return None;
    }
    let old_index = index_unique(old)?;
    let new_index = index_unique(new)?;

    let mut update = KeyedUpdate::default();
    for (old_i, key) in old.iter().enumerate() {
        if !new_index.contains_key(key) {
            update.deletes.push(old_i);
        }
    }
    // Survivors in new order, as (old, new); a changed one is replaced.
    let mut kept: Vec<(usize, usize)> = Vec::new();
    for (new_i, key) in new.iter().enumerate() {
        match old_index.get(key) {
            None => update.inserts.push(new_i),
            Some(&old_i) if changed(new_i) => {
                update.deletes.push(old_i);
                update.inserts.push(new_i);
            }
            Some(&old_i) => kept.push((old_i, new_i)),
        }
    }
    if kept.is_empty() {
        return None;
    }
    update.deletes.sort_unstable();

    let olds: Vec<usize> = kept.iter().map(|&(o, _)| o).collect();
    let stay = longest_increasing_subsequence(&olds);
    let mut stay_iter = stay.iter().peekable();
    for (pos, &pair) in kept.iter().enumerate() {
        if stay_iter.peek() == Some(&&pos) {
            stay_iter.next();
        } else {
            update.moves.push(pair);
        }
    }

    if update.len() > max_ops {
        return None;
    }
    Some(update)
}

/// key → index, or `None` when a key repeats.
fn index_unique(keys: &[ItemKey]) -> Option<HashMap<ItemKey, usize>> {
    let mut map = HashMap::with_capacity(keys.len());
    for (i, &k) in keys.iter().enumerate() {
        if map.insert(k, i).is_some() {
            return None;
        }
    }
    Some(map)
}

/// Positions (into `seq`) of one longest strictly increasing subsequence,
/// ascending. Patience sorting with predecessor links, O(n log n).
fn longest_increasing_subsequence(seq: &[usize]) -> Vec<usize> {
    // tails[k] = position of the smallest tail of an increasing run of
    // length k + 1 seen so far.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; seq.len()];
    for (i, &v) in seq.iter().enumerate() {
        let k = tails.partition_point(|&t| seq[t] < v);
        if k > 0 {
            prev[i] = Some(tails[k - 1]);
        }
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        out.push(i);
        cur = prev[i];
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(old: &[u64], new: &[u64]) -> Option<KeyedUpdate> {
        diff_keys(old, new, |_| false, MAX_BATCH_OPS)
    }

    /// Replay `update` the way a collection view does (deletes at old
    /// indices, moves old → new, inserts at new indices) and return the
    /// resulting key list. Inserted slots take the new key at that index.
    fn replay(old: &[u64], new: &[u64], update: &KeyedUpdate) -> Vec<u64> {
        let mut out: Vec<Option<u64>> = vec![None; new.len()];
        let moved_from: Vec<usize> = update.moves.iter().map(|m| m.0).collect();
        for &(o, n) in &update.moves {
            out[n] = Some(old[o]);
        }
        for &n in &update.inserts {
            assert!(out[n].is_none(), "index {n} inserted and moved to");
            out[n] = Some(new[n]);
        }
        // Rows neither deleted nor moved fill the remaining slots in
        // their old order.
        let mut rest = old
            .iter()
            .enumerate()
            .filter(|(i, _)| !update.deletes.contains(i) && !moved_from.contains(i))
            .map(|(_, k)| *k);
        for slot in out.iter_mut() {
            if slot.is_none() {
                *slot = rest.next();
            }
        }
        assert!(rest.next().is_none(), "count mismatch");
        out.into_iter().map(|k| k.expect("every slot filled")).collect()
    }

    fn check(old: &[u64], new: &[u64]) -> KeyedUpdate {
        let u = diff(old, new).expect("usable diff");
        for d in &u.deletes {
            assert!(!u.moves.iter().any(|m| m.0 == *d), "index {d} deleted and moved from");
        }
        assert_eq!(old.len() - u.deletes.len() + u.inserts.len(), new.len(), "count rule");
        assert_eq!(replay(old, new, &u), new, "replay reproduces the new order");
        u
    }

    #[test]
    fn same_keys_same_order_is_empty() {
        assert!(check(&[1, 2, 3], &[1, 2, 3]).is_empty());
    }

    #[test]
    fn removing_a_card_is_one_delete_and_no_moves() {
        let u = check(&[1, 2, 3, 4], &[1, 2, 4]);
        assert_eq!(u, KeyedUpdate { deletes: vec![2], ..Default::default() });
    }

    #[test]
    fn dropping_a_card_in_is_one_insert_and_no_moves() {
        let u = check(&[1, 2, 4], &[1, 2, 9, 4]);
        assert_eq!(u, KeyedUpdate { inserts: vec![2], ..Default::default() });
    }

    #[test]
    fn moving_one_key_is_one_move() {
        let u = check(&[1, 2, 3, 4, 5], &[1, 4, 2, 3, 5]);
        assert_eq!(u, KeyedUpdate { moves: vec![(3, 1)], ..Default::default() });
        let u = check(&[1, 2, 3, 4, 5], &[2, 3, 4, 5, 1]);
        assert_eq!(u.moves, vec![(0, 4)]);
    }

    #[test]
    fn move_insert_and_delete_together() {
        let u = check(&[1, 2, 3, 4, 5], &[4, 1, 6, 2, 5]);
        assert_eq!(u.deletes, vec![2]);
        assert_eq!(u.inserts, vec![2]);
        assert_eq!(u.moves, vec![(3, 0)]);
    }

    #[test]
    fn reversal_moves_all_but_one() {
        let u = check(&[1, 2, 3, 4], &[4, 3, 2, 1]);
        assert_eq!(u.moves.len(), 3);
    }

    #[test]
    fn a_changed_survivor_is_deleted_and_reinserted() {
        // Key 2 (new index 2) changed; key 4 moved but is unchanged.
        let u = diff_keys(&[1, 2, 3, 4], &[1, 4, 2, 3], |i| i == 2, MAX_BATCH_OPS).unwrap();
        assert_eq!(u.deletes, vec![1]);
        assert_eq!(u.inserts, vec![2]);
        assert_eq!(replay(&[1, 2, 3, 4], &[1, 4, 2, 3], &u), vec![1, 4, 2, 3]);
        assert!(!u.moves.iter().any(|m| m.0 == 1), "the replaced row is not also moved");
    }

    #[test]
    fn unusable_diffs_fall_back_to_a_reload() {
        assert_eq!(diff(&[], &[1, 2]), None, "first load");
        assert_eq!(diff(&[1, 1], &[1]), None, "duplicate old key");
        assert_eq!(diff(&[1, 2], &[2, 2]), None, "duplicate new key");
        assert_eq!(diff(&[1, 2], &[3, 4]), None, "nothing survives");
        assert_eq!(diff(&[1, 2], &[]), None, "everything deleted");
        assert_eq!(
            diff_keys(&[1, 2], &[1, 2], |_| true, MAX_BATCH_OPS),
            None,
            "every survivor changed"
        );
        let old: Vec<u64> = (0..10).collect();
        let new: Vec<u64> = (0..10).rev().collect();
        assert_eq!(diff_keys(&old, &new, |_| false, 4), None, "too many ops");
        assert!(diff_keys(&old, &new, |_| false, 9).is_some());
    }

    #[test]
    fn lis_is_longest_and_increasing() {
        let seq = [3, 1, 4, 1, 5, 9, 2, 6];
        let lis = longest_increasing_subsequence(&seq);
        assert_eq!(lis.len(), 4);
        assert!(lis.windows(2).all(|w| w[0] < w[1] && seq[w[0]] < seq[w[1]]));
        assert!(longest_increasing_subsequence(&[]).is_empty());
    }

    /// Exhaustive-ish: every permutation of 5 keys with one key dropped
    /// and one added replays correctly.
    #[test]
    fn permutations_replay() {
        fn perms(v: &mut Vec<u64>, k: usize, out: &mut Vec<Vec<u64>>) {
            if k == v.len() {
                out.push(v.clone());
                return;
            }
            for i in k..v.len() {
                v.swap(k, i);
                perms(v, k + 1, out);
                v.swap(k, i);
            }
        }
        let old = [1u64, 2, 3, 4, 5];
        let mut all = Vec::new();
        perms(&mut vec![1, 2, 3, 4, 9], 0, &mut all);
        for new in all {
            check(&old, &new);
        }
    }
}
