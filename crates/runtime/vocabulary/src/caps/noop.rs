//! No-op handle-ops ZSTs backing the `make_*_handle` defaults, so a
//! backend that doesn't override one still returns a type-correct no-op
//! handle. `&'static` references to these ZSTs come from rvalue static
//! promotion at the call sites.

use std::any::Any;

use runtime_shared::primitives;

pub(crate) struct NoopIconOps;
impl primitives::icon::IconOps for NoopIconOps {}

pub(crate) struct NoopImageOps;
impl primitives::image::ImageOps for NoopImageOps {}

pub(crate) struct NoopTextInputOps;
impl primitives::text_input::TextInputOps for NoopTextInputOps {
    fn focus(&self, _: &dyn Any) {}
    fn blur(&self, _: &dyn Any) {}
    fn select_all(&self, _: &dyn Any) {}
    fn insert_text(&self, _: &dyn Any, _: &str) {}
}

pub(crate) struct NoopTextAreaOps;
impl primitives::text_area::TextAreaOps for NoopTextAreaOps {
    fn focus(&self, _: &dyn Any) {}
    fn blur(&self, _: &dyn Any) {}
    fn select_all(&self, _: &dyn Any) {}
    fn insert_text(&self, _: &dyn Any, _: &str) {}
}

pub(crate) struct NoopToggleOps;
impl primitives::toggle::ToggleOps for NoopToggleOps {}

pub(crate) struct NoopScrollViewOps;
impl primitives::scroll_view::ScrollViewOps for NoopScrollViewOps {
    fn scroll_to(&self, _: &dyn Any, _: f32, _: f32) {}
}

pub(crate) struct NoopSliderOps;
impl primitives::slider::SliderOps for NoopSliderOps {}

pub(crate) struct NoopActivityIndicatorOps;
impl primitives::activity_indicator::ActivityIndicatorOps for NoopActivityIndicatorOps {}

pub(crate) struct NoopVirtualizerOps;
// Every method defaults to a no-op / `(0.0, 0.0)`, which is exactly
// this handle's contract — the backend either has no virtualizer or
// never overrode `make_virtualizer_handle`.
impl primitives::virtualizer::VirtualizerOps for NoopVirtualizerOps {}

pub(crate) struct NoopVirtualGridOps;
// Same "backend never overrode the factory" contract as
// `NoopVirtualizerOps` — every method already defaults.
impl primitives::virtual_grid::VirtualGridOps for NoopVirtualGridOps {}

pub(crate) struct NoopGraphicsOps;
impl primitives::graphics::GraphicsOps for NoopGraphicsOps {}

pub(crate) struct NoopNavigatorOps;
impl primitives::navigator::NavigatorOps for NoopNavigatorOps {}

pub(crate) struct NoopLinkOps;
impl primitives::link::LinkOps for NoopLinkOps {
    fn activate(&self, _node: &dyn Any) {}
}

pub(crate) struct NoopPresenceOps;
impl primitives::presence::PresenceOps for NoopPresenceOps {}

pub(crate) struct NoopPortalOps;
impl primitives::portal::PortalOps for NoopPortalOps {}

pub(crate) struct NoopButtonOps;
impl runtime_shared::ButtonOps for NoopButtonOps {
    fn click(&self, _node: &dyn Any) {}
}

pub(crate) struct NoopPressableOps;
impl runtime_shared::PressableOps for NoopPressableOps {
    fn click(&self, _node: &dyn Any) {}
}

pub(crate) struct NoopViewOps;
impl runtime_shared::ViewOps for NoopViewOps {}

pub(crate) struct NoopTextOps;
impl runtime_shared::TextOps for NoopTextOps {}
