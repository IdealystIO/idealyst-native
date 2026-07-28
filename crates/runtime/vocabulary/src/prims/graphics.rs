//! Graphics payload: the raw GPU-drawable surface.

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::graphics::{GraphicsHandle, OnLost, OnReady, OnResize};

use crate::style_attach::StyleProp;

/// The `graphics` primitive (`walker/graphics.rs`).
///
/// A backend-provided platform surface (canvas / SurfaceView / Metal
/// layer) the author renders to with their own GPU library. The
/// framework's surface is narrow by design: stand up the drawable, wire
/// the lifecycle callbacks, tear it down on unmount — everything else
/// (device init, render loop, redraw scheduling) is the author's,
/// driven through the `raw_window_handle` the backend delivers in
/// `on_ready` (see `runtime_core::primitives::graphics` for the
/// lifecycle contract: ready → resize* → lost → ready …).
///
/// All three callbacks move into `create_graphics` whole — the old
/// enum-variant fields, verbatim. Unlike the old builder, `on_resize` /
/// `on_lost` are NOT wrapped in `cycle()` ("born batched"): under the
/// staged-commit kernel every write stages until the host driver
/// flushes, so event batching is structural, not opt-in.
pub struct GraphicsPrim {
    pub on_ready: OnReady,
    pub on_resize: OnResize,
    pub on_lost: OnLost,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub ref_fill: Option<Box<dyn FnOnce(GraphicsHandle)>>,
}
