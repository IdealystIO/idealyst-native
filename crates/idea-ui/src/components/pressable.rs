//! `Pressable` — a styled wrapper around the `Button` primitive.
//!
//! Sizing (`sm` / `md` / `lg`) is a stylesheet variant — discrete,
//! cacheable. Coloring is driven by an [`Intent`] applied as overrides
//! on top of the variant-resolved base. Both layers re-resolve when
//! the theme signal flips.
//!
//! ```ignore
//! use idea_ui::{Primary, IntoRcIntent};
//!
//! ui! {
//!     Pressable(
//!         label = "Save",
//!         on_click = on_save,
//!         intent = Primary.into_rc(),
//!         size = PressableSize::Md
//!     )
//! }
//! ```

use std::rc::Rc;

use framework_core::{
    text, ui, IntoPrimitive, PressableHandle, Primitive, Ref, StyleApplication, VariantEnum,
};

use crate::intent::{apply_palette, Intent, IntoRcIntent, Primary};
use crate::stylesheets::Pressable;
use crate::theme::IdeaThemeRef;

pub use crate::stylesheets::PressableSize;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct PressableProps {
    pub label: String,
    pub on_click: Rc<dyn Fn()>,
    /// What this button means. Defaults to [`Primary`]. Any type
    /// implementing [`Intent`] works — see [`crate::intent`] for how
    /// to add a custom intent.
    pub intent: Rc<dyn Intent>,
    pub size: PressableSize,
    pub disabled: Option<Rc<dyn Fn() -> bool>>,
    /// When `Some`, fills the given `Ref<PressableHandle>` with the
    /// underlying Pressable primitive's handle on mount. Useful for
    /// anchoring an `Overlay` to this button — pass the same ref to
    /// the overlay's anchor target.
    pub bind_to: Option<Ref<PressableHandle>>,
}

impl Default for PressableProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            on_click: Rc::new(|| {}),
            intent: Primary.into_rc(),
            size: PressableSize::default(),
            disabled: None,
            bind_to: None,
        }
    }
}

pub fn pressable(props: &PressableProps) -> Primitive {
    let label = props.label.clone();
    let on_click = props.on_click.clone();
    let size = props.size;
    let disabled = props.disabled.clone();
    let intent: Rc<dyn Intent> = props.intent.clone();
    let bind_to = props.bind_to;

    // Style closure resolves the intent's palette on each fire so
    // theme swaps propagate into intent-colored components.
    let style = move || {
        let theme = framework_core::active_theme();
        let theme_ref = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let palette = intent.palette(theme_ref);

        let app = StyleApplication::new(Pressable::sheet())
            .with("size", size.as_variant_str().to_string());
        apply_palette(app, &palette)
    };

    // Built on the framework's `pressable` primitive (a tappable
    // `<div>` on web — no `<button>` UA chrome) so the visual is
    // entirely owned by the `Pressable` stylesheet. The label
    // becomes a `Text` child of the pressable.
    let children: Vec<Primitive> = vec![text(label).into_primitive()];
    let on_click_for_p = on_click.clone();
    let mut bound = framework_core::pressable(children, move || (on_click_for_p)())
        .with_style(style);
    if let Some(d) = disabled {
        bound = bound.disabled(move || (d)());
    }
    if let Some(r) = bind_to {
        bound = bound.bind(r);
    }
    bound.into_primitive()
}
