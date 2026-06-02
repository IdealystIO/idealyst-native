//! Regression: `#[derive(IdealystSchema)]` and `#[idealyst_tool]` must
//! RESOLVE and expand to a no-op without the `catalog` / `strict-docs`
//! features — exactly like `#[component]`. This is what lets SDKs,
//! idea-ui, and app code annotate props / tools *unconditionally*
//! (so the catalog can document them) without breaking a normal
//! production build, which compiles with neither feature.
//!
//! This file is built with runtime-core's DEFAULT features (no catalog).
//! If it compiles and runs, the no-op path works: the macros are
//! importable, the `#[schema(...)]` helper attribute is accepted and
//! inert, and the derive adds no fields/behavior. Before the
//! always-export fix, `use runtime_core::IdealystSchema` failed here with
//! `unresolved import` — the bug this guards against.

use runtime_core::{idealyst_tool, IdealystSchema};

/// The shape an SDK external-primitive props struct would have:
/// documented fields plus a constraint hint.
#[derive(Default, IdealystSchema)]
struct WidgetProps {
    /// The widget's visible label.
    #[schema(constraint = "max 40 chars")]
    label: String,
    /// Whether the widget starts expanded.
    expanded: bool,
}

/// A documented enum prop type.
#[derive(IdealystSchema)]
#[allow(dead_code)]
enum Tone {
    /// The primary tone.
    Primary,
    /// The secondary tone.
    Secondary,
}

/// A standalone function annotated as a tool.
#[idealyst_tool]
fn bump(x: i32) -> i32 {
    x + 1
}

#[test]
fn schema_and_tool_macros_are_noops_without_catalog() {
    // The derive added no fields and changed no behavior.
    let w = WidgetProps::default();
    assert_eq!(w.label, "");
    assert!(!w.expanded);
    let _ = Tone::Primary;
    // The tool attribute left the function untouched.
    assert_eq!(bump(1), 2);
}
