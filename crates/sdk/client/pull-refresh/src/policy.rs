//! The decisions the native handlers share, un-gated so the tests run on
//! the host: how the refreshing state is synced onto the native control,
//! how the tint resolves, and which view in the child's tree is the
//! scroller the control attaches to.

use runtime_shared::color::{self, Rgba};
use runtime_shared::Color;

/// What to do to the native control to make it show `wanted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sync {
    /// Start showing the spinner (`beginRefreshing` / `setRefreshing(true)`).
    Begin,
    /// Stop showing it (`endRefreshing` / `setRefreshing(false)`).
    End,
    /// The control already shows `wanted`.
    Keep,
}

/// Compare the control's own state with the author's `refreshing` value.
///
/// The native control flips itself to refreshing when the user pulls, before
/// `on_refresh` runs, so after a pull `native` is `true` while `wanted` is
/// whatever the app made it. Reading the control rather than remembering
/// what we last told it is what makes that case come out right: an app that
/// never sets `refreshing` gets `End`, and the spinner does not hang.
pub(crate) fn sync(native: bool, wanted: bool) -> Sync {
    match (native, wanted) {
        (false, true) => Sync::Begin,
        (true, false) => Sync::End,
        _ => Sync::Keep,
    }
}

/// The tint as RGBA, or `None` for the platform's default color. A color
/// the parser cannot read also falls back to the default rather than to
/// black: a typo should not repaint the spinner.
pub(crate) fn tint_rgba(tint: Option<&Color>) -> Option<Rgba> {
    color::parse(&tint?.0).ok()
}

/// How the scroller search treats one view. Used by the iOS handler only:
/// Android decides at gesture time instead (`RustSwipeRefresh.kt`).
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Probe {
    /// A scroll view that scrolls vertically: the control attaches here.
    Scroller,
    /// A scroller that must not take the control — a text view (UIKit's
    /// `UITextView` is a `UIScrollView`) or one that scrolls sideways only.
    /// Its own subtree is not searched either: what sits inside a text
    /// view or a carousel is not the screen's list.
    Opaque,
    /// Anything else: search its children.
    Container,
}

/// The view the refresh control attaches to: the first [`Probe::Scroller`]
/// in pre-order over `roots` — the wrapped child itself first, then its
/// descendants, in child order.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn first_scroller<N: Clone>(
    roots: &[N],
    probe: impl Fn(&N) -> Probe,
    children: impl Fn(&N) -> Vec<N>,
) -> Option<N> {
    let mut stack: Vec<N> = roots.iter().rev().cloned().collect();
    while let Some(node) = stack.pop() {
        match probe(&node) {
            Probe::Scroller => return Some(node),
            Probe::Opaque => {}
            Probe::Container => {
                let mut kids = children(&node);
                kids.reverse();
                stack.extend(kids);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny view tree: `(name, probe, children)`.
    #[derive(Clone)]
    struct V(&'static str, Probe, Vec<V>);

    fn find(roots: &[V]) -> Option<&'static str> {
        first_scroller(roots, |v| v.1, |v| v.2.clone()).map(|v| v.0)
    }

    fn leaf(name: &'static str) -> V {
        V(name, Probe::Container, vec![])
    }

    #[test]
    fn a_pull_the_app_ignores_ends_the_spinner() {
        // The user pulled (the control is refreshing) and on_refresh left
        // `refreshing` false: the spinner must end, not hang.
        assert_eq!(sync(true, false), Sync::End);
    }

    #[test]
    fn a_pull_the_app_takes_up_keeps_the_spinner() {
        assert_eq!(sync(true, true), Sync::Keep);
    }

    #[test]
    fn a_programmatic_refresh_begins_the_spinner() {
        assert_eq!(sync(false, true), Sync::Begin);
        assert_eq!(sync(false, false), Sync::Keep);
    }

    #[test]
    fn tint_parses_css_colors_and_falls_back_to_the_default() {
        assert_eq!(tint_rgba(None), None);
        assert_eq!(tint_rgba(Some(&Color("#ff0000".into()))), Some(Rgba::new(255, 0, 0, 255)));
        assert_eq!(tint_rgba(Some(&Color("not a color".into()))), None);
    }

    #[test]
    fn the_child_itself_is_the_scroller_when_it_scrolls() {
        let list = V("list", Probe::Scroller, vec![leaf("row")]);
        assert_eq!(find(&[list]), Some("list"));
    }

    #[test]
    fn the_first_scroller_below_a_wrapper_is_found_in_child_order() {
        let tree = V(
            "screen",
            Probe::Container,
            vec![
                V("header", Probe::Container, vec![leaf("title")]),
                V("body", Probe::Container, vec![V("list", Probe::Scroller, vec![])]),
                V("second", Probe::Scroller, vec![]),
            ],
        );
        assert_eq!(find(&[tree]), Some("list"));
    }

    #[test]
    fn text_views_and_sideways_scrollers_are_skipped_with_their_subtrees() {
        let tree = V(
            "screen",
            Probe::Container,
            vec![
                V("notes", Probe::Opaque, vec![V("inner", Probe::Scroller, vec![])]),
                V("chips", Probe::Opaque, vec![]),
                V("list", Probe::Scroller, vec![]),
            ],
        );
        assert_eq!(find(&[tree]), Some("list"));
    }

    #[test]
    fn a_child_with_no_scroller_finds_nothing() {
        let tree = V("card", Probe::Container, vec![leaf("a"), leaf("b")]);
        assert_eq!(find(&[tree]), None);
        assert_eq!(find(&[]), None);
    }

    #[test]
    fn several_roots_are_searched_in_order() {
        assert_eq!(
            find(&[leaf("spacer"), V("list", Probe::Scroller, vec![])]),
            Some("list")
        );
    }
}
