//! Keyboard-focus channel for containers — `view` / `pressable` `on_focus`.
//!
//! A focus handler fires `true` when **keyboard focus** lands on the node
//! or on anything inside it, and `false` when it leaves that subtree (or
//! stops being keyboard focus). It is focus-*within*: a `view` wrapping a
//! button hears the button's focus, which is what a wrapping component
//! (`HoverCard`, a toolbar group) needs — the wrapper itself is never a
//! tab stop.
//!
//! "Keyboard focus" is the focus a focus ring shows: the framework's
//! `StateBits::FOCUSED` rule, CSS `:focus-visible`. Tab / arrow
//! traversal and programmatic focus count; a mouse click that happens to
//! make a button first responder does not (it would otherwise open
//! focus-driven UI on every click). A text field counts on any focus, as
//! `:focus-visible` does — the ring marks where typing goes.
//!
//! The vocabulary forwards only *changes* (a backend that reports `false`
//! twice, or `false` before any `true`, is deduplicated), so a handler can
//! treat every call as a transition.
//!
//! Where a platform has no keyboard focus for an element the handler never
//! fires: touch-only devices without a hardware keyboard, and backends
//! with no focus model (see each backend's `InputOps::install_focus_handler`).

use std::rc::Rc;

/// Installed via the builder's `on_focus`. The `bool` argument is the new
/// state: `true` when keyboard focus enters the node's subtree, `false`
/// when it leaves. Born batched (one reactive cycle per call) like the
/// hover handler.
pub type FocusWithinHandler = Rc<dyn Fn(bool)>;
