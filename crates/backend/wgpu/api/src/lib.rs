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
//!                │ backend-wgpu │
//!                │     -api     │
//!                │              │
//!                │ - PointerEvent
//!                │ - KeyEvent   │
//!                │ - ScrollEvent│
//!                │ - SimulatedPlatform
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
//!    `framework_core::Backend`). The other side doesn't change.
//!
//! 2. **Cross-mixing**. Any native shell pairs with any render
//!    backend — they only talk through the types here. Each
//!    variant crate (`backend-wgpu-phone`, …) picks the (native,
//!    render, profile) trio it ships.

pub mod input;
pub mod platform;
pub mod profile;

pub use input::{
    Key, KeyEvent, KeyModifiers, PointerButton, PointerEvent, PointerId, ScrollEvent,
};
pub use platform::SimulatedPlatform;
pub use profile::DeviceProfile;

use std::time::Instant;

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
    fn key(&mut self, ev: &KeyEvent) -> bool;
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
    fn tick(&mut self, now: Instant) -> bool;
}
