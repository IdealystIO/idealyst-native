//! `Image` — a themed wrapper over the framework's `image` primitive
//! with optional explicit dimensions and circular cropping.
//!
//! ```ignore
//! ui! { Image(src = "https://…/avatar.png", alt = Some("Ada".into()), width = Some(64.0f32), height = Some(64.0f32), rounded = true) }
//!
//! static LOGO: Asset<kinds::Image> = asset!("logo.svg");
//! ui! { Image(src = LOGO, alt = Some("Logo".into())) }
//! ```
//!
//! `ui!` routes the PascalCase `Image` tag to this component; the
//! lowercase `image` tag is the framework's raw primitive.
//!
//! Sizing is opt-in: with no `width`/`height` the image takes its
//! natural / flex-given size. `rounded` clips to a circle (pair with
//! equal width/height for a round avatar).

use std::rc::Rc;

use runtime_core::{
    component, component_styles, image_from, Element, IdealystSchema, ImageLoadEvent, ImageSource, IntoElement,
    Length, Overflow, Reactive, StyleRules, Tokenized,
};

use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Image`].
    pub Image<IdeaThemeRef> {
        slot root {
            base(_t) {
                overflow: Overflow::Hidden,
            }
            /// `rounded`: clip to a circle.
            variant rounded {
                #[default]
                off(_t) {}
                on(t) { border_radius: t.radius.pill() }
            }
        }
    }
}

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>`, except `src`: an `ImageSource` already carries its own
// `Reactive<String>` URL (or a bundled asset), so it is `#[prop(static)]` and a
// `Signal`/`rx!` URL still repaints the image in place. `width`/`height`/`rounded` drive the style sink (`.get()`
// read INSIDE the closure). A live `alt` routes to the primitive's reactive
// `.alt_reactive()` sink (swaps the alt / a11y label in place); a `Static` alt
// is set once. A bare value stays a zero-cost `Static` snapshot.
#[runtime_core::props(styles = ImageStyles)]
#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ImageProps {
    /// What to show: a URL (`https://…`, `data:…`, or a root-relative
    /// `/path` resolved against the server base) or a bundled
    /// [`Asset`](runtime_core::assets::Asset) from `asset!` /
    /// `embed_asset!`. Static or live (`Signal`/`rx!` URL).
    #[schema(constraint = "ImageSource — URL string or bundled Asset")]
    #[prop(static)]
    pub src: ImageSource,
    /// Accessible description. Maps to `alt` on web.
    pub alt: Option<String>,
    /// Explicit width in px. `None` = natural / flex-sized.
    pub width: Option<f32>,
    /// Explicit height in px.
    pub height: Option<f32>,
    /// Clip to a circle (pill radius).
    pub rounded: bool,
    /// Fires once the bitmap has decoded, with its natural pixel
    /// [dimensions](runtime_core::ImageLoadEvent). Delivered on web + Apple;
    /// a no-op on Android / headless. `None` = no observer.
    pub on_load: Option<Rc<dyn Fn(ImageLoadEvent)>>,
    /// Fires when the image fails to load / decode. Same backend coverage
    /// as [`on_load`](Self::on_load).
    pub on_error: Option<Rc<dyn Fn()>>,
}

impl Default for ImageProps {
    fn default() -> Self {
        Self {
            src: ImageSource::from(""),
            alt: Reactive::Static(None),
            width: Reactive::Static(None),
            height: Reactive::Static(None),
            rounded: Reactive::Static(false),
            on_load: None,
            on_error: None,
            styles: Default::default(),
        }
    }
}

/// Themed wrapper over the framework's `image` primitive. Adds opt-in
/// explicit `width`/`height` sizing and a `rounded` (circular) clip on
/// top of the raw image.
#[component]
pub fn Image(props: &ImageProps) -> Element {
    // `rounded` is an axis (an enumerated arm with build-time CSS); the
    // author's pixel dims are continuous, so they ride the inline layer
    // rather than minting a cache entry per (w, h).
    let dims = |w: Option<f32>, h: Option<f32>| StyleRules {
        width: w.map(|w| Tokenized::Literal(Length::Px(w))),
        height: h.map(|h| Tokenized::Literal(Length::Px(h))),
        ..Default::default()
    };
    let style = ImageStyles::root().set(props.rounded.clone().map(ImageRounded::from));
    let style = match (props.width.clone(), props.height.clone()) {
        (Reactive::Static(None), Reactive::Static(None)) => style,
        (Reactive::Static(w), Reactive::Static(h)) => style.inline(dims(w, h)),
        (w, h) => style.inline_with(move || dims(w.get(), h.get())),
    };
    let style = style.overridden(props.styles.root.as_ref());

    // `image_from` picks the primitive's URL or asset path. A live URL
    // repaints the image in place (the primitive's handler re-runs
    // `update_image_src`); an asset is registered with the backend first.
    let mut img = image_from(props.src.clone()).with_style(style);

    // A live `alt` routes to the primitive's reactive `.alt_reactive()` sink
    // (the handler installs an Effect → `update_image_alt`); a `Static` alt is
    // set once via the one-shot `.alt()` setter.
    if props.alt.is_static() {
        if let Some(alt) = props.alt.get() {
            img = img.alt(alt);
        }
    } else {
        let alt = props.alt.clone();
        img = img.alt_reactive(move || alt.get());
    }

    // Optional load / error observers — bind only when present so the
    // primitive installs no handler otherwise (§9.6). The primitive's
    // handler borrows `&ImageLoadEvent`; the component's prop takes it by
    // value (`ImageLoadEvent` is `Copy`).
    if let Some(cb) = props.on_load.clone() {
        img = img.on_load(move |ev| cb(*ev));
    }
    if let Some(cb) = props.on_error.clone() {
        img = img.on_error(move || cb());
    }
    img.into_element()
}
