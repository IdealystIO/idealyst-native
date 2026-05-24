//! Per-primitive create/update functions. Each module owns one
//! `Primitive` kind end-to-end: the create call, any update call,
//! the `Ops` impl for refs (where applicable), and the
//! `make_*_handle` method.
//!
//! Functions take `&mut WebBackend` rather than being inherent
//! methods so each module is a flat file with no `impl WebBackend`
//! ceremony around its bodies. The thin `impl Backend for WebBackend`
//! in `lib.rs` calls into them.

pub(crate) mod activity_indicator;
pub(crate) mod button;
pub(crate) mod graphics;
pub(crate) mod icon;
pub(crate) mod image;
pub(crate) mod link;
pub(crate) mod navigator;
pub(crate) mod portal;
pub(crate) mod presence;
pub(crate) mod pressable;
pub(crate) mod scroll_view;
pub(crate) mod slider;
pub(crate) mod text;
pub(crate) mod text_area;
pub(crate) mod text_input;
pub(crate) mod touch;
pub(crate) mod toggle;
pub(crate) mod view;
pub(crate) mod virtualizer;
