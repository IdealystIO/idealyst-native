//! Per-primitive create/update functions. Each module owns one
//! `Element` kind end-to-end: the create call, any update call,
//! the `Ops` impl for refs (where applicable), and the
//! `make_*_handle` method.
//!
//! Functions take `&mut WebBackend` rather than being inherent
//! methods so each module is a flat file with no `impl WebBackend`
//! ceremony around its bodies. The thin `impl Backend for WebBackend`
//! in `lib.rs` calls into them.

/// Hand a listener closure over to the DOM element it was just attached to.
///
/// `add_event_listener` makes the event target hold a strong reference to its
/// callback, so the element IS the keepalive — nothing on the Rust side needs
/// to root it, and rooting it is actively harmful. These closures used to be
/// parked in a backend-owned `Vec` (`_touch_closures`) that was never cleared,
/// which pinned the function — and, where the platform supports weak
/// references, the Rust closure box behind it, since
/// [`Closure::into_js_value`](wasm_bindgen::closure::Closure::into_js_value)
/// hands reclamation to the JS GC — for the lifetime of the process, long
/// after the element had been detached and collected. An app that mounts
/// interactive elements dynamically (a virtualized list or grid re-slicing as
/// it scrolls) leaked several closures per cell per slice that way.
///
/// Dropping the returned `JsValue` releases the wasm-bindgen heap slot only;
/// the function object itself stays alive as long as the element holds it and
/// becomes collectable together with the element.
///
/// Only for listeners on element-lifetime targets. A listener on `window` /
/// `document` outlives every element, so it needs an explicit removal path
/// instead — see `touch::WINDOW_NET` and `keyboard`.
pub(crate) fn own_listener<T>(closure: wasm_bindgen::closure::Closure<T>)
where
    T: ?Sized + wasm_bindgen::closure::WasmClosure + 'static,
{
    let _ = closure.into_js_value();
}

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
