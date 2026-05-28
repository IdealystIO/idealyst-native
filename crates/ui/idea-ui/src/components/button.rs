//! `Button` — the styled clickable, built on the extensible
//! Variant/Tone/Size/Shape trait surface.
//!
//! ```ignore
//! ui! {
//!     Btn(
//!         label = "Save",
//!         on_click = on_save,
//!         tone = tone::Primary,
//!         variant = variant::Filled,
//!         size = size::Md,
//!         shape = shape::Md,
//!     )
//! }
//! ```
//!
//! Styling routes through the [installed Button
//! stylesheet][installed_button_sheet]. `install_idea_theme` installs
//! the default sheet at startup; apps with custom modifiers
//! (`Hype` tone, `Elevated` variant) override via
//! `install_button_sheet(ButtonSheetBuilder::new().add_tone(Hype.into()).build())`.
//!
//! Every supported `(tone, variant, size, shape)` combination is
//! pre-generated as a CSS rule at sheet registration time, so
//! apply-style is a className lookup — no FOUC, no dynamic CSS mint.

use std::rc::Rc;

use runtime_core::{text, IntoElement, PressableHandle, Element, Reactive, Ref, StyleApplication};

use idea_theme::extensible::{installed_button_sheet, ButtonSizeRef, ShapeRef, ToneRef, VariantRef};

/// Props for the extensible Button. Each modifier axis is a typed
/// handle (`*Ref` newtype) so call sites can write
/// `tone: tone::Primary.into()` instead of `Rc::new(...)`. Built-in
/// defaults route to Filled/Primary/Md/Md.
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ButtonProps {
    /// Button text. `Reactive<String>` — static for a literal/`String`,
    /// live for a `Signal<String>` or `rx!(…)`.
    pub label: Reactive<String>,
    pub on_click: Rc<dyn Fn()>,
    pub tone: ToneRef,
    pub variant: VariantRef,
    pub size: ButtonSizeRef,
    pub shape: ShapeRef,
    pub disabled: Option<Rc<dyn Fn() -> bool>>,
    /// When `Some`, fills the given `Ref<PressableHandle>` on mount.
    /// Useful for anchoring an `Overlay` to this button.
    pub bind_to: Option<Ref<PressableHandle>>,
}

impl Default for ButtonProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            on_click: Rc::new(|| {}),
            tone: ToneRef::default(),
            variant: VariantRef::default(),
            size: ButtonSizeRef::default(),
            shape: ShapeRef::default(),
            disabled: None,
            bind_to: None,
        }
    }
}

pub fn button(props: &ButtonProps) -> Element {
    let label = props.label.clone();
    let on_click = props.on_click.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();
    let size = props.size.clone();
    let shape = props.shape.clone();
    let disabled = props.disabled.clone();
    let bind_to = props.bind_to;

    // Variant-axis keys map directly to the installed stylesheet's
    // pre-generated arms. For a built-in modifier set the arms exist;
    // for an app-extended set, apps must have installed an extended
    // sheet that includes those arms (else the framework falls back
    // to the default arms).
    let appearance_key = format!("{}_{}", tone.key(), variant.key());
    let size_key = size.key().to_string();
    let shape_key = shape.key().to_string();

    // STATIC style — applied at build time (before first paint) and
    // re-applied in bulk by the theme cohort on `set_theme`. A
    // reactive closure here would defer the apply to a per-node
    // Effect, letting the element paint once with browser-default
    // styles before the themed class lands — which the CSS transition
    // then animates (the on-load / on-navigation flicker). The
    // variant-axis keys are fixed per instance, so nothing here needs
    // to be reactive; theme swaps flow through the CSS-variable tokens.
    let style = StyleApplication::new(installed_button_sheet())
        .with("appearance", appearance_key)
        .with("size", size_key)
        .with("shape", shape_key);

    let children: Vec<Element> = vec![text(label).into_element()];
    let on_click_for_p = on_click.clone();
    let mut bound = runtime_core::pressable(children, move || (on_click_for_p)())
        .with_style(style);
    if let Some(d) = disabled {
        bound = bound.disabled(move || (d)());
    }
    if let Some(r) = bind_to {
        bound = bound.bind(r);
    }
    bound.into_element()
}
