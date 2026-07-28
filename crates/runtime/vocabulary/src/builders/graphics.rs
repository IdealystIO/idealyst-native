//! Graphics builder: `graphics()`.

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::graphics::{GraphicsHandle, OnReadyEvent, OnResizeEvent};
use runtime_scene::{item, Element};

use crate::prims::{GraphicsPrim, PrimCell};
use crate::style_attach::IntoStyleProp;

/// Start a `graphics` surface — port of the old
/// `primitives::graphics::graphics(on_ready)` constructor. `on_ready`
/// is required (positionally, like the old entry point); `on_resize` /
/// `on_lost` default to no-ops.
///
/// The old builder wrapped `on_resize`/`on_lost` in `cycle()` ("born
/// batched"); under the staged-commit kernel every signal write stages
/// until the host driver flushes, so no wrapper is needed — see
/// [`GraphicsPrim`]'s docs.
pub fn graphics(on_ready: impl FnMut(OnReadyEvent) + 'static) -> GraphicsBuilder {
    GraphicsBuilder {
        prim: GraphicsPrim {
            on_ready: Box::new(on_ready),
            on_resize: Box::new(|_| {}),
            on_lost: Box::new(|| {}),
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
    }
}

pub struct GraphicsBuilder {
    prim: GraphicsPrim,
}

impl GraphicsBuilder {
    /// Drawable-size change after `on_ready` (NOT fired for the initial
    /// size — read `on_ready.size` for that).
    pub fn on_resize(mut self, f: impl FnMut(OnResizeEvent) + 'static) -> Self {
        self.prim.on_resize = Box::new(f);
        self
    }

    /// Surface invalidated (Android backgrounding, web context-lost).
    /// The author must drop every handle derived from the previous
    /// surface before returning.
    pub fn on_lost(mut self, f: impl FnMut() + 'static) -> Self {
        self.prim.on_lost = Box::new(f);
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(GraphicsHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}
