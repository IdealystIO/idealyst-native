//! Layout order for HOSTED roots — screen subtrees a native stack
//! presenter (`android-navigator-helpers`) seats in its own container
//! inside a navigator outlet, instead of inserting them into the outlet.
//!
//! # Why a hosted root exists
//!
//! A stack presenter has to show two screens at once while it animates
//! (the outgoing one sliding out, the incoming one sliding in). As Taffy
//! children of the outlet they would be laid out side by side in its flex
//! flow, so the presenter keeps each screen out of the outlet's Taffy
//! child list and the screen becomes a Taffy ROOT. An ordinary root is
//! computed against the viewport, which is the wrong box for a screen
//! under an author `StackHeader`: it would lay out taller than the
//! outlet it is shown in. A hosted root is computed against its HOST's
//! laid-out size (the outlet), and framed at the host's origin by the
//! presenter's full-size wrapper.
//!
//! # Why order matters
//!
//! The host's size is only known once the root containing the host has
//! been computed in this pass. Ordinary roots go first; a hosted root
//! whose host lives inside ANOTHER hosted root (a stack inside a pushed
//! screen of a stack) must go after that one. Un-gated like
//! `sticky_compute` so the ordering is tested on the host; the JNI half
//! (registry, compute) lives in `imp`.

use std::collections::HashMap;
use std::hash::Hash;

/// Order `hosted` (root, host) pairs so every root comes after the
/// hosted root its host lives in. `root_of(node)` walks a node up to its
/// Taffy root.
///
/// A host whose root is not itself hosted (it lives in the app root or
/// in a portal) has depth 0. Depth is capped at `hosted.len()`, so a
/// malformed registry (a screen hosting its own host) terminates instead
/// of looping; it cannot be produced by the presenter, which only ever
/// hosts a freshly realized screen in an outlet that already exists.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn hosted_root_order<N: Copy + Eq + Hash>(
    hosted: &[(N, N)],
    root_of: impl Fn(N) -> N,
) -> Vec<(N, N)> {
    let host_of: HashMap<N, N> = hosted.iter().copied().collect();
    let depth = |mut root: N| {
        let mut d = 0usize;
        while d < hosted.len() {
            match host_of.get(&root) {
                Some(host) => {
                    let outer = root_of(*host);
                    if !host_of.contains_key(&outer) {
                        break;
                    }
                    root = outer;
                    d += 1;
                }
                None => break,
            }
        }
        d
    };
    let mut ordered: Vec<(usize, (N, N))> = hosted.iter().map(|&p| (depth(p.0), p)).collect();
    ordered.sort_by_key(|(d, _)| *d);
    ordered.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Roots: `app` (ordinary), screens `s1`/`s2` hosted in the outer
    /// outlet `o1` (inside `app`), and `n1` hosted in the inner outlet
    /// `o2`, which lives inside `s2`. `root_of` follows that tree.
    fn root_of(node: &'static str) -> &'static str {
        match node {
            "o1" => "app",
            "o2" => "s2",
            other => other,
        }
    }

    #[test]
    fn a_screen_nested_in_a_hosted_screen_is_computed_after_it() {
        // Registry order is a HashMap's: put the inner one first.
        let hosted = [("n1", "o2"), ("s2", "o1"), ("s1", "o1")];
        let order: Vec<_> = hosted_root_order(&hosted, root_of).into_iter().map(|p| p.0).collect();
        let pos = |r| order.iter().position(|x| *x == r).unwrap();
        assert!(pos("s2") < pos("n1"), "n1's host is laid out by s2's pass: {order:?}");
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn a_self_hosting_registry_terminates() {
        let hosted = [("a", "b"), ("b", "a")];
        assert_eq!(hosted_root_order(&hosted, |n| n).len(), 2);
    }
}
