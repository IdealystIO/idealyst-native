//! `Image` — a themed wrapper over the framework's `image` primitive
//! with optional explicit dimensions and circular cropping.
//!
//! ```ignore
//! ui! { Image(src = "https://…/avatar.png", alt = Some("Ada".into()), width = Some(64.0f32), height = Some(64.0f32), rounded = true) }
//! ```
//!
//! `ui!` routes the PascalCase `Image` tag to this component; the
//! lowercase `image` tag is the framework's raw primitive.
//!
//! Sizing is opt-in: with no `width`/`height` the image takes its
//! natural / flex-given size. `rounded` clips to a circle (pair with
//! equal width/height for a round avatar).

use runtime_core::{
    component, IntoElement, Length, Element, StyleApplication, StyleRules, Tokenized,
};

use crate::stylesheets::ImageBox;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ImageProps {
    pub src: String,
    /// Accessible description. Maps to `alt` on web.
    pub alt: Option<String>,
    /// Explicit width in px. `None` = natural / flex-sized.
    pub width: Option<f32>,
    /// Explicit height in px.
    pub height: Option<f32>,
    /// Clip to a circle (pill radius).
    pub rounded: bool,
}

impl Default for ImageProps {
    fn default() -> Self {
        Self { src: String::new(), alt: None, width: None, height: None, rounded: false }
    }
}

#[component]
pub fn Image(props: &ImageProps) -> Element {
    let w = props.width;
    let h = props.height;
    let rounded = props.rounded;

    let key = format!(
        "img-{}-{}-{}",
        w.map(|x| x as i32).unwrap_or(-1),
        h.map(|x| x as i32).unwrap_or(-1),
        rounded as u8
    );
    let style = StyleApplication::new(ImageBox::sheet()).with_computed(key, move || {
        let mut r = StyleRules::default();
        if let Some(w) = w {
            r.width = Some(Tokenized::Literal(Length::Px(w)));
        }
        if let Some(h) = h {
            r.height = Some(Tokenized::Literal(Length::Px(h)));
        }
        if rounded {
            let pill = Tokenized::token("radius-pill", Length::Px(999.0));
            r.border_top_left_radius = Some(pill.clone());
            r.border_top_right_radius = Some(pill.clone());
            r.border_bottom_left_radius = Some(pill.clone());
            r.border_bottom_right_radius = Some(pill);
        }
        r
    });

    let mut img = runtime_core::image(props.src.clone()).with_style(style);
    if let Some(alt) = props.alt.clone() {
        img = img.alt(alt);
    }
    img.into_element()
}
