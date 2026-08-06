//! Breakpoint overlays must RE-APPLY when the viewport crosses a
//! breakpoint, on backends that don't handle style variants natively.
//!
//! # THESE TESTS ARE `#[ignore]`d — THEY PIN A KNOWN, UNFIXED GAP
//!
//! Responsive resize does not work on ANY runtime-v2 native backend, and
//! cannot until the viewport signal is migrated to `runtime-world`.
//!
//! `__bp_*` overlays are static styling. Web receives every overlay and
//! lets `@media` switch between them, so resizing re-pins for free. Every
//! other backend — GTK, Win32, terminal, CPU, the GPU hosts — resolves via
//! `merge_active_breakpoints`, which reads `current_breakpoint()` and BAKES
//! the winner into the node's rules at apply time. Re-applying therefore
//! requires the node to be subscribed to the viewport.
//!
//! It cannot be. `viewport.rs` holds
//! `static VIEWPORT: OnceCell<Signal<ViewportSize>>` where `Signal` is
//! `crate::reactive::Signal` — the LEGACY thread-local arena. v2 effects
//! live in `runtime-world`. A `runtime_world::effect` reading
//! `viewport_size().get()` registers no dependency at all, so no amount of
//! wrapping the style resolution in an effect will help.
//!
//! Measured, not assumed: an effect reading the raw viewport signal fired
//! once and never again across `set_viewport_size` + `world.flush()`, while
//! the identical effect over an ordinary `runtime_world::signal` re-ran
//! correctly. Same result reading `current_breakpoint()`, and same result
//! with the signal created inside `world.enter(...)` — so it is the arena,
//! not scope or world affinity.
//!
//! What DOES work today (and is fixed): boot-time correctness. host-gtk
//! seeds the viewport before the first style pass, so a window that opens
//! wide pins its sidebar and one that opens narrow collapses it. Only
//! CROSSING a breakpoint after boot is dead.
//!
//! Un-ignore these once `VIEWPORT` is a `runtime_world` signal; the second
//! half of the fix is to resolve the non-native branch of
//! `apply_sheet` inside an effect so the subscription exists.

use std::cell::RefCell;
use std::rc::Rc;

use host_mock::Harness;
use runtime_shared::{Length, StyleRules, StyleSheet, ViewportSize};
use runtime_vocabulary::builders::view;

/// Base `width: 100`; `md` → 500; `lg` → 900. `lg` is declared FIRST so a
/// resolver that merely preserved declaration order would fail.
fn responsive_view() -> runtime_scene::Element {
    let sheet = Rc::new(
        StyleSheet::new(|_vs| StyleRules {
            width: Some(Length::Px(100.0).into()),
            ..Default::default()
        })
        .variant("__bp_lg", "on", |_vs| StyleRules {
            width: Some(Length::Px(900.0).into()),
            ..Default::default()
        })
        .variant("__bp_md", "on", |_vs| StyleRules {
            width: Some(Length::Px(500.0).into()),
            ..Default::default()
        }),
    );
    view().style(sheet).build()
}

type Applied = Rc<RefCell<Vec<StyleRules>>>;

fn capture(h: &Harness) -> Applied {
    let applied: Applied = Rc::new(RefCell::new(Vec::new()));
    let a = applied.clone();
    h.set_style_line(move |n, r| {
        a.borrow_mut().push(r.clone());
        format!("apply_style n{n}")
    });
    applied
}

fn last_width(applied: &Applied) -> Option<Length> {
    applied
        .borrow()
        .last()
        .and_then(|r| r.width.clone())
        .and_then(|w| match w {
            runtime_shared::Tokenized::Literal(l) => Some(l),
            _ => None,
        })
}

#[test]
#[ignore = "viewport signal is on the legacy arena; v2 effects cannot subscribe (see module docs)"]
fn regression_breakpoint_overlay_reapplies_when_viewport_grows() {
    let h = Harness::new();
    h.shared.handles_states_natively.set(false);
    runtime_shared::set_viewport_size(ViewportSize::new(400.0, 800.0));
    let applied = capture(&h);
    let realized = h.mount(responsive_view());

    assert_eq!(
        last_width(&applied),
        Some(Length::Px(100.0)),
        "a 400px viewport is below `md`, so the base width applies"
    );

    // Cross into `lg`. The publish stages the signal; the flush commits it,
    // which is what re-runs the style effect (a staged write alone would
    // leave the node on its boot-time styling — that is the GTK bug).
    runtime_shared::set_viewport_size(ViewportSize::new(1200.0, 800.0));
    h.world.flush();

    assert_eq!(
        last_width(&applied),
        Some(Length::Px(900.0)),
        "crossing into `lg` must RE-APPLY the overlay. Still seeing 100 means \
         the style resolved once outside any reactive scope, so the node is \
         frozen at whatever breakpoint was active when it first mounted."
    );

    drop(realized);
}

#[test]
#[ignore = "viewport signal is on the legacy arena; v2 effects cannot subscribe (see module docs)"]
fn regression_breakpoint_overlay_reapplies_when_viewport_shrinks() {
    let h = Harness::new();
    h.shared.handles_states_natively.set(false);
    runtime_shared::set_viewport_size(ViewportSize::new(1200.0, 800.0));
    let applied = capture(&h);
    let realized = h.mount(responsive_view());

    assert_eq!(
        last_width(&applied),
        Some(Length::Px(900.0)),
        "a 1200px viewport is at `lg`"
    );

    // Shrink below `md`. The shrink direction is tested separately because
    // this is the face the user hit second: after the boot-time seed fix,
    // the sidebar pinned correctly at wide but would not collapse.
    runtime_shared::set_viewport_size(ViewportSize::new(400.0, 800.0));
    h.world.flush();

    assert_eq!(
        last_width(&applied),
        Some(Length::Px(100.0)),
        "shrinking below every breakpoint must fall back to the base rules"
    );

    drop(realized);
}

/// A node with NO breakpoint blocks must not subscribe to the viewport —
/// the reactive re-apply is scoped to nodes that actually declare them, so
/// a tree of plain nodes pays no effect and no re-styling churn.
#[test]
fn plain_node_does_not_restyle_on_viewport_change() {
    runtime_shared::set_viewport_size(ViewportSize::new(400.0, 800.0));

    let h = Harness::new();
    h.shared.handles_states_natively.set(false);
    let applied = capture(&h);
    let plain = Rc::new(StyleSheet::new(|_vs| StyleRules {
        width: Some(Length::Px(42.0).into()),
        ..Default::default()
    }));
    let realized = h.mount(view().style(plain).build());

    let before = applied.borrow().len();
    runtime_shared::set_viewport_size(ViewportSize::new(1200.0, 800.0));
    h.world.flush();

    assert_eq!(
        applied.borrow().len(),
        before,
        "a sheet with no `__bp_*` overlays must not re-apply on resize"
    );

    drop(realized);
}
