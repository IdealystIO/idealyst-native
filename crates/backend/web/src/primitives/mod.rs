//! Per-primitive create/update functions. Each module owns one
//! `Element` kind end-to-end: the create call, any update call,
//! the `Ops` impl for refs (where applicable), and the
//! `make_*_handle` method.
//!
//! Functions take `&mut WebBackend` rather than being inherent
//! methods so each module is a flat file with no `impl WebBackend`
//! ceremony around its bodies. The thin `impl Backend for WebBackend`
//! in `lib.rs` calls into them.

#[cfg(feature = "prim-activity")]
pub(crate) mod activity_indicator;
pub(crate) mod button;
#[cfg(feature = "prim-graphics")]
pub(crate) mod graphics;
#[cfg(feature = "prim-icon")]
pub(crate) mod icon;
#[cfg(feature = "prim-image")]
pub(crate) mod image;
pub(crate) mod link;
#[cfg(feature = "prim-portal")]
pub(crate) mod portal;
#[cfg(feature = "prim-presence")]
pub(crate) mod presence;
pub(crate) mod pressable;
pub(crate) mod scroll_view;
#[cfg(feature = "prim-slider")]
pub(crate) mod slider;
pub(crate) mod text;
#[cfg(feature = "prim-text-input")]
pub(crate) mod text_area;
pub(crate) mod hover;
pub(crate) mod keyboard;
pub(crate) mod file_drop;
#[cfg(feature = "prim-text-input")]
pub(crate) mod text_input;
pub(crate) mod touch;
#[cfg(feature = "prim-toggle")]
pub(crate) mod toggle;
pub(crate) mod wheel;
pub(crate) mod view;
#[cfg(feature = "prim-virtualizer")]
pub(crate) mod virtualizer;
