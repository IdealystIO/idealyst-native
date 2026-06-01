//! Ambient drawer chrome — lets any screen or component render its own
//! menu button (hamburger) instead of relying on a backend-native nav
//! bar.
//!
//! A drawer navigator publishes a [`DrawerChrome`] at init. Screens are
//! mounted as the navigator's *content* (not as slot closures), so they
//! don't receive `SlotProps`; this thread-local ambient is how they
//! reach the two things a page-level header needs:
//!
//! - [`DrawerChrome::open`] — open the drawer (the hamburger's action).
//! - [`DrawerChrome::collapse_below`] — the viewport width below which
//!   the drawer is modal/collapsed (a menu button should show). Compare
//!   [`crate::viewport_size`] against it inside a `ui!` region — the
//!   visible `.get()` keeps the region reactive, so the button appears
//!   and disappears as the viewport crosses the pin breakpoint. On
//!   mobile the drawer is always modal, so this is `f32::INFINITY`.
//!
//! This mirrors [`ambient_scroll_context`](super::ambient_scroll_context):
//! same publish-at-init / read-from-anywhere shape.
//!
//! ```ignore
//! use runtime_core::primitives::navigator::ambient_drawer;
//!
//! // Inside a screen's header component:
//! if let Some(d) = ambient_drawer() {
//!     let open = d.open.clone();
//!     let below = d.collapse_below;
//!     ui! {
//!         if viewport_size().get().width < below {
//!             // a pressable hamburger that calls `open()`
//!         }
//!     }
//! }
//! ```

use std::cell::RefCell;
use std::rc::Rc;

/// Handle a screen/component uses to render its own drawer menu button.
#[derive(Clone)]
pub struct DrawerChrome {
    /// Open the drawer. No-op if it's already open or pinned.
    pub open: Rc<dyn Fn()>,
    /// The viewport width (dp) below which the drawer collapses to a
    /// modal — i.e. a menu button should show. Compare it against
    /// [`crate::viewport_size`]`().get().width` **inside a `ui!`
    /// control-flow region**: the visible `.get()` makes the region
    /// reactive, so the button appears/disappears as the viewport
    /// crosses the pin breakpoint. On mobile the drawer is always
    /// modal, so this is `f32::INFINITY` (any width is below it).
    ///
    /// ```ignore
    /// if viewport_size().get().width < chrome.collapse_below { /* menu button */ }
    /// ```
    pub collapse_below: f32,
}

thread_local! {
    static AMBIENT_DRAWER: RefCell<Option<DrawerChrome>> = const { RefCell::new(None) };
}

/// Read the current drawer chrome. `None` when called from outside any
/// drawer navigator's subtree, or before its `init` has published.
pub fn ambient_drawer() -> Option<DrawerChrome> {
    AMBIENT_DRAWER.with(|c| c.borrow().clone())
}

/// SDK-only — publish a navigator's drawer chrome as the thread-local
/// ambient. `Some(chrome)` from a drawer's per-backend `init`; `None`
/// from `release`.
///
/// Hidden from rustdoc — author code reads via [`ambient_drawer`].
#[doc(hidden)]
pub fn _set_ambient_drawer(chrome: Option<DrawerChrome>) {
    AMBIENT_DRAWER.with(|c| *c.borrow_mut() = chrome);
}
