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
use crate::{Bound, Primitive, Ref, RefFill};
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
    Bound::new(Primitive::Image {
        src: src.into_image_source(),
        alt: None,
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
    Bound::new(Primitive::Image {
        src: Box::new(move || format!("asset://{}", id.0)),
        alt: None,
        style: None,
        ref_fill: None,
        asset: Some(asset),
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<ImageHandle> {
    /// Set an accessibility label. Maps to `alt` on web,
    /// `accessibilityLabel` on iOS, `contentDescription` on Android.
    pub fn alt(mut self, alt: String) -> Self {
        if let Primitive::Image { alt: slot, .. } = &mut self.primitive {
            *slot = Some(alt);
        }
        self
    }

    /// Bind to a `Ref<ImageHandle>` so the parent can call ops on
    /// this image post-mount. Mirrors `Bound<ButtonHandle>::bind`.
    pub fn bind(mut self, r: Ref<ImageHandle>) -> Self {
        if let Primitive::Image { ref_fill, .. } = &mut self.primitive {
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
            Primitive::Image { src, asset, .. } => {
                assert!(asset.is_none(), "url path should not carry an asset");
                assert_eq!(src(), "https://example.com/x.png");
            }
            _ => panic!("expected Image"),
        }
    }

    #[test]
    fn image_asset_constructor_emits_sentinel_url_and_carries_asset() {
        static LOGO: Asset<ImageKind> = asset!("logo.png");
        let b = image_asset(LOGO);
        match &b.primitive {
            Primitive::Image { src, asset, .. } => {
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
            Primitive::Image { alt, asset, .. } => {
                assert_eq!(alt.as_deref(), Some("User avatar"));
                assert!(asset.is_some());
            }
            _ => panic!("expected Image"),
        }
    }
}

