//! No-op handle-ops ZSTs backing the `make_*_handle` defaults.
//!
//! Copied from the private set at the bottom of
//! `runtime-core/src/backend.rs` (they are not exported there) so the
//! frozen `make_*_handle` defaults keep their "type-correct no-op
//! handle" behavior for direct implementors. `&'static` references to
//! these ZSTs come from rvalue static promotion at the call sites, same
//! as the originals. Deleted with the runtime-core dependency at P7 when
//! the handle layer migrates.

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
impl primitives::virtualizer::VirtualizerOps for NoopVirtualizerOps {
    fn scroll_to_index(&self, _: &dyn Any, _: usize) {}
}

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
