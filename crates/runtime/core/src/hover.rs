//! Hover (pointer-over) channel — the desktop/web counterpart to touch.
//!
//! A hover handler fires `true` when the pointer **enters** the view and
//! `false` when it **leaves**. It is the lowest-level "is the cursor over
//! me" primitive; higher-level affordances (hover tooltips, hover
//! highlights an app draws itself) build on top of it.
//!
//! Hover is a **pointer** concept: it exists on backends that have a
//! cursor (web, macOS) and is a **no-op on touch-only backends** (iOS,
//! Android) — a finger is either down or not, there is no "hovering".
//! For the touch affordance, pair `on_hover` with a `long_press`
//! recognizer via [`crate::Bound::on_touch`] (this is exactly what the
//! `idea-ui` `Tooltip` does: hover on desktop, long-press on mobile).
//!
//! Unlike touch/wheel there is no response value — a hover transition
//! never "consumes" or competes with native gestures, so the handler
//! returns `()`.

use std::rc::Rc;

/// Installed via [`crate::Bound::on_hover`]. The `bool` argument is the
/// new hover state: `true` when the pointer enters the view, `false`
/// when it leaves. Born batched (one reactive cycle per call) just like
/// the touch/wheel handlers.
pub type HoverHandler = Rc<dyn Fn(bool)>;
