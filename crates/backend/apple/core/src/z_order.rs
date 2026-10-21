//! Sibling `z_index` hit-testing, shared by the UIKit and AppKit
//! backends. Pure logic — no objc — so it is unit-tested on any host.
//!
//! Both backends paint `StyleRules::z_index` through `layer.zPosition`
//! (static styles write `StyleRules::stacking_z()`, `AnimProp::ZIndex`
//! writes the same channel), and Core Animation sorts sibling sublayers by
//! `zPosition` when it composites. But `-[UIView hitTest:withEvent:]` and
//! `-[NSView hitTest:]` walk `subviews` by array index (last = topmost) and
//! ignore `zPosition`, so a raised sibling would PAINT on top yet lose the
//! pointer to the later sibling it covers. The framework view classes'
//! `hitTest:` overrides therefore take over the subview walk whenever a
//! sibling has a nonzero `zPosition`, visiting subviews in the REVERSE of
//! [`runtime_shared::style::z_paint_order`] — the one ordering rule every
//! backend shares, so ties fall back to tree order exactly as paint does.
//! When every sibling's z is `0` the platform's own walk is already that
//! order and the overrides keep calling `super` ([`needs_z_walk`]).

/// Whether a subview walk must be reordered for `zs` (each subview's
/// `layer.zPosition`, in `subviews` order). `false` when every z is `0`
/// (or NaN, which paints as `0`): the platform's reverse-index walk is
/// then identical to the reverse paint order.
pub fn needs_z_walk(zs: &[f64]) -> bool {
    zs.iter().any(|z| *z != 0.0 && !z.is_nan())
}

/// Subview indices in hit-test order — topmost painted first: the reverse
/// of [`runtime_shared::style::z_paint_order`]. Equal z keeps the later
/// subview first, matching the platform walk.
pub fn hit_test_order(zs: &[f64]) -> Vec<usize> {
    let zs: Vec<f32> = zs.iter().map(|z| *z as f32).collect();
    let mut order = runtime_shared::style::z_paint_order(&zs);
    order.reverse();
    order
}

/// Walk subviews in [`hit_test_order`], returning the first subview hit
/// `probe` resolves (`probe(i)` hit-tests subview `i`, e.g. its own
/// `hitTest:` with the point converted into its space).
pub fn first_hit<T>(zs: &[f64], mut probe: impl FnMut(usize) -> Option<T>) -> Option<T> {
    hit_test_order(zs).into_iter().find_map(|i| probe(i))
}

/// Paint order for the subviews of a parent the framework does NOT own —
/// the `UIWindow` / host content view that portal containers mount into
/// — as indices into `zs` (each subview's `zPosition`, in `subviews`
/// order). `mount_seq[i]` is `Some(seq)` for a portal container (its
/// mount sequence) and `None` for any other view.
///
/// That parent's class is the platform's, so its `hitTest:` walks by
/// index; the backend instead REORDERS the subviews into this order, after
/// which index order is paint order and the stock walk hits the topmost
/// painted view. "Tree order" here is the order web would have: other
/// views where they are, portal containers in mount order (the order web
/// appends their roots) in the slots portal containers occupy — so an
/// earlier reorder never leaks into a later tie. Then a stable sort by z.
pub fn host_subview_order(zs: &[f64], mount_seq: &[Option<u64>]) -> Vec<usize> {
    assert_eq!(zs.len(), mount_seq.len());
    let slots: Vec<usize> = (0..zs.len()).filter(|&i| mount_seq[i].is_some()).collect();
    let mut by_mount = slots.clone();
    by_mount.sort_by_key(|&i| mount_seq[i]);
    let mut tree: Vec<usize> = (0..zs.len()).collect();
    for (slot, portal) in slots.iter().zip(by_mount) {
        tree[*slot] = portal;
    }
    let tree_zs: Vec<f32> = tree.iter().map(|&i| zs[i] as f32).collect();
    runtime_shared::style::z_paint_order(&tree_zs)
        .into_iter()
        .map(|k| tree[k])
        .collect()
}

/// Below this `alpha` UIKit's default `hitTest:withEvent:` ignores a view
/// (UIView documentation: "an alpha level less than 0.01").
pub const UIKIT_MIN_HIT_ALPHA: f64 = 0.01;

