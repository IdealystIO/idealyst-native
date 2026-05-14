//! Image primitive.
//!
//! Backed by `<img>` on web, `UIImageView` on iOS, `ImageView` on
//! Android. Source is a reactive closure so authors can pass a static
//! URL (`image("https://...")`) or a closure reading a signal.
//!
//! Source resolution is URL-only for v1 — backends fetch the bytes
//! themselves via their native primitive's URL-loading machinery.
//! Bundled / file:// / data:// sources are valid URLs and supported
//! by the platforms' native loaders; the framework doesn't translate.

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

