//! Component implementations. Each module exports a plain `fn`
//! plus the variant enums its stylesheet uses. Invocation macros
//! live in `crate::invocations` so all of them are `#[macro_export]`
//! at the crate root.

use std::rc::Rc;

use runtime_core::{
    text, when, AlignSelf, Element, IdealystSchema, IntoElement, IntoStyleSource, Reactive,
    StyleRules,
};
use runtime_vocabulary::StyleProp;

/// `StyleRules` that make an inline component HUG its content on the cross
/// axis instead of inheriting a flex parent's default `align-items: stretch`
/// (which would grow a Badge/Tag/Chip to the row's height and float its label
/// to the top — the same class of bug Button fixes). `Center` keeps the pill
/// sized to content while still letting a centering parent (a toolbar row, a
/// centered preview) center it. Attach as a `with_computed` layer.
pub(crate) fn hug_self() -> StyleRules {
    StyleRules { align_self: Some(AlignSelf::Center), ..Default::default() }
}

/// Render an optional, possibly-reactive text prop
/// (`Reactive<Option<String>>`) as an optional styled text node:
///
/// - `Static(None)` → `None` (no node — no layout slot for an absent
///   label).
/// - `Static(Some(s))` → a static text node.
/// - `Dynamic(f)` → a GUARDED hole: the styled text node mounts only
///   while `f()` is `Some` (the reactive mirror of the `Static(None)`
///   arm) and re-paints on content changes while mounted. It must NOT
///   render `""` while `None`: a mounted empty text is not invisible —
///   it keeps its line box and a slot in the parent's gap, which held
///   every typed input (their parse-error channel is always `Dynamic`)
///   visibly taller than a plain `Field`.
///
/// Shared by the components with an optional text prop (Switch/Field
/// `label`, Alert `body`). Coercion is uniform: a call-site
/// `label = Some("x".to_string())` or `label = None` lands here via the
/// `ui!`/`jsx!` dispatch's per-field `.into()` (blanket
/// `From<Option<String>>`), and a `Signal<Option<String>>` /
/// `rx!(Some(...))` arrives `Dynamic`.
pub(crate) fn optional_reactive_text(
    content: Reactive<Option<String>>,
    style: impl IntoStyleSource,
) -> Option<Element> {
    match content {
        Reactive::Static(None) => None,
        Reactive::Static(Some(s)) => Some(text(s).with_style(style).into_element()),
        Reactive::Dynamic(f) => {
            // The branch builder re-fires on every remount, so the one-shot
            // style is normalized to a `StyleProp` up front and re-applied
            // per mount via `reapply_style_prop` — the value shapes clone
            // (a static/preminted attach stays static, no engine dragged
            // in); the closure shapes re-wrap around the shared source.
            let src = Rc::new(style.into_style_prop());
            let cond = {
                let f = f.clone();
                move || f().is_some()
            };
            Some(when(
                cond,
                move || {
                    let content = f.clone();
                    text(move || content().unwrap_or_default())
                        .with_style(reapply_style_prop(&src))
                        .into_element()
                },
                runtime_core::empty_absolute_view,
            ))
        }
    }
}

/// Re-issue a shared [`StyleProp`] for another mount of the same node
/// shape (the guarded arm of [`optional_reactive_text`], whose branch
/// builder runs once per remount but received its style exactly once).
/// Value shapes clone; closure shapes get a fresh closure delegating to
/// the shared original, preserving each shape's attach path (a
/// `Preminted` stamp must NOT degrade to a `Dynamic` re-resolve).
fn reapply_style_prop(src: &Rc<StyleProp>) -> StyleProp {
    match &**src {
        StyleProp::Static(rules) => StyleProp::Static(rules.clone()),
        StyleProp::Sheet(app) => StyleProp::Sheet(app.clone()),
        StyleProp::Dynamic(_) => {
            let src = src.clone();
            StyleProp::Dynamic(Box::new(move || match &*src {
                StyleProp::Dynamic(g) => g(),
                _ => unreachable!("shape checked by reapply_style_prop"),
            }))
        }
        StyleProp::SheetDynamic(_) => {
            let src = src.clone();
            StyleProp::SheetDynamic(Box::new(move || match &*src {
                StyleProp::SheetDynamic(g) => g(),
                _ => unreachable!("shape checked by reapply_style_prop"),
            }))
        }
        StyleProp::Preminted { class, overrides, inline } => StyleProp::Preminted {
            class: class.clone(),
            overrides: overrides.clone(),
            inline: inline.clone(),
        },
        StyleProp::PremintedDynamic { overrides, .. } => {
            let overrides = overrides.clone();
            let src = src.clone();
            StyleProp::PremintedDynamic {
                class_of: Box::new(move || match &*src {
                    StyleProp::PremintedDynamic { class_of, .. } => class_of(),
                    _ => unreachable!("shape checked by reapply_style_prop"),
                }),
                overrides,
            }
        }
        // No optional-text caller hands a `signal_class` (or any future
        // shape) here; fail loudly rather than silently dropping styling.
        _ => panic!("optional_reactive_text: unsupported style prop shape"),
    }
}

