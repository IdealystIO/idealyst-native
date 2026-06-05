//! `Chip` — a selectable pill toggle.
//!
//! Where [`Badge`](super::badge::Badge) is a static status pill and
//! [`Tag`](super::tag::Tag) is a removable one, `Chip` is the *selectable*
//! member of the family: tapping it reports a select, and its `selected`
//! flag drives a distinct on/off appearance. It's the everyday building
//! block for filter rows, choice chips, and multi-select toggles.
//!
//! ```ignore
//! let on = signal!(false);
//! let on_select: Rc<dyn Fn()> = Rc::new(move || on.set(!on.get()));
//! ui! {
//!     Chip(
//!         label = "Rust",
//!         selected = on.get(),
//!         on_select = Some(on_select.clone()),
//!         tone = tone::Primary,
//!     )
//! }
//! ```
//!
//! Like every selection control in idea-ui, `Chip` is **controlled**: it
//! never owns the boolean. The host keeps the selected state (a signal, a
//! set membership test, a route flag, …), passes it in as `selected`, and
//! flips it in `on_select`. That keeps a row of chips trivially
//! single-select *or* multi-select depending on what the host does in the
//! callback.
//!
//! ## Selected vs unselected appearance
//! Both states resolve from the installed Tag stylesheet (chips and tags
//! share the pill shape and the tone × variant axes). The `selected`
//! state paints with the caller's chosen `variant`; the unselected state
//! drops to the quieter `Ghost` variant of the same tone, so a row of
//! chips reads as "one (or some) lit up, the rest muted" without the
//! caller wiring two stylesheets. Pass an explicit `variant` to set the
//! *selected* look (e.g. `Filled` for a stronger highlight).

use std::rc::Rc;

use runtime_core::{
    component, pressable, recipe, ui, Element, IdealystSchema, IntoElement, Reactive,
    StyleApplication,
};

use idea_theme::extensible::{installed_tag_sheet, tone, variant, ToneRef, Variant, VariantRef};

use crate::components::ControlSize;
use crate::stylesheets::TagLabel;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct ChipProps {
    /// Chip text. `Reactive<String>` — static or live (signal/`rx!`).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
    /// Whether this chip is currently selected. Controlled by the host —
    /// the chip never owns it. Drives the lit/muted appearance.
    pub selected: bool,
    /// Fires when the chip is tapped. The host flips its `selected` source
    /// here (`move || on.set(!on.get())` for a toggle, or a set
    /// insert/remove for multi-select). When unset, the chip is inert
    /// (renders, but a tap does nothing) per idea-ui's §9.6 optional
    /// callback rule.
    pub on_select: Option<Rc<dyn Fn()>>,
    /// Semantic color palette (Primary, Neutral, Success, …). Default
    /// Neutral. Applies to both states (the selected state is lit, the
    /// unselected state is the muted Ghost of the same tone).
    pub tone: ToneRef,
    /// Surface treatment used for the **selected** state (Soft, Filled,
    /// Outline, …). Default Soft. The unselected state always uses the
    /// quieter Ghost variant of the same tone.
    pub variant: VariantRef,
    /// Size scale (Sm, Md, Lg). Default Md. Shared with the other
    /// selection controls.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub size: ControlSize,
}

impl Default for ChipProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            selected: false,
            on_select: None,
            tone: tone::Neutral.into(),
            variant: variant::Soft.into(),
            size: ControlSize::Md,
        }
    }
}

/// Renders a selectable pill: a pressable whose tone × variant appearance
/// switches between a lit (`selected`) and a muted (unselected) state,
/// reporting taps via `on_select`. The host owns the selected boolean.
#[component]
pub fn Chip(props: &ChipProps) -> Element {
    let label = props.label.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();
    let on_select = props.on_select.clone();

    // Selected → caller's variant (lit); unselected → Ghost (muted) of
    // the same tone. Both arms resolve from the installed Tag sheet, so a
    // chip looks like a tag of the matching tone/variant — no separate
    // stylesheet needed.
    let variant_key = if props.selected {
        variant.key()
    } else {
        variant::Ghost.key()
    };
    let appearance_key = format!("{}_{}", tone.key(), variant_key);
    let container_style =
        StyleApplication::new(installed_tag_sheet()).with("appearance", appearance_key);

    let label_style = TagLabel();
    let label_el: Element = ui! { text(style = label_style) { label } };

    // §9.6: bind the press handler only when the host supplied one. A
    // `pressable` always needs *a* handler, so the inert case gets a
    // no-op — but we keep the conditional shape so the intent reads
    // clearly and matches the rest of the library.
    match on_select {
        Some(cb) => pressable(vec![label_el], move || (cb)())
            .with_style(container_style)
            .into_element(),
        None => pressable(vec![label_el], || {})
            .with_style(container_style)
            .into_element(),
    }
}

recipe!(
    Chip,
    /// A selectable filter chip. The host owns the selected state (here a
    /// `Signal<bool>`); `on_select` flips it. Drop several in a row for a
    /// filter bar — make it multi-select by toggling each independently,
    /// or single-select by clearing the others in the callback.
    pub fn chip_filter() -> ::runtime_core::Element {
        use crate::components::chip::Chip;
        use crate::{tone, variant};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let on = signal!(false);
        let on_select: Rc<dyn Fn()> = Rc::new(move || on.set(!on.get()));
        ui! {
            Chip(
                label = "Rust",
                selected = on.get(),
                on_select = Some(on_select.clone()),
                tone = tone::Primary,
                variant = variant::Soft,
            )
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_unselected_and_inert() {
        let p = ChipProps::default();
        assert!(!p.selected);
        assert!(p.on_select.is_none());
        assert_eq!(p.size, ControlSize::Md);
    }

    /// A chip with no `on_select` still renders (it just doesn't react to
    /// taps) — never panics, per §9.6. (Resolving the chip's appearance
    /// reads the installed Tag sheet, so install the theme first like the
    /// other style-resolving component tests do.)
    #[test]
    fn inert_chip_renders_without_callback() {
        use idea_theme::theme::{install_idea_theme, light_theme};
        install_idea_theme(light_theme());

        let el = Chip(&ChipProps {
            label: Reactive::Static("Tag".to_string()),
            ..Default::default()
        });
        assert!(matches!(el, Element::Pressable { .. }));
    }
}
