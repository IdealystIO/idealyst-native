//! Image primitive.
//!
//! Backed by `<img>` on web, `UIImageView` on iOS, `ImageView` on
//! Android, and a layer-backed image view on macOS. How the bitmap
//! fits its box is controlled by the `object_fit` style property
//! ([`ObjectFit`](crate::ObjectFit)) — `Fill` / `Contain` / `Cover`,
//! defaulting to `Contain` (aspect-fit) on every backend. Two
//! construction paths:
//!
//! - **URL-based**: [`image`] takes a free-form `&str`/`String` or a
//!   closure returning `String`. The framework hands the URL to the
//!   backend as-is. Bundled / `file://` / `data:` URLs are supported
//!   by the native loaders; the framework doesn't translate.
//! - **Asset-based**: [`image_asset`] takes a declarative
//!   [`Asset<kinds::Image>`](crate::assets::Asset). The walker calls
//!   `Backend::register_asset` once before `create_image`, and the
//!   backend resolves the asset to its locally-correct URL (web's
//!   `dist/assets/` path, iOS bundle resource, Android `AssetManager`,
//!   etc.). Internally the framework still hands the backend a URL —
//!   the sentinel `"asset://{id}"` — which backends rewrite to the
//!   resolved location.

use crate::assets::{kinds, Asset};
use std::any::Any;
use std::rc::Rc;

/// Handle exposed to a parent via `Ref<ImageHandle>`. No methods in
/// v1 — image is a passive widget. Future additions could include
/// `reload()`, `measure()`, or load-state callbacks.
#[derive(Clone)]
pub struct ImageHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn ImageOps,
}

impl ImageHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ImageOps) -> Self {
        Self { node, ops }
    }
}

pub trait ImageOps {
    // Reserved for future image-specific operations (reload, measure).
}

/// Payload delivered to an [`on_load`](Bound::on_load) handler once an
/// image's bitmap has decoded. `width`/`height` are the image's
/// **natural** (intrinsic) pixel dimensions — `naturalWidth`/
/// `naturalHeight` on web, the `NSImage`/`UIImage` `size` on Apple.
/// These are otherwise awkward to obtain (they require a live ref +
/// a measure op), so the load event surfaces them directly — e.g. to
/// compute an aspect ratio for a placeholder box before the image
/// paints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageLoadEvent {
    pub width: f32,
    pub height: f32,
}

/// Installed via [`Bound::<ImageHandle>::on_load`]. Fires once, when the
/// image's bitmap has finished decoding, with its natural
/// [dimensions](ImageLoadEvent). Reactive `src` swaps re-fire it when the
/// new bitmap loads. Born batched — one reactive cycle per call, like the
/// touch / hover handlers.
///
/// Delivered on web (`<img>` `load`) and Apple (async URL completion +
/// synchronous asset assignment). A **no-op on Android** (its `ImageView`
/// has no URL loader — nothing decodes to observe) and on headless / CPU
/// backends (no real decode). See [`crate::Backend::install_image_load_handler`].
pub type ImageLoadHandler = Rc<dyn Fn(&ImageLoadEvent)>;

/// Installed via [`Bound::<ImageHandle>::on_error`]. Fires when the image
/// fails to load or decode — a network error, a 404, or bytes no decoder
/// accepts. Carries no payload (there's nothing to report but the
/// failure). Born batched like [`ImageLoadHandler`].
///
/// Same backend coverage as [`ImageLoadHandler`]: web (`<img>` `error`)
/// and Apple (async completion failure); no-op elsewhere. See
/// [`crate::Backend::install_image_error_handler`].
pub type ImageErrorHandler = Rc<dyn Fn()>;

/// Trait the macro emits for `src = ...`. Accepts a bare string, a
/// `String`, or a closure returning `String` — closures enable
/// reactive sources without explicit `move ||` from the caller.
pub trait IntoImageSource {
    fn into_image_source(self) -> Box<dyn Fn() -> String>;
}

impl IntoImageSource for &str {
    fn into_image_source(self) -> Box<dyn Fn() -> String> {
        let s = self.to_string();
        Box::new(move || s.clone())
    }
}

impl IntoImageSource for String {
    fn into_image_source(self) -> Box<dyn Fn() -> String> {
        Box::new(move || self.clone())
    }
}

impl<F> IntoImageSource for F
where
    F: Fn() -> String + 'static,
{
    fn into_image_source(self) -> Box<dyn Fn() -> String> {
        Box::new(self)
    }
}



/// A unified source for an image — a (reactive) URL **or** a declarative
/// bundled [`Asset`]. The two existing construction paths ([`image`] for a
/// free-form URL, [`image_asset`] for a registered asset) converge here so
/// an image-bearing API (an `Avatar`, a card, …) can accept either with a
/// single prop instead of forcing a string URL.
///
/// `#[non_exhaustive]`: future modes (raw bytes, a pre-decoded handle, a
/// blob) can be added as new variants without breaking call sites —
/// construct via the `From` impls (`&str` / `String` / `Reactive<String>` /
/// `Signal<String>` / `Asset<Image>`) rather than matching exhaustively.
#[non_exhaustive]
#[derive(Clone)]
pub enum ImageSource {
    /// A URL string (`http(s)://`, `file://`, `data:`…). Reactive — a
    /// static literal or a signal-driven getter; the image repaints when it
    /// changes.
    Url(crate::Reactive<String>),
    /// A declarative bundled asset, resolved to a backend-local path
    /// (web `dist/assets/`, iOS bundle resource, Android `AssetManager`).
    Asset(Asset<kinds::Image>),
}

impl From<&str> for ImageSource {
    fn from(s: &str) -> Self {
        ImageSource::Url(s.into())
    }
}
impl From<String> for ImageSource {
    fn from(s: String) -> Self {
        ImageSource::Url(s.into())
    }
}
impl From<crate::Reactive<String>> for ImageSource {
    fn from(r: crate::Reactive<String>) -> Self {
        ImageSource::Url(r)
    }
}
impl From<crate::Signal<String>> for ImageSource {
    fn from(s: crate::Signal<String>) -> Self {
        ImageSource::Url(s.into())
    }
}
impl From<Asset<kinds::Image>> for ImageSource {
    fn from(a: Asset<kinds::Image>) -> Self {
        ImageSource::Asset(a)
    }
}