/// Size scale shared by the selection controls (Switch, Checkbox,
/// Radio) and the linear Progress bar. These have intrinsic
/// dimensions rather than the Button family's padding/font scale, so
/// they use this closed enum instead of the `ButtonSize` trait. The
/// string keys match the `size` arms generated by the matching
/// idea-theme sheet builders.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema)]
pub enum ControlSize {
    /// Small.
    Sm,
    /// Medium (default).
    #[default]
    Md,
    /// Large.
    Lg,
}

impl ControlSize {
    /// The `size` variant-axis key for this scale.
    pub fn as_variant_str(self) -> &'static str {
        match self {
            ControlSize::Sm => "sm",
            ControlSize::Md => "md",
            ControlSize::Lg => "lg",
        }
    }
}

// `VariantEnum` lets the docs-app `DocControls` derive render a size
// picker; the inherent `as_variant_str` above is what components call.
impl runtime_core::VariantEnum for ControlSize {
    fn as_variant_str(self) -> &'static str {
        match self {
            ControlSize::Sm => "sm",
            ControlSize::Md => "md",
            ControlSize::Lg => "lg",
        }
    }
    fn all_variants() -> &'static [Self] {
        &[ControlSize::Sm, ControlSize::Md, ControlSize::Lg]
    }
}

#[cfg(test)]
mod control_size_tests {
    use super::ControlSize;

    #[test]
    fn variant_keys_match_sheet_builder_size_arms() {
        // These strings MUST equal the `size` arm keys generated by the
        // idea-theme sheet builders (SWITCH_TRACK_DIMS / CHECKBOX_DIMS /
        // RADIO_DIMS / PROGRESS_DIMS), or the component would request a
        // nonexistent arm and silently fall back to the default size.
        assert_eq!(ControlSize::Sm.as_variant_str(), "sm");
        assert_eq!(ControlSize::Md.as_variant_str(), "md");
        assert_eq!(ControlSize::Lg.as_variant_str(), "lg");
        assert_eq!(ControlSize::default(), ControlSize::Md);
    }
}

// The catalog scope every component in this module tree belongs to (by
// ambient module proximity — `idea_ui::components` is an ancestor of
// `idea_ui::components::*`). No-op without the `catalog` feature.
runtime_core::doc_scope!(
    Components = "Components",
    slug = "components",
    docs = "idea-ui's component library — buttons, inputs, layout, overlays, feedback, and data-display widgets, all cross-platform."
);

pub mod alert;
pub mod autocomplete;
pub mod avatar;
pub mod badge;
pub mod breadcrumbs;
pub mod checkbox;
pub mod grid;
pub mod image;
pub mod link;
pub mod list;
pub mod menu;
/// Shared scrolling panel for the anchored-menu family (Select/Menu/SubMenu/
/// Autocomplete). Internal — not a component; see `menu_panel::scrolling_menu_panel`.
pub(crate) mod menu_panel;
pub mod pagination;
pub mod progress;
pub mod radio;
pub mod textarea;
pub mod toast;
pub mod tooltip;
pub mod button;
pub mod calendar;
pub mod date_input;
pub mod date_picker;
pub mod time_input;
/// Shared parse/commit/normalize wiring for the typed value inputs
/// (TimeInput/DateInput/DateTimeInput). Internal — not a component.
pub(crate) mod typed_field;
pub mod card;
pub mod center;
pub mod chip;
pub mod collapsible;
pub mod divider;
// Hosts the Field component AND the shared input/help stylesheets
// Textarea renders with.
pub mod field;
pub mod icon;
pub mod icon_button;
pub mod modal;
pub mod popover;
pub mod segmented_control;
// Hosts the Select component AND the `SelectOption` / `SelectSize`
// data/style types Autocomplete reuses.
pub mod select;
pub mod skeleton;
pub mod slider;
pub mod spacer;
pub mod spinner;
pub mod stack;
pub mod surface;
pub mod switch;
#[cfg(feature = "table")]
pub mod table;
pub mod tabs;
pub mod tag;
pub mod typography;

// `Icon` is re-exported here (rather than only at the crate root in
// lib.rs) so it's reachable as `crate::components::Icon`. The crate-root
// `idea_ui::Icon` alias lives in lib.rs alongside the other components.
pub use icon::{Icon, IconProps};
