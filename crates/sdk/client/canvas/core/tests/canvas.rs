//! `Canvas(CanvasProps { .. }).with_style(…)` mounted through the scene
//! registry on the shared `host-mock` recording substrate, using the
//! renderer-agnostic SSR/hydration host handler (`register_ssr` —
//! bare `<canvas>` + author style).
//!
//! The renderer-specific handlers (canvas-native web Canvas2D) are
//! `WebBackend`-concrete and covered by canvas-native's wasm32 check
//! gate; the placeholder arm is pinned in canvas-native's own tests.

use canvas_core::{Canvas, CanvasProps};
use host_mock::Harness;
use runtime_shared::{Length, StyleRules};
use runtime_scene::Realized;
use runtime_vocabulary::glue::IntoElement;

fn harness() -> Harness {
    Harness::with_registry(|r| canvas_core::register_ssr(r))
}

#[test]
fn ssr_host_mounts_a_real_canvas_element() {
    let h = harness();
    let el = Canvas(CanvasProps::default()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops();
    assert_eq!(
        log.first().map(String::as_str),
        Some("create n0 element \"canvas\""),
        "the SSR host must emit a real <canvas> (hydration adopts it): {log:?}"
    );
}

/// Regression for the "bare canvas doesn't fill" papercut (Whiteboard Pro
/// feedback): an unstyled `Canvas(...)` carries the shared fill-parent
/// sheet, which the handler attaches to the `<canvas>` node. Without it a
/// bare canvas collapses to a 0×0 box on native.
#[test]
fn unstyled_canvas_defaults_to_fill_parent() {
    let h = harness();
    let el = Canvas(CanvasProps::default()).into_element();
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops();
    assert!(
        log.iter()
            .any(|l| l.starts_with("apply_style n0") && l.contains("Percent(100.0)")),
        "the fill default (100% width) must land on the canvas node: {log:?}"
    );
}

/// An explicit `.with_style(...)` REPLACES the fill default — authors who
/// size the canvas themselves aren't fighting a baked-in 100%.
#[test]
fn explicit_style_overrides_fill_default() {
    let h = harness();
    let mut fixed = StyleRules::default();
    fixed.width = Some(Length::Px(120.0).into());
    let el = Canvas(CanvasProps::default())
        .with_style(fixed)
        .into_element();
    let _realized: Realized<u32> = h.mount(el);

    let log = h.ops();
    assert!(
        log.iter()
            .any(|l| l.starts_with("apply_style n0") && l.contains("Px(120.0)")),
        "the author sheet must win: {log:?}"
    );
    assert!(
        !log.iter().any(|l| l.contains("Percent(100.0)")),
        "the fill default must be REPLACED, not merged: {log:?}"
    );
}

#[test]
fn teardown_releases_the_external_node() {
    let h = harness();
    let el = Canvas(CanvasProps::default()).into_element();
    let realized: Realized<u32> = h.mount(el);
    let _ = h.take_log();

    drop(realized);
    let log = h.ops();
    assert!(
        log.iter().any(|l| l == "release_external n0"),
        "unmount must release the canvas node (teardown-guard contract): {log:?}"
    );
}
