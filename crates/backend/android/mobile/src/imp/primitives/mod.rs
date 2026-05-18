//! Per-primitive create/update functions. Each module owns one
//! `Primitive` kind end-to-end: the create call, any update call,
//! the `Ops` impl for refs (where applicable), and the
//! `make_*_handle` builder.
//!
//! Functions take `&AndroidBackend` (or `&mut AndroidBackend`) rather
//! than being inherent methods so each module is a flat file with no
//! `impl AndroidBackend` ceremony around its bodies. The thin
//! `impl Backend for AndroidBackend` in `imp/mod.rs` calls into them.

pub(crate) mod activity_indicator;
pub(crate) mod button;
pub(crate) mod graphics;
pub(crate) mod icon;
pub(crate) mod image;
pub(crate) mod link;
pub(crate) mod navigator;
pub(crate) mod overlay;
pub(crate) mod scroll_view;
pub(crate) mod tab_drawer;
pub(crate) mod slider;
pub(crate) mod text;
pub(crate) mod text_input;
pub(crate) mod toggle;
pub(crate) mod video;
pub(crate) mod view;
pub(crate) mod virtualizer;
pub(crate) mod web_view;
