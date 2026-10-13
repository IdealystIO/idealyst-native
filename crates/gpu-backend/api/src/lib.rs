//! Platform-agnostic preview-backend API.
//!
//! Shared contract between **native backends** (platform shells
//! that translate native events — e.g. winit on desktop, browser
//! DOM on web, UIKit on iOS) and **render backends** (rendering
//! implementations — currently wgpu, future Skia / vello / etc.).
//!
//! # Architecture
//!
//! Two layers communicate through this crate only. Neither layer
//! imports the other's internals.
//!
//! ```text
//!  ┌──────────────────────┐         ┌──────────────────────┐
//!  │   Native backend     │         │   Render backend     │
//!  │   (winit / web /     │         │   (wgpu / Skia /     │
//!  │    UIKit / Android)  │         │    vello / …)        │
//!  │                      │         │                      │
//!  │  - owns event source │   uses  │  - impl Backend      │
//!  │  - owns window       │ ──────► │  - impl EventSink    │
//!  │  - translates events │         │  - owns rendering    │
//!  │  - drives per frame  │         │  - owns interaction  │
//!  └──────────────────────┘         └──────────────────────┘
//!             │                                │
//!             └──────────┬─────────────────────┘
//!                        ▼
//!                ┌──────────────┐
//!                │  render-api  │
//!                │              │
//!                │ - PointerEvent
//!                │ - KeyEvent   │
//!                │ - ScrollEvent│
//!                │ - DeviceProfile
//!                │ - EventSink (trait)
//!                └──────────────┘
//! ```
//!
//! Two consequences:
//!
//! 1. **Parallel work**. Adding a new native shell means
//!    implementing one event-translation crate that depends on
//!    this api crate plus the chosen render backend. Adding a
//!    new render backend means implementing `EventSink` (and
//!    the framework's backend surface). The other side doesn't change.
//!
//! 2. **Cross-mixing**. Any native shell pairs with any render
//!    backend — they only talk through the types here. Each
//!    variant crate (`variant-phone`, …) picks the (native,
//!    render, profile) trio it ships.

pub mod input;
pub mod profile;

pub use input::{
    Key, KeyEvent, KeyModifiers, PointerButton, PointerEvent, PointerId, ScrollEvent,
};
pub use profile::DeviceProfile;

// No time type in the contract. The render backend owns its clock —
// shells don't sample a wall-clock and hand it in. See
// [`EventSink::tick`] below for the rationale.

/// What a render backend must accept from any native shell.
///
/// Coordinates are in **logical** CSS pixels — the shell does
/// the physical-→-logical conversion (dividing by the
/// platform's scale factor, normalizing lines-to-pixels for
/// wheel events, etc.). The render side never sees
/// platform-specific units.
///
/// Calling these methods is the only way a native shell drives
/// the render side. There's no platform-flavor escape hatch:
/// if you can't express your event as one of these, the
/// vocabulary needs widening here in the api crate so all
/// shells benefit.
///
/// The render side, not the shell, feeds the app-wide
/// `user_activity()` heartbeat from `pointer_down`, `scroll` and
/// pressed `key` calls — a shell reports nothing extra for it.
pub trait EventSink {
    fn pointer_down(&mut self, ev: PointerEvent);
    fn pointer_move(&mut self, ev: PointerEvent);
    fn pointer_up(&mut self, ev: PointerEvent);
    /// OS-level cancellation: window lost focus, OS-interrupted
    /// touch, etc. Render side should treat any in-flight
    /// gesture as aborted without firing release actions.
    fn pointer_cancel(&mut self);
    fn scroll(&mut self, ev: ScrollEvent);
    /// Returns `true` if the render side consumed the key (e.g.
    /// a focused TextInput accepted the character or handled
    /// Backspace). Shells can use this to decide whether to let
    /// the key propagate to platform shortcuts.
    ///
    /// Called for presses AND releases ([`KeyEvent::pressed`]): every
    /// key reaches the app-level keyboard sink; only presses reach a
    /// focused text input.
    fn key(&mut self, ev: &KeyEvent) -> bool;
    /// The window / app stopped receiving keys (window blur, app
    /// deactivation, terminal focus-out). The platform will not deliver
    /// the releases of keys still held at that moment, so the render
    /// side tells the app-level keyboard sink to release them. Shells
    /// call this alongside [`pointer_cancel`](Self::pointer_cancel) on
    /// focus loss.
    fn focus_lost(&mut self);
    /// Tell the render side how big the viewport is, in logical
    /// CSS pixels. Called by the shell on startup and on resize.
    /// The render side uses this for layout + on-screen
    /// keyboard / overlay placement.
    fn set_viewport(&mut self, w: f32, h: f32);
    /// Advance per-frame animation state (tweens, momentum
    /// scroll, keyboard slide, caret blink, …). Returns `true`
    /// if anything is still in flight — the shell should
    /// `request_redraw` so the next frame samples the next
    /// step.
    ///
    /// No `now` parameter: the render side owns its monotonic
    /// clock (on wgpu, the framework's installed
    /// `runtime_shared::time` source; anything else on a different
    /// backend) so shells don't have to pick one or thread it
    /// across the API boundary. Cross-target consistency is the
    /// render backend's problem, not the shell's.
    fn tick(&mut self) -> bool;

    // -----------------------------------------------------------------
    // App situation + app-level pointer (added after 2.0.0). Every method
    // below has a default no-op body so existing shells and render
    // backends keep compiling; a shell calls the ones its platform can
    // observe.
    // -----------------------------------------------------------------

    /// Raw relative mouse motion (platform device deltas — unaccelerated
    /// where the platform offers it, e.g. winit's
    /// `DeviceEvent::MouseMotion`), in the same logical-pixel scale as
    /// pointer positions. Only meaningful while the pointer is locked:
    /// the render side turns it into app-level `Move` events carrying
    /// `dx`/`dy` and ignores it otherwise (raw deltas arrive even when
    /// the cursor is outside the window).
    #[allow(unused_variables)]
    fn mouse_motion(&mut self, dx: f32, dy: f32) {}

    /// The keyboard modifier state changed (some platforms — winit —
    /// report modifiers separately from key events). The render side
    /// stamps the latest state onto app-level pointer events.
    #[allow(unused_variables)]
    fn modifiers_changed(&mut self, modifiers: KeyModifiers) {}

    /// The window / app regained input focus — the counterpart of
    /// [`focus_lost`](Self::focus_lost).
    fn focus_gained(&mut self) {}

    /// The app became visible (`true`) or hidden (`false`: minimized,
    /// fully occluded, backgrounded).
    #[allow(unused_variables)]
    fn app_visibility(&mut self, visible: bool) {}

    /// The platform granted (`true`) or dropped (`false`) pointer lock —
    /// the answer to a lock request the render side forwarded to the
    /// shell, or a loss the app didn't ask for (window deactivation,
    /// grab lost). A shell that cannot lock never calls this with `true`.
    #[allow(unused_variables)]
    fn pointer_lock_changed(&mut self, locked: bool) {}
}