/// UIKit's default eligibility check, which a `hitTest:withEvent:` override
/// that takes over the subview walk must reproduce on itself: a hidden view,
/// one with user interaction disabled, or one with alpha below
/// [`UIKIT_MIN_HIT_ALPHA`] returns `nil` without consulting its subviews.
pub fn uikit_accepts_hits(hidden: bool, user_interaction_enabled: bool, alpha: f64) -> bool {
    !hidden && user_interaction_enabled && alpha >= UIKIT_MIN_HIT_ALPHA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_zero_keeps_the_platform_walk() {
        assert!(!needs_z_walk(&[0.0, 0.0, 0.0]));
        assert!(!needs_z_walk(&[]));
        assert!(!needs_z_walk(&[0.0, f64::NAN]));
        assert_eq!(hit_test_order(&[0.0, 0.0, 0.0]), vec![2, 1, 0]);
    }

    #[test]
    fn any_nonzero_takes_over_the_walk() {
        assert!(needs_z_walk(&[0.0, 2.0]));
        assert!(needs_z_walk(&[1.0, 0.0]));
    }

    /// The z_index contract: where siblings overlap, the one painted on top
    /// receives the pointer. Two overlapping siblings, the FIRST raised —
    /// the platform walk (last subview first) would hand the hit to the
    /// second, which paints underneath.
    #[test]
    fn regression_raised_first_sibling_wins_the_hit() {
        let zs = [2.0, 0.0];
        assert_eq!(hit_test_order(&zs), vec![0, 1]);
        // Both siblings contain the point.
        assert_eq!(first_hit(&zs, Some), Some(0));
    }

    /// Ties keep tree order: among equal z the later sibling paints on top,
    /// so it is probed first.
    #[test]
    fn ties_probe_the_later_sibling_first() {
        assert_eq!(hit_test_order(&[1.0, 0.0, 1.0, 0.0]), vec![2, 0, 3, 1]);
    }

    /// Sticky's raise (`STICKY_Z` = 1) loses to an explicit higher z_index.
    #[test]
    fn explicit_z_beats_sticky_raise() {
        assert_eq!(hit_test_order(&[1.0, 3.0, 0.0]), vec![1, 0, 2]);
    }

    /// A miss on the topmost falls through to the next in paint order.
    #[test]
    fn miss_on_top_falls_through_in_paint_order() {
        let zs = [0.0, 5.0, 1.0];
        let mut probed = Vec::new();
        let hit = first_hit(&zs, |i| {
            probed.push(i);
            (i == 0).then_some(i)
        });
        assert_eq!(hit, Some(0));
        assert_eq!(probed, vec![1, 2, 0]);
    }

    /// NaN z (an animated value gone bad) paints as 0, so it hit-tests as 0.
    #[test]
    fn nan_orders_as_zero() {
        assert_eq!(hit_test_order(&[f64::NAN, 1.0, 0.0]), vec![1, 2, 0]);
    }

    /// Two portals: the first mounted raises itself with `z_index`, so it
    /// moves above the later one; the host's own views keep their place.
    #[test]
    fn raised_portal_moves_above_a_later_portal() {
        // [app root, portal#1 (z 5), portal#2 (z 0)]
        let order = host_subview_order(&[0.0, 5.0, 0.0], &[None, Some(1), Some(2)]);
        assert_eq!(order, vec![0, 2, 1]);
    }

    /// After a reorder, dropping the z restores MOUNT order on a tie — the
    /// earlier reorder must not stick (web appends portal roots in mount
    /// order and never moves them).
    #[test]
    fn tie_falls_back_to_mount_order_not_current_order() {
        // Currently [app root, portal#2, portal#1], both back at z 0.
        let order = host_subview_order(&[0.0, 0.0, 0.0], &[None, Some(2), Some(1)]);
        assert_eq!(order, vec![0, 2, 1]);
    }

    /// Nothing raised, already in mount order: the identity, so the backend
    /// leaves the host's subviews untouched.
    #[test]
    fn unraised_host_is_left_alone() {
        let order = host_subview_order(&[0.0, 0.0, 0.0], &[None, Some(1), Some(2)]);
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn uikit_eligibility_matches_the_default_hit_test() {
        assert!(uikit_accepts_hits(false, true, 1.0));
        assert!(uikit_accepts_hits(false, true, UIKIT_MIN_HIT_ALPHA));
        assert!(!uikit_accepts_hits(false, true, 0.009));
        assert!(!uikit_accepts_hits(true, true, 1.0));
        assert!(!uikit_accepts_hits(false, false, 1.0));
    }
}
