//! `IconButton` — square clickable for a single glyph. Same
//! `intent` + `kind` + `size` axes as [`Button`](super::button::Button);
//! the only differences are square dimensions and the glyph string in
//! place of a label.
//!
//! ```ignore
//! ui! {
//!     IconButton(
//!         glyph = "×".to_string(),
//!         on_click = on_dismiss,
//!         intent = IntentTag::Neutral,
//!         kind = ButtonKind::Ghost,
//!     )
//! }
//! ```

use std::rc::Rc;

use runtime_core::{text, IntoPrimitive, Primitive, StyleApplication, VariantEnum};

use crate::components::button::{ButtonKind, IntentTag};
use crate::stylesheets::IconButton;
use crate::theme::IdeaThemeRef;

pub use crate::stylesheets::IconButtonSize;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct IconButtonProps {
    pub glyph: String,
    pub on_click: Rc<dyn Fn()>,
    pub intent: IntentTag,
    pub kind: ButtonKind,
    pub size: IconButtonSize,
    pub disabled: Option<Rc<dyn Fn() -> bool>>,
}

impl Default for IconButtonProps {
    fn default() -> Self {
        Self {
            glyph: String::new(),
            on_click: Rc::new(|| {}),
            // IconButton defaults to Neutral/Solid — a "+", "×", or
            // similar in a card or toolbar usually shouldn't have a
            // semantic emphasis baked in.
            intent: IntentTag::Neutral,
            kind: ButtonKind::Solid,
            size: IconButtonSize::default(),
            disabled: None,
        }
    }
}

pub fn icon_button(props: &IconButtonProps) -> Primitive {
    let glyph = props.glyph.clone();
    let on_click = props.on_click.clone();
    let size = props.size;
    let intent = props.intent;
    let kind = props.kind;
    let disabled = props.disabled.clone();

    let appearance = format!("{}_{}", intent.as_str(), kind.as_str());
    let style = move || {
        let _ = crate::theme_runtime::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        StyleApplication::new(IconButton::sheet())
            .with("size", size.as_variant_str().to_string())
            .with("appearance", appearance.clone())
    };

    let glyph_child = text(glyph).into_primitive();
    let mut bound = runtime_core::pressable(vec![glyph_child], move || (on_click)())
        .with_style(style);
    if let Some(d) = disabled {
        bound = bound.disabled(move || (d)());
    }
    bound.into_primitive()
}
