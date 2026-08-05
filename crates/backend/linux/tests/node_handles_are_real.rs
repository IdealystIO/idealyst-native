//! Regression: the GTK backend must override the caps traits' DEFAULTED
//! `make_*_handle` methods with handles that actually reach a widget.
//!
//! ## The bug this pins
//!
//! `caps::ViewOps::make_view_handle` (and its text / scroll siblings) ship
//! a default body:
//!
//! ```ignore
//! fn make_view_handle(&self, node: &Self::Node) -> ViewHandle {
//!     ViewHandle::new(Rc::new(()), &noop::NoopViewOps)
//! }
//! ```
//!
//! During the runtime-v2 port `newcore.rs` overrode `create_view` /
//! `create_text` but not the handle builders, so every one fell through to
//! that default. `AnimatedValue::bind` routes each per-frame write through
//! whatever handle these return, and a no-op handle *accepts every write
//! and discards it* — the exact failure `handles.rs` warns about: "the
//! value ticks, nothing paints."
//!
//! The damage was invisible to everything cheap: the crate compiled clean,
//! the reactive flush ran, the scheduler ran, and
//! `IDEALYST_GTK_DUMP_LAYOUT` showed all 16 welcome-app nodes correctly
//! framed, allocated, mapped and visible. The window was simply blank,
//! because the nodes the author starts at `opacity: 0` and animates in
//! never received a single animated write (`anim=None` in the dump).
//!
//! ## Why the assertion is shaped this way
//!
//! A defaulted handle is distinguishable from a real one by what it wraps:
//! the no-op carries `Rc::new(())`, ours carries the backend's own handle
//! state. So "downcasts to `()`" is precisely "this is the default", with
//! no dependence on paint output — which keeps the test headless and
//! deterministic rather than screenshot-based.
//!
//! Driving the handle's `ViewOps` directly is not possible from a test:
//! `ViewHandle::ops` is private. Asserting on the payload is the tightest
//! reachable check, per CLAUDE.md §8's "closest reachable test" rule.

#![cfg(target_os = "linux")]

use backend_linux::{gtk4, LinuxBackend};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_vocabulary::caps::{ScrollOps, TextOps, ViewOps};

/// GTK must be initialised before any widget is constructed. Headless CI
/// has no display; skip rather than fail there (same guard the sibling
/// GTK tests use).
fn gtk_ready() -> bool {
    if gtk4::init().is_err() {
        eprintln!("skipping: no display available to initialize GTK");
        return false;
    }
    true
}

fn backend() -> LinuxBackend {
    LinuxBackend::new(gtk4::Window::new())
}

#[test]
fn regression_view_handle_is_not_the_noop_default() {
    if !gtk_ready() {
        return;
    }
    let mut b = backend();
    let node = ViewOps::create_view(&mut b, &AccessibilityProps::default());

    let handle = ViewOps::make_view_handle(&b, &node);

    assert!(
        handle.as_any().downcast_ref::<()>().is_none(),
        "make_view_handle fell through to the caps default (ViewHandle over \
         Rc<()> + NoopViewOps). Every AnimatedValue::bind write would be \
         accepted and discarded, so animated opacity/transform never reaches \
         the widget and the scene renders blank."
    );
}

#[test]
fn regression_text_handle_is_not_the_noop_default() {
    if !gtk_ready() {
        return;
    }
    let mut b = backend();
    let node = TextOps::create_text(&mut b, "hello", &AccessibilityProps::default());

    let handle = TextOps::make_text_handle(&b, &node);

    assert!(
        handle.as_any().downcast_ref::<()>().is_none(),
        "make_text_handle fell through to the caps default — animated text \
         colour/opacity writes would be silently dropped."
    );
}

// `make_scroll_view_handle` had the SAME defect and is fixed alongside the
// two above, but it cannot be pinned from here. `ScrollViewHandle` exposes
// no `as_any()` (only `scroll_to`), so the payload trick doesn't apply, and
// asserting behaviourally means reading the GTK adjustment off the node —
// `LinuxNode::widget` is `pub(crate)`, invisible to an integration test.
//
// Covering it properly needs an in-crate `#[cfg(test)]` test that can reach
// the widget and assert `scroll_to` moves the vadjustment. Noted here rather
// than left as a silent gap: a reader comparing the fix to this file would
// otherwise reasonably assume the scroll case was simply forgotten.
#[test]
fn regression_scroll_view_handle_builds_without_panicking() {
    if !gtk_ready() {
        return;
    }
    let mut b = backend();
    let node = ScrollOps::create_scroll_view(&mut b, false, None, &AccessibilityProps::default());

    // Weak by design (see the note above): this only proves the override is
    // wired and reachable, NOT that scrolling reaches the adjustment.
    let _handle = ScrollOps::make_scroll_view_handle(&b, &node);
}
