//! TextInput primitive (controlled).
//!
//! Backed by `<input type="text">` on web, `UITextField` on iOS,
//! `EditText` on Android. The value is controlled — the parent owns
//! a `Signal<String>` that the framework subscribes to and writes to
//! the native widget; native input events fire `on_change` which the
//! parent uses to update the signal. Cyclic but stable: widgets
//! no-op when set to their current value.
//!
//! Why controlled by default? It matches the rest of the framework's
//! reactive shape — every input has a single source of truth (a
//! signal), and the parent decides how/whether to accept incoming
//! values (e.g. validation, transformation). Uncontrolled variants
//! can be added later if a real need arises.

use std::any::Any;
use std::rc::Rc;

/// Decision returned from an [`on_blur`](Bound::on_blur) handler when an input
/// is about to lose focus via the dismiss path (an outside tap / click, or a
/// programmatic blur). Lets the author veto the blur — e.g. keep focus while a
/// field is mid-validation.
///
/// Scope: this governs the "drop to no-focus" path only. Tapping ANOTHER input
/// always transfers focus (there is nowhere for focus to stay), so `Keep` means
/// "don't dismiss to nothing", not "trap focus forever".
///
/// Platform contract (CLAUDE.md §7 — same observable result, native mechanism):
/// - **iOS**: `UITextFieldDelegate.textFieldShouldEndEditing:` returns `NO` on
///   `Keep` — a native veto, so the outside-tap `endEditing:` respects it.
/// - **macOS**: the outside-click handler consults this before resigning.
/// - **web**: `blur` is not preventable by spec, so `Keep` re-`focus()`es the
///   input (one frame of flicker; focus is retained).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BlurOutcome {
    /// Let the blur proceed (default when there is no handler).
    Allow,
    /// Veto the blur — keep focus (and, on mobile, the keyboard up).
    Keep,
}

/// Default preferred content width (in px) for an unconstrained `text_input`.
///
/// Web renders `<input type=text>` at the UA default `size=20` — a stable
/// ~150–175px box that does NOT shrink to its content. Native fields have no
/// such default: their measurer reports `intrinsicContentSize`, which hugs the
/// current text (so a field showing "Sea" collapses to a few characters — the
/// reported "no sensible min width" bug). Native `create_text_input` measurers
/// fall back to this width when the author sets no explicit `width`/`block`,
/// giving every backend web's stable default box (Rule #7). An author `width`,
/// `width: 100%`, or flex-stretch still wins (the measurer only uses this when
/// Taffy passes no known width). `200` reads as a comfortable default field and
/// sits just above web's UA width; it does not scale with font size (a
/// documented approximation — an explicit `width` covers the rare case that
/// matters).
pub const DEFAULT_WIDTH_PX: f32 = 200.0;

/// Resolve a `text_input`'s measured preferred width for a native backend's
/// `measure_fn`. An author-constrained width — `width`, `width: 100%`, or a
/// flex-stretch that Taffy resolved to a definite size — arrives as
/// `known_width = Some(px)` and always wins. Otherwise the field takes the
/// stable [`DEFAULT_WIDTH_PX`] box instead of hugging its content, matching
/// web's default `<input>`. Shared by the macOS and iOS field measurers so the
/// fallback is defined once (Rule #7).
pub fn measured_width(known_width: Option<f32>) -> f32 {
    known_width.unwrap_or(DEFAULT_WIDTH_PX)
}


/// Shared handler type carried into the backend `create_text_input`. Aliased so
/// the Backend trait signature stays readable. Mirrors [`KeyDownHandler`].
///
/// [`KeyDownHandler`]: crate::primitives::key::KeyDownHandler
pub type BlurHandler = Rc<dyn Fn() -> BlurOutcome>;

