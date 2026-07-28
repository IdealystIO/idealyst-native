//! Media payloads: `image`, `icon`, `link`.

use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::assets::{kinds, Asset};
use runtime_core::primitives::icon::{IconData, IconHandle, StrokeAnimation};
use runtime_core::primitives::image::ImageHandle;
use runtime_core::primitives::link::LinkHandle;
use runtime_core::{Color, ImageErrorHandler, ImageLoadHandler};
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

/// The `link` primitive (`walker/link.rs`). P2 carries the activation
/// callback directly: `external` links default `on_activate` to the
/// platform URL opener (the walker's port); non-external links REQUIRE
/// `on_activate` — the old walker's navigator dispatch is composed by the
/// navigation layer, which mounts through this same payload when it lands
/// (deferred set: navigator).
pub struct LinkPrim {
    pub url: Value<String>,
    pub external: bool,
    pub on_activate: Option<Rc<dyn Fn()>>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(LinkHandle)>>,
}
