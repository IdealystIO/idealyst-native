//! Scroll-context primitive — the framework-level abstraction for
//! "the scroll surface this content sits inside."
//!
//! Navigator SDKs publish a [`ScrollContext`] when their screen-mount
//! area (the body of a drawer, the body of a stack, etc.) is a
//! scrollable surface. The SDK's per-backend handler reads the
//! native scroll surface's dimensions + offset and reflects them
//! into the context's reactive signals; programmatic scrolling
//! routes through the context's `scroll_to` dispatcher.
//!
//! Author code reads via [`ambient_scroll_context`]:
//!
//! ```ignore
//! use runtime_core::primitives::navigator::ambient_scroll_context;
//!
//! effect!({
//!     let Some(ctx) = ambient_scroll_context() else { return };
//!     let y = ctx.scroll_y.get();
//!     // … react to scroll
//! });
//! ```
//!
//! ## Why ambient
//!
//! Slot closures (sidebar, top/bottom/trailing chrome) receive a
//! `SlotProps` that can carry the scroll context directly; nothing
//! more is needed there. But **screens** mount inside the
//! navigator's outlet without `SlotProps` access — and screens are
//! exactly where things like a docs-page TOC scroll-spy live. The
//! ambient lookup hands those screens the same `ScrollContext` the
//! navigator's chrome sees, without plumbing it through every
//! `layout()` / page-render signature.
//!
//! ## Two-axis from day one
//!
//! Most chrome cases are vertical-only — a docs drawer body, a
//! stack's content area. But the type supports both axes so a
//! future horizontal carousel screen or a 2D-pan map can share the
//! same primitive without an API split. Backends that only
//! observe one axis populate the other with const-zero signals.

use std::cell::RefCell;
use std::rc::Rc;

use crate::reactive::Signal;

/// The reactive bundle a navigator publishes about its scroll
/// surface. All signals stay valid for the navigator's lifetime;
/// reading inside an `effect!` subscribes the surrounding scope to
/// position / dimension changes.
///
/// `Clone` is cheap — every field is either a `Copy` Signal handle
/// or an `Rc`.
#[derive(Clone)]
pub struct ScrollContext {
    /// Scroll surface's top edge in window/viewport coordinates.
    /// Subtract from [`crate::ViewHandle::absolute_frame()`]
    /// results (also window-relative) to convert into
    /// surface-local coordinates.
    pub viewport_top: Signal<f32>,
    /// Scroll surface's left edge in window/viewport coordinates.
    /// Symmetric with `viewport_top` for the horizontal axis.
    pub viewport_left: Signal<f32>,

    /// Visible (clipped) viewport height — typically
    /// `clientHeight` on web, the equivalent native bounds
    /// otherwise.
    pub height: Signal<f32>,
    /// Visible (clipped) viewport width.
    pub width: Signal<f32>,

    /// Current vertical scroll offset within the content.
    pub scroll_y: Signal<f32>,
    /// Current horizontal scroll offset within the content.
    pub scroll_x: Signal<f32>,

    /// Total content height including overflow. For
    /// "at the bottom" detection: `scroll_y + height >= scroll_height`.
    pub scroll_height: Signal<f32>,
    /// Total content width including overflow.
    pub scroll_width: Signal<f32>,

    /// Programmatic scroll dispatcher — backends clamp `x` / `y`
    /// to the valid range automatically. Use `(0.0, target_y)` for
    /// vertical-only scroll, `(target_x, 0.0)` for horizontal-only.
    pub scroll_to: Rc<dyn Fn(f32, f32)>,
}

// ---------------------------------------------------------------------------
// Ambient publication
// ---------------------------------------------------------------------------
//
// Thread-local storage of the most recently-published scroll
// context. SDK handlers populate this via [`_set_ambient_scroll_context`]
// at init time; screens and slot closures read via
// [`ambient_scroll_context`].
//
// Multi-navigator note: in a nested-navigator setup (drawer
// containing a stack containing a tab navigator, for example), the
// "innermost" scrollable navigator's context wins — whichever one
// most recently published. For UX, that's usually what you want:
// a TOC inside a stack screen tracks the stack's scroll, not the
// outer drawer's. Future work could expose a context stack (push /
// pop) similar to [`AmbientNavGuard`](super::AmbientNavGuard) if
// patterns demand it.

thread_local! {
    static AMBIENT: RefCell<Option<ScrollContext>> = const { RefCell::new(None) };
}

/// Read the current scroll context. Returns `None` when called
/// from outside any scrollable navigator's subtree, or before any
/// such navigator has finished its `init`.
pub fn ambient_scroll_context() -> Option<ScrollContext> {
    AMBIENT.with(|c| c.borrow().clone())
}

/// SDK-only — publish a navigator's scroll context as the
/// thread-local ambient. Pass `Some(ctx)` from a navigator's
/// per-backend `init` after constructing the context; pass `None`
/// from the navigator's `release` to clear.
///
/// Hidden from rustdoc because author code shouldn't reach for
/// this — it's an SDK-substrate primitive.
#[doc(hidden)]
pub fn _set_ambient_scroll_context(ctx: Option<ScrollContext>) {
    AMBIENT.with(|c| *c.borrow_mut() = ctx);
}
