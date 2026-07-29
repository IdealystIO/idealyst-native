//! Regression pins for idea-ui's per-component prim gating.
//!
//! The contract (mirrors `runtime-core/tests/prim_gating.rs`): a
//! component compiles ONLY when every primitive family it transitively
//! renders is enabled, so a restricted app gets a compile error naming
//! the missing `prim-*` feature instead of a runtime "not supported"
//! placeholder. Default features keep the full set — this file's
//! default-run arm pins that. The OFF arms compile (and run) under
//! `cargo test -p idea-ui --no-default-features [--features …]`, which
//! CI/manual verification uses alongside the per-family check matrix.

/// Default build: every family is on and every gated component's props
/// type exists. Constructing the props (not just naming the type)
/// proves the component fn + `Default` glue compiled, one per family
/// plus the multi-family closures.
#[cfg(all(
    feature = "prim-icon",
    feature = "prim-image",
    feature = "prim-text-input",
    feature = "prim-activity",
    feature = "prim-portal",
    feature = "prim-presence",
))]
mod defaults_on {
    #[test]
    fn default_features_compile_every_gated_component() {
        // Several props `Default`s create signals (world-ambient on the
        // new core) — identity wrapper on the old core.
        idea_theme::testing::with_test_world(|| {
        // Single-family components.
        let _ = idea_ui::IconProps::default(); // prim-icon
        let _ = idea_ui::ImageProps::default(); // prim-image
        let _ = idea_ui::AvatarProps::default(); // prim-image (image_from)
        let _ = idea_ui::TextareaProps::default(); // prim-text-input
        let _ = idea_ui::SpinnerProps::default(); // prim-activity
        let _ = idea_ui::PopoverProps::default(); // prim-portal
        // Multi-family closures.
        let _ = idea_ui::ButtonProps::default(); // icon + activity
        let _ = idea_ui::SelectProps::default(); // icon + portal
        let _ = idea_ui::ModalProps::default(); // portal + presence
        let _ = idea_ui::FieldProps::default(); // icon + activity + text-input
        let _ = idea_ui::AutocompleteProps::default(); // text-input + portal
        let _ = idea_ui::ToastHostProps::default(); // icon+activity+portal+presence
        });
    }
}

/// All families off: the ungated core set must still be fully usable.
/// This arm is what `cargo test -p idea-ui --no-default-features`
/// exercises — it fails to COMPILE if an "ungated" component secretly
/// grows a gated-primitive dependency, which is the regression this
/// file exists to catch.
#[cfg(not(any(
    feature = "prim-icon",
    feature = "prim-image",
    feature = "prim-text-input",
    feature = "prim-activity",
    feature = "prim-portal",
    feature = "prim-presence",
)))]
mod all_off {
    #[test]
    fn core_components_survive_with_every_family_off() {
        let _ = idea_ui::CardProps::default();
        let _ = idea_ui::ChipProps::default();
        let _ = idea_ui::TagProps::default();
        let _ = idea_ui::BadgeProps::default();
        let _ = idea_ui::DividerProps::default();
        let _ = idea_ui::ProgressProps::default();
        let _ = idea_ui::SkeletonProps::default();
        let _ = idea_ui::TypographyProps::default();
        let _ = idea_ui::TableProps::default();
    }
}

/// Textarea needs ONLY prim-text-input — it shares the field module's
/// stylesheets but not the Field component (which additionally needs
/// icon + activity). Compiled under
/// `--no-default-features --features prim-text-input`; regresses if the
/// module-level gate ever re-widens to Field's full requirement set.
#[cfg(all(
    feature = "prim-text-input",
    not(feature = "prim-icon"),
    not(feature = "prim-activity"),
))]
mod regression_textarea_without_field {
    #[test]
    fn textarea_compiles_without_icon_or_activity() {
        let _ = idea_ui::TextareaProps::default();
        // The style-level types stay reachable too (Textarea's size axis).
        let _ = idea_ui::FieldSize::default();
    }
}

/// Autocomplete needs text-input + portal but NOT icon — it reuses
/// `SelectOption`/`SelectSize` (data/style types) from the select
/// module without the icon-rendering Select component. Compiled under
/// `--no-default-features --features prim-text-input,prim-portal`.
#[cfg(all(
    feature = "prim-text-input",
    feature = "prim-portal",
    not(feature = "prim-icon"),
))]
mod regression_autocomplete_without_select {
    #[test]
    fn autocomplete_compiles_without_icon() {
        let _ = idea_ui::AutocompleteProps::default();
        let _ = idea_ui::SelectOption::new("value", "label");
    }
}
