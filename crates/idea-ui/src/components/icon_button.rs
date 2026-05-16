//! `IconButton` — a Pressable with square, content-sized dimensions
//! and equal horizontal + vertical padding, suitable for a single
//! icon glyph.
//!
//! Takes a `glyph: String` (typically a Unicode symbol like `×`,
//! `+`, or a font-icon ligature) rather than a `label`. The intent
//! and size variants work the same way they do on `Pressable`.
//!
//! ```ignore
//! use idea_ui::{Ghost, IntoRcIntent};
//!
//! ui! {
//!     IconButton(
//!         glyph = "×".to_string(),
//!         on_click = on_dismiss,
//!         intent = Ghost.into_rc()
//!     )
//! }
//! ```
//!
//! Right now the content has to be a string because the framework's
//! `Button` primitive carries a `String` label, not an arbitrary
//! primitive child. A future "clickable container" primitive would
//! let us host any subtree as the icon — that's the natural follow-up.

use std::rc::Rc;

use framework_core::{ui, Primitive, StyleApplication, VariantEnum};

use crate::intent::{apply_palette, Intent, IntoRcIntent, Neutral};
use crate::stylesheets::IconButton;
use crate::theme::IdeaThemeRef;

pub use crate::stylesheets::IconButtonSize;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct IconButtonProps {
    /// The glyph or short string rendered inside the button.
    pub glyph: String,
    pub on_click: Rc<dyn Fn()>,
    pub intent: Rc<dyn Intent>,
    pub size: IconButtonSize,
    pub disabled: Option<Rc<dyn Fn() -> bool>>,
}

impl Default for IconButtonProps {
    fn default() -> Self {
        Self {
            glyph: String::new(),
            on_click: Rc::new(|| {}),
            intent: Neutral.into_rc(),
            size: IconButtonSize::default(),
            disabled: None,
        }
    }
}

pub fn icon_button(props: &IconButtonProps) -> Primitive {
    let glyph = props.glyph.clone();
    let on_click = props.on_click.clone();
    let size = props.size;
    let disabled = props.disabled.clone();
    let intent: Rc<dyn Intent> = props.intent.clone();

    let style = move || {
        let theme = framework_core::active_theme();
        let theme_ref = theme
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let palette = intent.palette(theme_ref);

        let app = StyleApplication::new(IconButton::sheet())
            .with("size", size.as_variant_str().to_string());
        apply_palette(app, &palette)
    };

    match disabled {
        Some(d) => ui! {
            Button(
                label = glyph,
                on_click = move || (on_click)(),
                style = style,
                disabled = move || (d)()
            )
        },
        None => ui! {
            Button(
                label = glyph,
                on_click = move || (on_click)(),
                style = style
            )
        },
    }
}
