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
    fn focus(&self, node: &dyn Any);
    fn blur(&self, node: &dyn Any);
    fn select_all(&self, node: &dyn Any);
    /// See [`TextInputHandle::insert_text`]. Backends MUST replace
    /// the active selection (if any), advance the caret to the end
    /// of the inserted text, and fire the input's normal on-change
    /// path so the controlling `Signal` observes the new value.
    fn insert_text(&self, node: &dyn Any, text: &str);
}



