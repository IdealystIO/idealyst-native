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

use framework_core::{ui, ButtonHandle, IntoPrimitive, Primitive, Ref, StyleApplication, VariantEnum};

use crate::intent::{apply_palette, Intent, IntoRcIntent, Primary};
use crate::stylesheets::Pressable;
use crate::theme::IdeaThemeRef;

pub use crate::stylesheets::PressableSize;

pub struct PressableProps {
    pub label: String,
    pub on_click: Rc<dyn Fn()>,
    /// What this button means. Defaults to [`Primary`]. Any type
    /// implementing [`Intent`] works — see [`crate::intent`] for how
    /// to add a custom intent.
    pub intent: Rc<dyn Intent>,
    pub size: PressableSize,
    pub disabled: Option<Rc<dyn Fn() -> bool>>,
    /// When `Some`, fills the given `Ref<ButtonHandle>` with the
    /// underlying Button primitive's handle on mount. Useful for
    /// anchoring an `Overlay` to this button — pass the same ref to
    /// the overlay's anchor target.
    pub bind_to: Option<Ref<ButtonHandle>>,
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

    // The style closure re-runs whenever the apply-style effect
    // re-fires — i.e. on theme change or on any signal read inside.
    // We re-resolve the intent's palette each time so theme swaps
    // propagate into intent-colored components.
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

    // Build the underlying `Button` directly (not via `ui! { Button(...) }`)
    // so we keep the `Bound<ButtonHandle>` and can chain `.bind(r)`
    // before coercing to Primitive. The macro's lowering would
    // erase the bound and prevent the bind call.
    let label_clone = label.clone();
    let on_click_for_btn = on_click.clone();
    let mut bound = framework_core::button(label_clone, move || (on_click_for_btn)())
        .with_style(style);
    if let Some(d) = disabled {
        bound = bound.disabled(move || (d)());
    }
    if let Some(r) = bind_to {
        bound = bound.bind(r);
    }
    bound.into_primitive()
}
