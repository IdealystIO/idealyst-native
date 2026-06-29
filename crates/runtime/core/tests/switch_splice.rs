//! Regression: `Element::Switch` (the `ui!` `match` lowering and the typed
//! `switch()` builder) must take the ANCHORLESS splice path on backends that
//! support child splicing — the active arm mounts DIRECTLY into the parent,
//! with no `create_reactive_anchor` wrapper.
//!
//! Why it matters: the anchored wrapper is a real native box that auto-sizes
//! to its in-flow child. Under a parent that centers its cross axis
//! (`align_items: center` — e.g. the idea-ui docs `DemoSurface`), that wrapper
//! hugs the arm's content and centers it, so a full-width-intended arm (an
//! idea-ui `Field`, which fills its container) collapsed to its content width
//! on macOS/iOS/Android — while web's `display:contents` anchor let the same
//! arm fill. `When` already spliced; `Switch` did not until this fix, so a
//! `Field` rebuilt inside a `switch` (e.g. a password field toggling `secure`)
//! stayed icon-width on native.
//!
//! Standalone `#[path]` includes (not `mod common`): this binary only needs
//! `mock_backend` + `runtime`, so it pulls those two directly — mirrors
//! `text_input_blur.rs`.

#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use mock_backend::{Event, MockBackendConfig};
use runtime::TestRuntime;
use runtime_core::{signal, ui, Element, Signal};

fn count_text(events: &[Event], needle: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { content } if content == needle))
        .count()
}

#[test]
fn switch_splices_active_arm_when_backend_supports_it() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        supports_child_splice: true,
        ..Default::default()
    });
    let mode: Signal<u32> = signal!(0u32);
    let tree: Element = ui! {
        view {
            match mode.get() {
                0 => { text { "zero".to_string() } }
                1 => { text { "one".to_string() } }
                _ => { text { "other".to_string() } }
            }
        }
    };
    let _owner = rt.render(tree);

    // The arm rendered...
    assert_eq!(count_text(&rt.events(), "zero"), 1, "initial arm built");
    // ...with NO reactive-anchor wrapper — it spliced straight into the view,
    // so under a centering parent the arm would fill instead of hugging.
    assert!(
        !rt.events()
            .iter()
            .any(|e| matches!(e, Event::CreateReactiveAnchor)),
        "Switch must splice its arm into the parent (no anchor box) when the \
         backend supports child splicing: {:?}",
        rt.events()
    );

    // Re-keys on signal change: old arm removed via remove_child (the spliced
    // unmount), new arm built.
    rt.backend_mut().clear_events();
    mode.set(1);
    assert_eq!(count_text(&rt.events(), "one"), 1, "arm swapped on change");
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::RemoveChild { .. })),
        "old arm unmounted via remove_child (spliced region), not ClearChildren: {:?}",
        rt.events()
    );
}

#[test]
fn switch_uses_anchor_when_splice_unsupported() {
    // Default backend reports no splice support → the anchored path, which DOES
    // create a reactive anchor. Guards that the splice gate actually branches
    // (so the test above is meaningful, not vacuously anchor-free).
    let rt = TestRuntime::new();
    let mode: Signal<u32> = signal!(0u32);
    let tree: Element = ui! {
        view {
            match mode.get() {
                0 => { text { "zero".to_string() } }
                _ => { text { "other".to_string() } }
            }
        }
    };
    let _owner = rt.render(tree);
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::CreateReactiveAnchor)),
        "without splice support, Switch falls back to the anchored path: {:?}",
        rt.events()
    );
}
