//! Media payloads: `image`, `icon`, `link`.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::assets::{kinds, Asset};
use runtime_shared::primitives::icon::{IconData, IconHandle, StrokeAnimation};
use runtime_shared::primitives::image::ImageHandle;
use runtime_shared::primitives::link::LinkHandle;
use runtime_shared::{Color, ImageErrorHandler, ImageLoadHandler};
use runtime_world::Value;

use crate::style_attach::StyleProp;

/// The `image` primitive (`walker/image.rs`).
///
/// - `src`: `Const` and `Dyn` both emit one `update_image_src` at mount
///   after handler installation (the old walker installs the src effect
///   unconditionally — its first fire is that update), then `Dyn`
///   re-fires on dependency changes.
/// - `alt`: `Const` rides `create_image`; `Dyn` creates with `None` and
///   updates in place (`update_image_alt`), first fire at mount — the
///   walker's `alt`/`alt_fn` split, expressed as one `Value`.
/// - `asset`: registered with the backend BEFORE `create_image` so the
///   `asset://{id}` sentinel resolves.
pub struct ImagePrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub src: Value<String>,
    pub alt: Value<Option<String>>,
    pub on_load: Option<ImageLoadHandler>,
    pub on_error: Option<ImageErrorHandler>,
    pub asset: Option<Asset<kinds::Image>>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(ImageHandle)>>,
}

/// The `icon` primitive (`walker/icon.rs`). `data` is required (the
/// builder panics at `build()` without it, matching the old constructor's
/// mandatory argument). `Dyn` color/data create at the closure's initial
/// value and update in place; `stroke` applies inline at mount and (when
/// `Dyn`) re-applies per fire; `draw_in` snaps to `from` then schedules
/// the stroke animation on the next microtask.
pub struct IconPrim {
    /// Robot/automation anchor (`test_id = …`). Always present so the
    /// builder setter compiles in every build; read only by the
    /// `robot`-feature registration in the mount handler.
    pub test_id: Option<&'static str>,
    pub data: Value<IconData>,
    pub color: Option<Value<Color>>,
    pub stroke: Option<Value<f32>>,
    pub draw_in: Option<StrokeAnimation>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(IconHandle)>>,
}

/// An in-app route destination riding on [`LinkPrim`] (`link(route =
/// …)`, P6). The mount handler composes the activation: it captures the
/// ambient [`LinkActivator`](crate::prims::LinkActivator) at mount and
/// dispatches `(name, url, make_params())` through it — push-vs-select
/// decided by the enclosing navigator, the old ambient-navigator
/// contract. No activator ambient ⇒ activation silently no-ops (old
/// `link()` posture).
pub struct RouteLink {
    /// `Route::name()` — also surfaced as `LinkConfig::route`.
    pub name: &'static str,
    /// Fresh boxed params per activation (`P: Clone` reproduces).
    pub make_params: Rc<dyn Fn() -> Box<dyn std::any::Any>>,
}

/// The `link` primitive (`walker/link.rs`). P2 carries the activation
/// callback directly: `external` links default `on_activate` to the
/// platform URL opener (the walker's port); in-app route links carry a
/// [`RouteLink`] the mount handler resolves against the ambient
/// `LinkActivator` (P6); links with NONE of callback/external/route
/// panic at mount — a link that silently does nothing is a footgun.
pub struct LinkPrim {
    pub url: Value<String>,
    pub external: bool,
    pub on_activate: Option<Rc<dyn Fn()>>,
    /// In-app route destination (see [`RouteLink`]).
    pub route_link: Option<RouteLink>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(LinkHandle)>>,
}
