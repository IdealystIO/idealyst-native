//! Image primitive.
//!
//! Backed by `<img>` on web, `UIImageView` on iOS, `ImageView` on
//! Android. Two construction paths:
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
use crate::{Bound, Element, Ref, RefFill};
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

/// Construct an `Image` primitive. The `src` argument is reactive
/// via `IntoImageSource` — pass a `&str`/`String` for a static URL
/// or a closure for a signal-driven one.
pub fn image<S: IntoImageSource>(src: S) -> Bound<ImageHandle> {
    Bound::new(Element::Image {
        src: src.into_image_source(),
        alt: None,
        alt_fn: None,
        style: None,
        ref_fill: None,
        asset: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

/// Construct an `Image` primitive backed by a declarative asset.
///
/// ```ignore
/// use runtime_core::{image_asset, asset};
/// use runtime_core::assets::{kinds::Image, Asset};
///
/// static LOGO: Asset<Image> = asset!("images/logo.png");
///
/// ui! { ImageAsset(asset = &LOGO) }   // or programmatically:
/// image_asset(LOGO).alt("Logo".into())
/// ```
///
/// The framework registers the asset with the backend on first use
/// (deduped per [`AssetId`](crate::assets::AssetId)) and emits the
/// matching `RegisterAsset` over the wire ahead of `CreateImage`.
/// The image's `src` resolves to `"asset://{id}"`; each backend
/// rewrites that to its real loader path on `create_image`.
pub fn image_asset(asset: Asset<kinds::Image>) -> Bound<ImageHandle> {
    let id = asset.id;
    Bound::new(Element::Image {
        src: Box::new(move || format!("asset://{}", id.0)),
        alt: None,
        alt_fn: None,
        style: None,
        ref_fill: None,
        asset: Some(asset),
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
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

/// Construct an `Image` from a unified [`ImageSource`], dispatching to the
/// URL path ([`image`]) or the asset path ([`image_asset`]). Accepts
/// anything `Into<ImageSource>`, so component props can hold one
/// `ImageSource` and callers still write `src = Some("https://…".into())`
/// or `src = Some(LOGO.into())`.
pub fn image_from(src: impl Into<ImageSource>) -> Bound<ImageHandle> {
    match src.into() {
        // `r.get()` is a reactive read, so a `Signal`/`rx!` URL repaints the
        // image; a `Reactive::Static` URL is a constant.
        ImageSource::Url(r) => image(move || r.get()),
        ImageSource::Asset(a) => image_asset(a),
    }
}

impl Bound<ImageHandle> {
    /// Set an accessibility label. Maps to `alt` on web,
    /// `accessibilityLabel` on iOS, `contentDescription` on Android.
    pub fn alt(mut self, alt: String) -> Self {
        if let Element::Image { alt: slot, .. } = &mut self.primitive {
            *slot = Some(alt);
        }
        self
    }

    /// Set a reactive `alt` (accessibility label). When the closure's
    /// signals change, the rendered alt / a11y label swaps in place (no
    /// node rebuild) via `Backend::update_image_alt`. The image mounts at
    /// the closure's initial value. Static labels use [`alt`](Self::alt)
    /// and skip this. Mirrors the reactive `src` shape.
    pub fn alt_reactive<F: Fn() -> Option<String> + 'static>(mut self, f: F) -> Self {
        if let Element::Image { alt, alt_fn, .. } = &mut self.primitive {
            *alt = f();
            *alt_fn = Some(Box::new(f));
        }
        self
    }

    /// Bind to a `Ref<ImageHandle>` so the parent can call ops on
    /// this image post-mount. Mirrors `Bound<ButtonHandle>::bind`.
    pub fn bind(mut self, r: Ref<ImageHandle>) -> Self {
        if let Element::Image { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Image(Box::new(move |h| r.fill(h))));
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset;
    use crate::assets::{kinds::Image as ImageKind, AssetTag};

    #[test]
    fn image_url_constructor_leaves_asset_unset() {
        let b = image("https://example.com/x.png");
        match &b.primitive {
            Element::Image { src, asset, .. } => {
                assert!(asset.is_none(), "url path should not carry an asset");
                assert_eq!(src(), "https://example.com/x.png");
            }
            _ => panic!("expected Image"),
        }
    }

    // `image_from` + `ImageSource` unify the two paths: a string/URL source
    // dispatches to the URL path (asset unset, src = the url), and an asset
    // source dispatches to the asset path (asset set, sentinel src).
    #[test]
    fn image_from_url_source_dispatches_to_url_path() {
        let b = image_from("https://example.com/y.png");
        match &b.primitive {
            Element::Image { src, asset, .. } => {
                assert!(asset.is_none(), "a URL ImageSource must not carry an asset");
                assert_eq!(src(), "https://example.com/y.png");
            }
            _ => panic!("expected Image"),
        }
        // `From<String>` + the explicit `Url` variant route the same way.
        assert!(matches!(ImageSource::from("u".to_string()), ImageSource::Url(_)));
    }

    #[test]
    fn image_from_asset_source_dispatches_to_asset_path() {
        static PIC: Asset<ImageKind> = asset!("pic.png");
        let b = image_from(PIC);
        match &b.primitive {
            Element::Image { src, asset, .. } => {
                assert!(asset.is_some(), "an Asset ImageSource must carry the asset");
                assert_eq!(src(), format!("asset://{}", PIC.id.0), "sentinel asset url");
            }
            _ => panic!("expected Image"),
        }
        assert!(matches!(ImageSource::from(PIC), ImageSource::Asset(_)));
    }

    #[test]
    fn image_asset_constructor_emits_sentinel_url_and_carries_asset() {
        static LOGO: Asset<ImageKind> = asset!("logo.png");
        let b = image_asset(LOGO);
        match &b.primitive {
            Element::Image { src, asset, .. } => {
                let a = asset.expect("asset path should carry an Asset");
                assert_eq!(a.id, LOGO.id);
                assert_eq!(a.tag, AssetTag::Image);
                // Sentinel format matches what the backend's
                // `create_image` decodes — keep these in sync.
                assert_eq!(src(), format!("asset://{}", LOGO.id.0));
            }
            _ => panic!("expected Image"),
        }
    }

    #[test]
    fn image_asset_alt_builder_threads_through() {
        static AVATAR: Asset<ImageKind> = asset!("avatar.png");
        let b = image_asset(AVATAR).alt("User avatar".to_string());
        match &b.primitive {
            Element::Image { alt, asset, .. } => {
                assert_eq!(alt.as_deref(), Some("User avatar"));
                assert!(asset.is_some());
            }
            _ => panic!("expected Image"),
        }
    }
}

