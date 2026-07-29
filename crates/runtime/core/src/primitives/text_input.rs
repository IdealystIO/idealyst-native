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
use std::rc::Rc;

// The data/handle/Ops types of this primitive moved to `runtime-shared`
// (the walker-free half); this file keeps the Element/Bound builder
// surface (and its tests). The wildcard re-export preserves every old
// path.
pub use runtime_shared::primitives::text_input::*;

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
