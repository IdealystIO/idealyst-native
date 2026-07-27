//! `StyleSource::Preminted` — the build-time-minted class path.
//!
//! Stage 1 of the preminted-styles plan: the walker must stamp the
//! class via `Backend::attach_html_class` and do **no** style-engine
//! work (no `ApplyStyle`, no cohort registration), unless runtime
//! overrides are present — in which case exactly the override layer
//! goes through the normal static-application path on top of the
//! class.

#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use mock_backend::Event;
use runtime::TestRuntime;
use runtime_core::{ui, Color, IntoElement, StyleRules, StyleSource, Tokenized};
use std::rc::Rc;

fn preminted_root(class: &'static str) -> runtime_core::Element {
    ui! { view { text { "hi" } } }
        .into_element()
        .with_style(StyleSource::Preminted { class: class.into(), overrides: None })
}

/// A preminted class is stamped on the node and the style engine is
/// never invoked for it.
#[test]
fn preminted_class_stamps_without_style_engine() {
    let rt = TestRuntime::new();
    let root = ui! { view { text { "hi" } } }
        .into_element()
        .with_style(StyleSource::Preminted { class: "iy-abc123".into(), overrides: None });
    let _owner = rt.render(root);

    let events = rt.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::AttachHtmlClass { class, .. } if class == "iy-abc123")),
        "preminted class was not stamped; events: {events:#?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, Event::ApplyStyle { .. })),
        "preminted path must not invoke the style engine; events: {events:#?}"
    );
}

/// Runtime slot overrides on a preminted node keep the class AND apply
/// the override rules as a normal static layer.
#[test]
fn preminted_overrides_layer_static_application() {
    let rt = TestRuntime::new();
    let overrides = StyleRules {
        background: Some(Tokenized::Literal(Color("#f00".into()))),
        ..Default::default()
    };
    let root = ui! { view { text { "hi" } } }
        .into_element()
        .with_style(StyleSource::Preminted {
            class: "iy-abc123".into(),
            overrides: Some(Rc::new(overrides)),
        });
    let _owner = rt.render(root);

    let events = rt.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::AttachHtmlClass { class, .. } if class == "iy-abc123")),
        "class must still be stamped when overrides are present; events: {events:#?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::ApplyStyle { .. })),
        "override rules must reach the backend via the static path; events: {events:#?}"
    );
}

/// `with_style_overrides` on an already-preminted element accumulates
/// into the variant's override slot (later layers win) instead of
/// discarding the class.
#[test]
fn with_style_overrides_composes_onto_preminted() {
    let rt = TestRuntime::new();
    let override_rules = Rc::new(StyleRules {
        background: Some(Tokenized::Literal(Color("#0f0".into()))),
        ..Default::default()
    });
    let root = ui! { view { text { "hi" } } }
        .into_element()
        .with_style(StyleSource::Preminted { class: "iy-base".into(), overrides: None })
        .with_style_overrides(override_rules);
    let _owner = rt.render(root);

    let events = rt.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::AttachHtmlClass { class, .. } if class == "iy-base")),
        "override composition must not drop the preminted class; events: {events:#?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::ApplyStyle { .. })),
        "composed overrides must apply; events: {events:#?}"
    );
}

/// The premint host driver: a fully-preminted app performs NO sheet
/// registrations, so the host-state flush that normally rides
/// `ensure_registered_with` (theme tokens, app background, default
/// text font) must reach the backend anyway — via the driver the
/// `Preminted` walker arm installs.
#[test]
fn preminted_app_receives_theme_state_without_sheet_registration() {
    // Queue theme state BEFORE render, exactly like `install_theme` at
    // app start: tokens pend until a backend is in scope.
    runtime_core::install_tokens(&[runtime_core::TokenEntry {
        name: "color-surface",
        value: runtime_core::TokenValue::Color(Color("#fff".into())),
    }]);
    runtime_core::set_app_background(Tokenized::Literal(Color("#eee".into())));
    runtime_core::set_default_text_font(Some(runtime_core::FontFamily::System(
        "Inter".into(),
    )));

    let rt = TestRuntime::new();
    let _owner = rt.render(preminted_root("iy-themed"));

    let events = rt.events();
    assert!(
        !events.iter().any(|e| matches!(e, Event::RegisterStylesheet { .. })),
        "a fully-preminted tree must register no sheets; events: {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::InstallThemeVariables { token_count: 1 })),
        "queued tokens must reach the backend without any sheet \
         registration; events: {events:#?}"
    );
    assert!(
        events.iter().any(
            |e| matches!(e, Event::ApplyDefaultTextFont { family: Some(f) } if f == "Inter")
        ),
        "the theme's default text font must publish at the document \
         level for preminted classes; events: {events:#?}"
    );

    // Cleanup the thread-global font slot for hygiene (tests share
    // nothing else; tokens/bg were drained by the flush above).
    runtime_core::set_default_text_font(None);
}

/// A theme swap AFTER mount (update_tokens) re-fires the driver — the
/// version-signal subscription is what keeps a preminted app themable
/// at runtime.
#[test]
fn preminted_app_theme_swap_reaches_backend() {
    let rt = TestRuntime::new();
    let _owner = rt.render(preminted_root("iy-swap"));
    rt.backend_mut().clear_events();

    runtime_core::update_tokens(&[runtime_core::TokenEntry {
        name: "color-surface",
        value: runtime_core::TokenValue::Color(Color("#000".into())),
    }]);

    let events = rt.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::UpdateTokens { token_count: 1 })),
        "post-mount update_tokens must re-fire the premint host driver \
         and drain to the backend; events: {events:#?}"
    );
}
