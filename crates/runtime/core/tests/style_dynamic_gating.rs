//! Regression tests for the `style-dynamic` feature gate (preminted
//! styles, stage 4).
//!
//! The bug class being prevented: with `style-dynamic` disabled, a
//! dynamic style that still reaches the walker (a live-minted
//! `Static` application, a `Reactive` closure, a preminted node's
//! runtime overrides) must degrade to an UNSTYLED node — the tree
//! still mounts, nothing panics, and no style-engine backend hook
//! fires. `StyleSource::Preminted` class stamping and the premint
//! host driver (theme tokens → backend) must keep working: they are
//! exactly what the gated configuration exists for.
//!
//! Run the gated-off half with:
//!   cargo test -p runtime-core --no-default-features --test style_dynamic_gating

#[path = "common/mod.rs"]
mod common;

#[cfg(not(feature = "style-dynamic"))]
mod style_dynamic_gated_off {
    use super::common::mock_backend::Event;
    use super::common::runtime::TestRuntime;
    use runtime_core::{
        ui, Color, IntoElement, StyleApplication, StyleRules, StyleSheet, StyleSource, Tokenized,
    };
    use std::rc::Rc;

    fn red_static_source() -> StyleSource {
        let sheet = Rc::new(StyleSheet::r#static(StyleRules {
            background: Some(Tokenized::Literal(Color("#f00".into()))),
            ..Default::default()
        }));
        StyleSource::Static(StyleApplication::new(sheet))
    }

    /// A live-minted static style degrades to an unstyled node: the
    /// tree mounts, no panic, and the style engine's backend hooks
    /// (`ApplyStyle`, `RegisterStylesheet`) never fire.
    #[test]
    fn regression_gated_static_style_mounts_unstyled_without_panic() {
        let rt = TestRuntime::new();
        let root = ui! { view { text { "hi" } } }
            .into_element()
            .with_style(red_static_source());
        let _owner = rt.render(root);

        let events = rt.events();
        assert!(
            events.iter().any(|e| matches!(e, Event::CreateView { .. })),
            "the node itself must still mount; events: {events:#?}"
        );
        assert!(
            !events.iter().any(|e| matches!(
                e,
                Event::ApplyStyle { .. } | Event::RegisterStylesheet { .. }
            )),
            "gated-off dynamic style must not reach the engine hooks; events: {events:#?}"
        );
    }

    /// A reactive style closure degrades the same way — and, critically,
    /// never runs (its captured signals are not subscribed).
    #[test]
    fn regression_gated_reactive_style_mounts_unstyled_without_panic() {
        let rt = TestRuntime::new();
        let root = ui! { view { text { "hi" } } }
            .into_element()
            .with_style(StyleSource::Reactive(Box::new(|| {
                panic!(
                    "reactive style closure must not run with style-dynamic \
                     disabled — the walker degrades before evaluating it"
                )
            })));
        let _owner = rt.render(root);

        let events = rt.events();
        assert!(
            events.iter().any(|e| matches!(e, Event::CreateView { .. })),
            "the node itself must still mount; events: {events:#?}"
        );
    }

    /// Preminted class stamping — the whole point of the gated build —
    /// keeps working, including with overrides present (the class
    /// stays; only the override layer is dropped).
    #[test]
    fn regression_gated_preminted_class_still_stamps() {
        let rt = TestRuntime::new();
        let root = ui! { view { text { "hi" } } }
            .into_element()
            .with_style(StyleSource::Preminted {
                class: "iy-gated".into(),
                overrides: Some(Rc::new(StyleRules {
                    background: Some(Tokenized::Literal(Color("#0f0".into()))),
                    ..Default::default()
                })),
            });
        let _owner = rt.render(root);

        let events = rt.events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::AttachHtmlClass { class, .. } if class == "iy-gated")),
            "preminted class must stamp with style-dynamic off; events: {events:#?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, Event::ApplyStyle { .. })),
            "the override layer needs the live engine and must be dropped, \
             not half-applied; events: {events:#?}"
        );
    }

    /// The premint host driver is independent of the gate: theme tokens
    /// queued before render still reach the backend in the gated build.
    #[test]
    fn regression_gated_premint_host_driver_still_delivers_tokens() {
        runtime_core::install_tokens(&[runtime_core::TokenEntry {
            name: "color-surface",
            value: runtime_core::TokenValue::Color(Color("#fff".into())),
        }]);

        let rt = TestRuntime::new();
        let root = ui! { view { text { "hi" } } }
            .into_element()
            .with_style(StyleSource::Preminted { class: "iy-tok".into(), overrides: None });
        let _owner = rt.render(root);

        let events = rt.events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::InstallThemeVariables { token_count: 1 })),
            "premint host driver must deliver tokens in the gated build; events: {events:#?}"
        );
    }
}