/// Focus-change notification carried into the backend. Fires `true` when the
/// input gains keyboard focus and `false` when it loses it — the symmetric
/// partner of [`BlurHandler`], but a plain notification (no veto). A parent
/// uses it to drive focus-dependent chrome it can't otherwise observe: e.g.
/// the idea-ui `Field` lights its bordered SHELL's focus ring when the inner
/// (borderless) input focuses, since the shell `view` never receives the
/// input's `FOCUSED` state itself.
pub type FocusHandler = Rc<dyn Fn(bool)>;

/// Handle exposed to a parent via `Ref<TextInputHandle>`. Backends
/// implement the ops trait below to make `focus()`, `blur()`,
/// `select_all()`, and `insert_text()` work.
#[derive(Clone)]
pub struct TextInputHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn TextInputOps,
}

impl TextInputHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn TextInputOps) -> Self {
        Self { node, ops }
    }

    /// Move keyboard focus to this input.
    ///
    /// Safe to call at any point after the handle exists — including from
    /// `on_handle` / a `Ref` fill right after mount, before the native node
    /// is in a window or document (a modal's content, a portal, a
    /// `presence` child). If the node can't take focus yet, the backend
    /// remembers the request and focuses it when the node is attached; a
    /// [`blur`](Self::blur) before then cancels it. No timer is needed. See
    /// [`TextInputOps::focus`].
    pub fn focus(&self) {
        self.ops.focus(&*self.node);
    }

    /// Drop keyboard focus from this input.
    pub fn blur(&self) {
        self.ops.blur(&*self.node);
    }

    /// Select all the current text. Useful for "tap to edit"
    /// patterns where the entire value should be replaced on
    /// focus.
    pub fn select_all(&self) {
        self.ops.select_all(&*self.node);
    }

    /// Replace the current selection (or insert at the caret if no
    /// selection) with `text`, then place the caret immediately
    /// after the inserted text. Fires the same on-change signal
    /// path a real keystroke would, so the controlling `Signal`
    /// stays in sync.
    ///
    /// Typical use: from inside an [`on_key_down`](crate::primitives::key)
    /// handler that returns [`KeyOutcome::PreventDefault`], to
    /// substitute custom text for the suppressed default behaviour
    /// (e.g. inserting four spaces for Tab in a code editor).
    pub fn insert_text(&self, text: &str) {
        self.ops.insert_text(&*self.node, text);
    }
}

pub trait TextInputOps {
    /// Give the field keyboard focus. **Attach-safe**: when the node can
    /// take focus now, focus it now. When it can't because it isn't
    /// attached yet (no window / not in the document — realize builds
    /// subtrees before inserting them, and portals/presence attach later
    /// still), record a pending focus on THAT node and apply it when the
    /// node is attached, exactly once. A later [`blur`](Self::blur) before
    /// attach cancels the pending focus. Backends must not poll with
    /// timers; they hook their own attach notification. Every backend
    /// gives the same observable result: the field ends up focused.
    fn focus(&self, node: &dyn Any);
    /// Drop keyboard focus. Also cancels a pending (pre-attach)
    /// [`focus`](Self::focus).
    fn blur(&self, node: &dyn Any);
    fn select_all(&self, node: &dyn Any);
    /// See [`TextInputHandle::insert_text`]. Backends MUST replace
    /// the active selection (if any), advance the caret to the end
    /// of the inserted text, and fire the input's normal on-change
    /// path so the controlling `Signal` observes the new value.
    fn insert_text(&self, node: &dyn Any, text: &str);
}




/// The pending half of the attach-safe [`TextInputOps::focus`] contract, as
/// one shared state machine every backend drives with its own node type and
/// its own "is it attached?" / attach notification (DOM `isConnected` + the
/// backend's insert, `-[NSView window]` + `viewDidMoveToWindow`, UIKit
/// `didMoveToWindow`, Android `isAttachedToWindow` + `onViewAttachedToWindow`,
/// GTK `is_realized` + `map`). Defined once so every backend answers the same
/// questions the same way (Rule 7).
///
/// ONE slot, not a list. Focus is a single thing, so the latest request wins:
///
/// - a request on an attached node focuses now and DROPS any pending one (an
///   older field must not steal focus back when it attaches later);
/// - a request on a detached node replaces whatever was pending (when several
///   `autofocus` fields mount together, the last one wins — HTML's rule);
/// - a `blur()` on the pending node cancels it ([`cancel`](Self::cancel));
/// - on attach, the pending node is taken exactly once
///   ([`take_attached`](Self::take_attached)).
///
/// The single slot also bounds what a node that never attaches (built, then
/// discarded) can keep alive to one.
#[derive(Debug)]
pub struct PendingFocus<K> {
    slot: Option<K>,
}

