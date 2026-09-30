//! Per-primitive create/update functions. Each module owns one
//! `Element` kind end-to-end: the create call, any update call,
//! the `Ops` impl for refs (where applicable), and the
//! `make_*_handle` method.
//!
//! Functions take `&mut WebBackend` rather than being inherent
//! methods so each module is a flat file with no `impl WebBackend`
//! ceremony around its bodies. The thin `impl Backend for WebBackend`
//! in `lib.rs` calls into them.

// Element-lifetime listeners — the element owns the listener and its
// closure is released when JS collects the element — are
// `crate::glue_dom::listen_for_element_lifetime`. (They used to be parked in
// a backend-owned `Vec` that was never cleared, pinning every closure for
// the life of the process: an app mounting interactive elements
// dynamically — a virtualized list re-slicing as it scrolls — leaked
// several per cell per slice.) Listeners on `window` / `document` outlive
// every element and need an explicit removal path instead — a
// `web_glue::dom::Listener` held by their owner (see `touch::WINDOW_NET`
// and `keyboard`).

pub(crate) mod activity_indicator;
pub(crate) mod button;
pub(crate) mod graphics;
pub(crate) mod icon;
pub(crate) mod image;
pub(crate) mod link;
pub(crate) mod portal;
pub(crate) mod presence;
pub(crate) mod pressable;
pub(crate) mod scroll_view;
pub(crate) mod slider;
pub(crate) mod text;
pub(crate) mod text_area;
pub(crate) mod focus_retention;
pub(crate) mod hover;
pub(crate) mod keyboard;
pub(crate) mod file_drop;
pub(crate) mod text_input;
pub(crate) mod touch;
pub(crate) mod toggle;
pub(crate) mod wheel;
pub(crate) mod view;
pub(crate) mod virtual_grid;
pub(crate) mod virtualizer;
