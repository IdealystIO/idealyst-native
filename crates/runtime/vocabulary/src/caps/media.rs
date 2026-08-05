//! Image, icon, and link capabilities.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives;
use runtime_shared::primitives::icon::IconData;
use runtime_shared::{Color, Easing, ImageErrorHandler, ImageLoadHandler};

use super::noop;
use super::{ExternalOps, ViewOps};

/// The `image` primitive. Serves `walker/image.rs`.
pub trait ImageOps: ExternalOps {
    /// Create an image node with the initial URL.
    #[allow(unused_variables)]
    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.missing_primitive_placeholder("image (backend compiled without `prim-image`)")
    }

    /// Swap the image URL reactively.
    #[allow(unused_variables)]
    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        // default: no-op; backends that don't implement images just
        // leave the URL static.
    }

    /// Update `alt` text in place (`None` clears).
    #[allow(unused_variables)]
    fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
        // default: no-op
    }

    /// Fire `handler` when the bitmap decodes (MUST fire immediately
    /// if already decoded — cached images).
    #[allow(unused_variables)]
    fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
        // default: no-op
    }

    /// Fire `handler` when the image fails to load or decode.
    #[allow(unused_variables)]
    fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
        // default: no-op
    }

    /// Imperative-ref handle for an image. Default: no-op.
    #[allow(unused_variables)]
    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
        primitives::image::ImageHandle::new(Rc::new(()), &noop::NoopImageOps)
    }
}

/// The `icon` primitive (vector path data + stroke animation). Serves
/// `walker/icon.rs`.
pub trait IconOps: ExternalOps {
    /// Create an icon node from static vector path data.
    #[allow(unused_variables)]
    fn create_icon(
        &mut self,
        data: &IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.missing_primitive_placeholder("icon (backend compiled without `prim-icon`)")
    }

    /// Update the fill color reactively.
    #[allow(unused_variables)]
    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        // default: no-op
    }

    /// Swap the icon's geometry (paths/viewbox) in place.
    #[allow(unused_variables)]
    fn update_icon_data(&mut self, node: &Self::Node, data: &IconData) {
        // default: no-op
    }

    /// Set stroke progress immediately (0.0 = nothing, 1.0 = full).
    #[allow(unused_variables)]
    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        // default: no-op — icon stays fully drawn
    }

    /// Animate stroke progress with the platform's animation system.
    #[allow(unused_variables)]
    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        // default: no-op — icon renders fully drawn
    }

    /// Imperative-ref handle for an icon. Default: no-op.
    #[allow(unused_variables)]
    fn make_icon_handle(&self, node: &Self::Node) -> primitives::icon::IconHandle {
        primitives::icon::IconHandle::new(Rc::new(()), &noop::NoopIconOps)
    }
}

/// The `link` primitive (in-app navigation container). Serves
/// `walker/link.rs`. `: ViewOps` because the frozen default degrades to
/// a non-tappable container (children still mount).
pub trait LinkOps: ViewOps {
    /// Create a navigable container; the backend fires
    /// `config.on_activate()` on activation.
    #[allow(unused_variables)]
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.create_view(a11y)
    }

    /// Update the `<a href>` destination in place (web only; native
    /// ignores the href).
    #[allow(unused_variables)]
    fn update_link_url(&mut self, node: &Self::Node, url: &str) {}

    /// Imperative-ref handle for a link. Default: no-op.
    #[allow(unused_variables)]
    fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
        primitives::link::LinkHandle::new(Rc::new(()), &noop::NoopLinkOps)
    }
}