/// What a backend does with a focus request ([`PendingFocus::request`]).
#[derive(Debug, PartialEq, Eq)]
pub enum FocusRequest<K> {
    /// The node is attached: focus it now.
    Now(K),
    /// Recorded; focus it when it attaches.
    Deferred,
}

impl<K> Default for PendingFocus<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K> PendingFocus<K> {
    pub const fn new() -> Self {
        Self { slot: None }
    }

    /// A `focus()` on `node`, which is (`attached`) or isn't yet in a
    /// window / document.
    pub fn request(&mut self, node: K, attached: bool) -> FocusRequest<K> {
        if attached {
            self.slot = None;
            FocusRequest::Now(node)
        } else {
            self.slot = Some(node);
            FocusRequest::Deferred
        }
    }

    /// A `blur()` on the node `is` matches: drop its pending focus. Another
    /// node's pending focus is left alone.
    pub fn cancel(&mut self, is: impl FnOnce(&K) -> bool) {
        if self.slot.as_ref().is_some_and(is) {
            self.slot = None;
        }
    }

    /// An attach happened: if the pending node is now `attached`, take it
    /// (to focus) — exactly once. Otherwise it keeps waiting.
    pub fn take_attached(&mut self, attached: impl FnOnce(&K) -> bool) -> Option<K> {
        if self.slot.as_ref().is_some_and(attached) {
            self.slot.take()
        } else {
            None
        }
    }

    /// Whether a focus is waiting for its node to attach.
    pub fn is_pending(&self) -> bool {
        self.slot.is_some()
    }

    /// Whether the pending node is the one `is` matches.
    pub fn is_pending_for(&self, is: impl FnOnce(&K) -> bool) -> bool {
        self.slot.as_ref().is_some_and(is)
    }
}

#[cfg(test)]
mod pending_focus_tests {
    use super::{FocusRequest, PendingFocus};

    #[test]
    fn an_attached_node_focuses_now_and_records_nothing() {
        let mut p = PendingFocus::new();
        assert_eq!(p.request(1, true), FocusRequest::Now(1));
        assert!(!p.is_pending());
    }

    #[test]
    fn a_detached_node_focuses_once_when_it_attaches() {
        let mut p = PendingFocus::new();
        assert_eq!(p.request(1, false), FocusRequest::Deferred);
        assert_eq!(p.take_attached(|_| false), None, "still detached: keeps waiting");
        assert!(p.is_pending());
        assert_eq!(p.take_attached(|&n| n == 1), Some(1));
        assert_eq!(p.take_attached(|_| true), None, "exactly once");
    }

    #[test]
    fn blur_before_attach_cancels_only_its_own_node() {
        let mut p = PendingFocus::new();
        p.request(1, false);
        p.cancel(|&n| n == 2);
        assert!(p.is_pending_for(|&n| n == 1), "another node's blur leaves it");
        p.cancel(|&n| n == 1);
        assert!(!p.is_pending());
        assert_eq!(p.take_attached(|_| true), None);
    }

    #[test]
    fn the_latest_request_wins() {
        let mut p = PendingFocus::new();
        p.request(1, false);
        p.request(2, false);
        assert_eq!(p.take_attached(|_| true), Some(2), "last autofocus mounted wins");
        p.request(3, false);
        assert_eq!(p.request(4, true), FocusRequest::Now(4));
        assert_eq!(p.take_attached(|_| true), None, "an immediate focus supersedes a pending one");
    }
}
