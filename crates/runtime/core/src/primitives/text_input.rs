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

use crate::primitives::key::{KeyEvent, KeyOutcome};
use crate::{Bound, Element, Reactive, Ref, RefFill, Signal};
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

#[cfg(test)]
mod width_tests {
    use super::{measured_width, DEFAULT_WIDTH_PX};

    // Regression: an unconstrained native field measured its content-hugging
    // intrinsic width, so a field showing "Sea" collapsed to a few characters
    // (web's `<input>` holds a stable default box). The fallback must be the
    // framework default, and an explicit/known width must still win.
    #[test]
    fn unconstrained_field_takes_the_default_box_not_its_content() {
        assert_eq!(measured_width(None), DEFAULT_WIDTH_PX);
    }

    #[test]
    fn an_author_or_stretched_width_wins() {
        assert_eq!(measured_width(Some(320.0)), 320.0);
        // Even a width narrower than the default is honored — the author asked.
        assert_eq!(measured_width(Some(80.0)), 80.0);
    }
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

/// Construct a `TextInput`. The `value` signal is the source of
/// truth — the input reflects whatever the signal currently holds.
/// `on_change` fires for every native input event with the new
/// text; the typical pattern is to call `value.set(new_text)`
/// inside the callback (the framework optimizes away the redundant
/// write-back when the signal already matches).
#[cfg(feature = "prim-text-input")]
pub fn text_input<F: Fn(String) + 'static>(
    value: Signal<String>,
    on_change: F,
) -> Bound<TextInputHandle> {
    Bound::new(Element::TextInput {
        value,
        // Born batched — see `reactive::cycle`.
        on_change: Rc::new(move |s: String| crate::cycle(|| on_change(s))),
        on_key_down: None,
        on_blur: None,
        on_focus: None,
        placeholder: Reactive::Static(None),
        secure: Reactive::Static(false),
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<TextInputHandle> {
    /// Placeholder text shown when the input is empty. Takes a `String` for
    /// the common static case (`Static(Some(text))`); for a live placeholder
    /// use [`placeholder_reactive`](Self::placeholder_reactive).
    pub fn placeholder(mut self, text: String) -> Self {
        if let Element::TextInput { placeholder, .. } = &mut self.primitive {
            *placeholder = Reactive::Static(Some(text));
        }
        self
    }

    /// Placeholder from anything coercing into `Reactive<Option<String>>` — a
    /// `Signal`/`rx!` makes the placeholder live (updated in place, no
    /// rebuild). `None` shows no placeholder.
    pub fn placeholder_reactive(
        mut self,
        placeholder_src: impl Into<Reactive<Option<String>>>,
    ) -> Self {
        if let Element::TextInput { placeholder, .. } = &mut self.primitive {
            *placeholder = placeholder_src.into();
        }
        self
    }

    /// Mask the entered text (password entry). Maps to each backend's native
    /// secure-entry mode; the masked-character behaviour is identical
    /// everywhere. Default `false`.
    ///
    /// Accepts anything that coerces into `Reactive<bool>`: a bare `bool`
    /// (`Static`, the common case), a `Signal<bool>`, or `rx!(…)` — a live
    /// source lets the mask toggle at runtime (password show/hide) without
    /// rebuilding the input.
    pub fn secure(mut self, is_secure: impl Into<Reactive<bool>>) -> Self {
        if let Element::TextInput { secure, .. } = &mut self.primitive {
            *secure = is_secure.into();
        }
        self
    }

    /// Bind to a `Ref<TextInputHandle>` for imperative
    /// `focus()`/`blur()`/`select_all()`/`insert_text()` from the parent.
    pub fn bind(mut self, r: Ref<TextInputHandle>) -> Self {
        if let Element::TextInput { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::TextInput(Box::new(move |h| r.fill(h))));
        }
        self
    }

    /// Attach a keyboard hook that fires on every keydown while the
    /// input has focus. Return [`KeyOutcome::PreventDefault`] to
    /// suppress the platform's default behaviour for that key.
    /// See [`primitives::key`](crate::primitives::key) for the
    /// cross-platform contract.
    pub fn on_key_down<F>(mut self, handler: F) -> Self
    where
        F: Fn(&KeyEvent) -> KeyOutcome + 'static,
    {
        if let Element::TextInput { on_key_down, .. } = &mut self.primitive {
            // Born batched — see `reactive::cycle`. Return value (preventDefault)
            // is preserved through the cycle flush.
            *on_key_down = Some(Rc::new(move |e: &KeyEvent| crate::cycle(|| handler(e))));
        }
        self
    }

    /// Attach a blur hook, consulted when the input is about to lose focus via
    /// the dismiss path (an outside tap/click, or programmatic blur). Return
    /// [`BlurOutcome::Keep`] to veto the blur and keep focus (and the keyboard
    /// up on mobile); [`BlurOutcome::Allow`] (or no handler) lets it proceed.
    /// See [`BlurOutcome`] for the per-platform contract.
    pub fn on_blur<F>(mut self, handler: F) -> Self
    where
        F: Fn() -> BlurOutcome + 'static,
    {
        if let Element::TextInput { on_blur, .. } = &mut self.primitive {
            // Born batched — see `reactive::cycle`. The veto return value is
            // preserved through the cycle flush, mirroring `on_key_down`.
            *on_blur = Some(Rc::new(move || crate::cycle(|| handler())));
        }
        self
    }

    /// Attach a focus-change hook: `handler(true)` fires when the input gains
    /// keyboard focus, `handler(false)` when it loses it. Unlike
    /// [`on_blur`](Self::on_blur) this is a plain notification (no veto) — its
    /// purpose is to let a parent drive focus-dependent chrome the input itself
    /// can't, e.g. the idea-ui `Field` lighting its bordered shell's focus ring
    /// for an adorned (borderless-input) layout.
    pub fn on_focus<F>(mut self, handler: F) -> Self
    where
        F: Fn(bool) + 'static,
    {
        if let Element::TextInput { on_focus, .. } = &mut self.primitive {
            // Born batched — see `reactive::cycle` — so a signal write inside the
            // handler coalesces with the focus event's other work.
            *on_focus = Some(Rc::new(move |f: bool| crate::cycle(|| handler(f))));
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive::Signal;
    use std::cell::Cell;

    // Regression: `on_focus` is the event the idea-ui `Field` uses to light an
    // ADORNED field's shell focus ring (the bordered shell can't receive the
    // borderless input's FOCUSED state on its own). Pin the builder contract:
    // absent by default, installed by `.on_focus`, and fired with the bool.
    #[test]
    fn on_focus_builder_installs_a_bool_notifier() {
        let val = Signal::new(String::new());
        let ti = text_input(val, |_| {});
        // Default: no focus notifier.
        match &ti.primitive {
            Element::TextInput { on_focus, .. } => assert!(on_focus.is_none(), "default is None"),
            _ => panic!("expected TextInput"),
        }
        // After `.on_focus`, the handler is installed and forwards the bool.
        let seen: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
        let seen2 = seen.clone();
        let ti = ti.on_focus(move |f| seen2.set(Some(f)));
        match &ti.primitive {
            Element::TextInput { on_focus: Some(h), .. } => {
                h(true);
                assert_eq!(seen.get(), Some(true), "focus fires true");
                h(false);
                assert_eq!(seen.get(), Some(false), "blur fires false");
            }
            _ => panic!("expected on_focus to be Some after .on_focus()"),
        }
    }
}
