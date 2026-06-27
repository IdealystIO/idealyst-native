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
    component, IdealystSchema, IntoElement, Length, Element, Reactive, StyleApplication, StyleRules,
    Tokenized,
};

use crate::stylesheets::ImageBox;

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>`. `src` routes to the framework `image` primitive's reactive
// source (it accepts a `Fn() -> String`), so a `Signal`/`rx!` URL repaints the
// image in place. `width`/`height`/`rounded` drive the style sink (`.get()`
// read INSIDE the closure). A live `alt` routes to the primitive's reactive
// `.alt_reactive()` sink (swaps the alt / a11y label in place); a `Static` alt
// is set once. A bare value stays a zero-cost `Static` snapshot.
#[runtime_core::props]
#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ImageProps {
    /// Image source URL handed to the underlying `image` primitive.
    #[schema(constraint = "image URL the platform can load (http/https/file/data)")]
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
        Self {
            src: Reactive::Static(String::new()),
            alt: Reactive::Static(None),
            width: Reactive::Static(None),
            height: Reactive::Static(None),
            rounded: Reactive::Static(false),
        }
    }
}

/// Themed wrapper over the framework's `image` primitive. Adds opt-in
/// explicit `width`/`height` sizing and a `rounded` (circular) clip on
/// top of the raw image.
#[component]
pub fn Image(props: &ImageProps) -> Element {
    // The style is REACTIVE when any style-driving dim prop is live; otherwise
    // the build-time fast path. The closure reads each prop's `.get()` INSIDE so
    // the apply-style Effect subscribes to whichever are dynamic, and the cache
    // key tracks the live values.
    let style_is_reactive =
        !props.width.is_static() || !props.height.is_static() || !props.rounded.is_static();

    let make_style = {
        let width = props.width.clone();
        let height = props.height.clone();
        let rounded = props.rounded.clone();
        move || -> StyleApplication {
            let w = width.get();
            let h = height.get();
            let rounded = rounded.get();
            let key = format!(
                "img-{}-{}-{}",
                w.map(|x| x as i32).unwrap_or(-1),
                h.map(|x| x as i32).unwrap_or(-1),
                rounded as u8
            );
            StyleApplication::new(ImageBox::sheet()).with_computed(key, move || {
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
            })
        }
    };

    // `src` routes to the framework `image` primitive's reactive source: a
    // `Reactive::Static` URL is a constant, a `Signal`/`rx!` URL repaints the
    // image in place (the primitive's walker re-runs `update_image_src`).
    let src = props.src.clone();
    let img = runtime_core::image(move || src.get());
    let mut img = if style_is_reactive {
        img.with_style(make_style)
    } else {
        img.with_style(make_style())
    };

    // A live `alt` routes to the primitive's reactive `.alt_reactive()` sink
    // (the walker installs an Effect → `update_image_alt`); a `Static` alt is
    // set once via the one-shot `.alt()` setter.
    if props.alt.is_static() {
        if let Some(alt) = props.alt.get() {
            img = img.alt(alt);
        }
    } else {
        let alt = props.alt.clone();
        img = img.alt_reactive(move || alt.get());
    }
    img.into_element()
}
