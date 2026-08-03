//! Shared wiring for the *typed* value inputs (`TimeInput`,
//! `DateInput`, `DateTimeInput`): a `Field` whose text is parsed into
//! a typed value as the user types.
//!
//! Contract (same for every consumer):
//! - **Commit on valid**: each edit is parsed; a valid parse commits
//!   `Some(value)` via `on_commit`, an emptied field commits `None`.
//! - **Error while invalid**: unparseable non-empty text sets a live
//!   error (the consumer's message) but does NOT clobber the host's
//!   value — the last committed value stands until the text parses
//!   again. With a mask, a valid in-progress PREFIX of the format is
//!   exempt while focused (every keystroke passes through one; erroring
//!   there mounts the error line and resizes the field on the first
//!   digit) — blur re-raises the error if the text is left partial.
//! - **Normalize on blur**: leaving the field rewrites valid text to
//!   its canonical rendering (`3/4/26` → what `render` returns).
//! - **Follow external writes**: a host/programmatic value write that
//!   doesn't match the current text re-renders the text — but text the
//!   user typed that parses to the SAME value is left alone (no cursor
//!   jump mid-typing).
//! - **Smart typing (mask)**: when the spec carries a
//!   [`crate::date_mask::Mask`], appended digits auto-insert the
//!   format's delimiters and jump segments as soon as they're
//!   unambiguous, and Tab completes a valid partial segment in place
//!   (see the mask module docs for the full rules). Deletions,
//!   mid-string edits and non-conforming text bypass the mask and fall
//!   back to the lenient parse.
//!
//! Not a component — a wiring bundle the consumers splat into their
//! `Field(...)` call.

use std::rc::Rc;

use runtime_core::primitives::key::{KeyEvent, KeyOutcome};
use runtime_core::{effect, signal, Reactive, Signal};

use crate::date_mask::{Mask, TabAction};

/// What a typed input needs to describe its value type.
pub(crate) struct TypedFieldSpec<T: Copy + PartialEq + 'static> {
    /// The host's controlled value.
    pub value: Signal<Option<T>>,
    /// Fires on each commit: `Some` on a valid parse, `None` when the
    /// field is emptied. The host writes `value` in response.
    pub on_commit: Rc<dyn Fn(Option<T>)>,
    /// Text → value. `None` = invalid.
    pub parse: Rc<dyn Fn(&str) -> Option<T>>,
    /// Value → canonical text.
    pub render: Rc<dyn Fn(T) -> String>,
    /// Error message shown while the text is non-empty and unparseable.
    pub invalid_message: String,
    /// The host's own error, layered UNDER the invalid-parse error
    /// (a parse error is more immediate than e.g. a server validation).
    pub host_error: Reactive<Option<String>>,
    /// Smart-typing mask built from the same format string as
    /// `parse`/`render` (see [`crate::date_mask`]): auto-inserts
    /// delimiters on appended digits and gives Tab its
    /// segment-completing behavior. `None` = plain free-text field.
    pub mask: Option<Rc<Mask>>,
}

/// The pieces to splat into a `Field`.
pub(crate) struct TypedFieldWiring {
    /// Bind as the Field's `value`.
    pub text: Signal<String>,
    /// Bind as the Field's `error`.
    pub error: Reactive<Option<String>>,
    /// Bind as the Field's `on_change`.
    pub on_change: Rc<dyn Fn(String)>,
    /// Bind as the Field's `on_focus_change`.
    pub on_focus_change: Rc<dyn Fn(bool)>,
    /// Bind as the Field's `on_key_down` (present only when the spec
    /// carries a mask): Tab completes the active segment in place.
    pub on_key_down: Option<Rc<dyn Fn(&KeyEvent) -> KeyOutcome>>,
}

pub(crate) fn typed_field_wiring<T: Copy + PartialEq + 'static>(
    spec: TypedFieldSpec<T>,
) -> TypedFieldWiring {
    let TypedFieldSpec { value, on_commit, parse, render, invalid_message, host_error, mask } =
        spec;

    let text: Signal<String> =
        signal(value.peek().map(|v| (render)(v)).unwrap_or_default());
    let invalid: Signal<bool> = signal(false);

    // Follow external value writes. Semantic comparison against the
    // current text decides whether to rewrite: after OUR commit the
    // host's write parses back equal, so typed-but-uncanonical text
    // (`3/4/26` mid-typing) survives; a genuinely different external
    // write re-renders. `peek` on `text` — this effect must react to
    // VALUE writes only, or every keystroke would loop through it.
    {
        let parse = parse.clone();
        let render = render.clone();
        effect!({
            let v = value.get();
            let current = (parse)(&text.peek());
            if v != current {
                match v {
                    Some(v) => text.set((render)(v)),
                    None => text.set(String::new()),
                }
                invalid.set(false);
            }
        });
    }

    // Parse/commit `s` and write it as the field text. Shared by the
    // edit path and the mask's Tab-completion. `force` = write the text
    // signal even when the value is unchanged — the mask REJECTING a
    // character leaves the signal at its old value while the native
    // input already shows the stray char, and only a forced
    // notification makes the backend rewrite the input back.
    let apply_text: Rc<dyn Fn(String, bool)> = {
        let parse = parse.clone();
        let on_commit = on_commit.clone();
        let mask = mask.clone();
        Rc::new(move |s: String, force: bool| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                invalid.set(false);
                if value.peek().is_some() {
                    (on_commit)(None);
                }
            } else {
                match (parse)(trimmed) {
                    Some(v) => {
                        invalid.set(false);
                        if value.peek() != Some(v) {
                            (on_commit)(Some(v));
                        }
                    }
                    None => {
                        // A masked in-progress prefix ("07/" on the way
                        // to a date) is normal typing, not an error —
                        // flagging it would mount the error line and
                        // resize the field on the first keystroke. Blur
                        // re-raises it if the text is left partial.
                        let mid_typing = mask
                            .as_ref()
                            .is_some_and(|m| m.is_incomplete_prefix(trimmed));
                        invalid.set(!mid_typing);
                    }
                }
            }
            if force {
                text.set_always(s);
            } else {
                text.set(s);
            }
        })
    };

    let on_change: Rc<dyn Fn(String)> = {
        let apply_text = apply_text.clone();
        let mask = mask.clone();
        Rc::new(move |s: String| match &mask {
            Some(m) => {
                let masked = m.feed(&text.peek(), &s);
                // Forced write: `masked` may equal the signal's current
                // text (a rejected char) and the input still needs the
                // rewrite.
                (apply_text)(masked, true);
            }
            None => (apply_text)(s, false),
        })
    };

    let on_focus_change: Rc<dyn Fn(bool)> = {
        let parse = parse.clone();
        let render = render.clone();
        Rc::new(move |focused: bool| {
            if !focused {
                // Blur: canonicalize valid text in place. Invalid text is
                // left as typed — the error explains why, and rewriting
                // would destroy what the user is mid-way through fixing.
                let current = text.peek();
                match (parse)(&current) {
                    Some(v) => {
                        let canonical = (render)(v);
                        if current != canonical {
                            text.set(canonical);
                        }
                    }
                    // Leaving non-empty unparseable text DOES error —
                    // including the masked prefixes that were forgiven
                    // mid-typing ("07/" abandoned half-way).
                    None => {
                        if !current.trim().is_empty() {
                            invalid.set(true);
                        }
                    }
                }
            }
        })
    };

    let error: Reactive<Option<String>> = Reactive::Dynamic(Rc::new(move || {
        if invalid.get() {
            Some(invalid_message.clone())
        } else {
            host_error.get()
        }
    }));

    // Tab-to-next-segment (mask only): a valid partial segment
    // completes in place and focus STAYS; an uncommittable partial
    // (month "0") swallows the Tab; everything else falls through to
    // the platform's focus move. Modified Tab (Shift-Tab back-nav etc.)
    // and a caret anywhere but the end always fall through — the mask
    // only ever reasons about appended-at-end text.
    let on_key_down: Option<Rc<dyn Fn(&KeyEvent) -> KeyOutcome>> = mask.map(|m| {
        let apply_text = apply_text.clone();
        Rc::new(move |e: &KeyEvent| {
            if e.key != "Tab" || e.shift || e.ctrl || e.alt || e.meta {
                return KeyOutcome::Default;
            }
            let current = text.peek();
            let end = current.encode_utf16().count();
            if e.selection_start != end || e.selection_end != end {
                return KeyOutcome::Default;
            }
            match m.tab(&current) {
                TabAction::Complete(t) => {
                    (apply_text)(t, false);
                    KeyOutcome::PreventDefault
                }
                TabAction::Swallow => KeyOutcome::PreventDefault,
                TabAction::Pass => KeyOutcome::Default,
            }
        }) as Rc<dyn Fn(&KeyEvent) -> KeyOutcome>
    });

    TypedFieldWiring { text, error, on_change, on_focus_change, on_key_down }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::{parse_date, CivilDate};
    use idea_theme::testing::{commit, with_test_world};
    use std::cell::RefCell;

    fn date_spec(
        value: Signal<Option<CivilDate>>,
        committed: Rc<RefCell<Vec<Option<CivilDate>>>>,
    ) -> TypedFieldSpec<CivilDate> {
        TypedFieldSpec {
            value,
            on_commit: Rc::new({
                let committed = committed.clone();
                move |v| {
                    committed.borrow_mut().push(v);
                    value.set(v);
                }
            }),
            parse: Rc::new(|s| parse_date(s, "YYYY-MM-DD")),
            render: Rc::new(|d| crate::date::format_date(d, "YYYY-MM-DD")),
            invalid_message: "Invalid date".into(),
            host_error: Reactive::Static(None),
            mask: None,
        }
    }

    /// A masked spec, as the real inputs build it: mask and
    /// parse/render share one format string.
    fn masked_date_spec(
        value: Signal<Option<CivilDate>>,
        committed: Rc<RefCell<Vec<Option<CivilDate>>>>,
    ) -> TypedFieldSpec<CivilDate> {
        TypedFieldSpec {
            mask: Some(Rc::new(Mask::new("YYYY-MM-DD"))),
            ..date_spec(value, committed)
        }
    }

    /// Simulate the native input reporting one appended keystroke.
    /// Flushes after the edit — each real keystroke is its own turn,
    /// and the mask's `text.peek()` reads the committed value.
    fn key(w: &TypedFieldWiring, ch: char) {
        (w.on_change)(format!("{}{ch}", w.text.peek()));
        commit();
    }

    fn tab(w: &TypedFieldWiring) -> KeyOutcome {
        let end = w.text.peek().encode_utf16().count();
        (w.on_key_down.as_ref().expect("masked wiring exposes on_key_down"))(&KeyEvent {
            key: "Tab".into(),
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
            selection_start: end,
            selection_end: end,
        })
    }

    #[test]
    fn commits_on_valid_parse_and_errors_on_garbage() {
        with_test_world(|| {
            let value = signal(None);
            let committed = Rc::new(RefCell::new(Vec::new()));
            let w = typed_field_wiring(date_spec(value, committed.clone()));

            (w.on_change)("2026-03-07".into());
            commit();
            assert_eq!(*committed.borrow(), vec![Some(CivilDate::new(2026, 3, 7).unwrap())]);
            assert_eq!(w.error.get(), None);

            (w.on_change)("2026-03-0x".into());
            commit();
            assert_eq!(committed.borrow().len(), 1, "invalid text must not commit");
            assert_eq!(w.error.get(), Some("Invalid date".into()));
            assert_eq!(value.peek(), CivilDate::new(2026, 3, 7), "last valid value stands");

            (w.on_change)("".into());
            commit();
            assert_eq!(committed.borrow().last(), Some(&None), "emptying commits None");
            assert_eq!(w.error.get(), None, "empty is not an error");
        });
    }

    #[test]
    fn blur_normalizes_lenient_text() {
        with_test_world(|| {
            let value = signal(None);
            let committed = Rc::new(RefCell::new(Vec::new()));
            let w = typed_field_wiring(date_spec(value, committed));

            (w.on_change)("2026-3-7".into());
            commit();
            assert_eq!(w.text.peek(), "2026-3-7", "text stays as typed while focused");
            (w.on_focus_change)(false);
            commit();
            assert_eq!(w.text.peek(), "2026-03-07", "blur canonicalizes");
        });
    }

    #[test]
    fn external_write_rerenders_but_own_commit_does_not() {
        with_test_world(|| {
            let value = signal(None);
            let committed = Rc::new(RefCell::new(Vec::new()));
            let w = typed_field_wiring(date_spec(value, committed));

            // Own commit via lenient text: the host writes back the same
            // value; the typed text must survive (no cursor jump).
            (w.on_change)("2026-3-7".into());
            commit();
            assert_eq!(w.text.peek(), "2026-3-7");

            // Genuine external write → canonical re-render.
            value.set(CivilDate::new(2027, 1, 2));
            commit();
            assert_eq!(w.text.peek(), "2027-01-02");

            // External clear → empty text.
            value.set(None);
            commit();
            assert_eq!(w.text.peek(), "");
        });
    }

    #[test]
    fn masked_typing_inserts_delimiters_and_commits() {
        with_test_world(|| {
            let value = signal(None);
            let committed = Rc::new(RefCell::new(Vec::new()));
            let w = typed_field_wiring(masked_date_spec(value, committed.clone()));

            for ch in "20260803".chars() {
                key(&w, ch);
            }
            commit();
            assert_eq!(w.text.peek(), "2026-08-03", "delimiters insert themselves");
            assert_eq!(*committed.borrow(), vec![Some(CivilDate::new(2026, 8, 3).unwrap())]);
            assert_eq!(w.error.get(), None);
        });
    }

    #[test]
    fn masked_reject_forces_text_resync() {
        with_test_world(|| {
            let value = signal(None);
            let committed = Rc::new(RefCell::new(Vec::new()));
            let w = typed_field_wiring(masked_date_spec(value, committed));

            for ch in "2026-0".chars() {
                key(&w, ch);
            }
            commit();
            assert_eq!(w.text.peek(), "2026-0");

            // "00" is no month: the char is rejected — but the signal
            // must still NOTIFY so the backend rewrites the input (the
            // native field already shows the stray 0). Subscribe like a
            // backend does and count notifications.
            let seen = Rc::new(RefCell::new(0));
            let seen_in_effect = seen.clone();
            effect!({
                let _ = w.text.get();
                *seen_in_effect.borrow_mut() += 1;
            });
            commit();
            let before = *seen.borrow();
            key(&w, '0');
            commit();
            assert_eq!(w.text.peek(), "2026-0", "rejected char never lands in the text");
            assert!(
                *seen.borrow() > before,
                "reject must force a notification so the input resyncs"
            );
        });
    }

    #[test]
    fn regression_masked_prefix_typing_raises_no_error() {
        // The bug: the first digit of a masked date set the "Invalid
        // date" error, mounting the error line under the Field and
        // resizing it on every keystroke of normal typing.
        with_test_world(|| {
            let value = signal(None);
            let committed = Rc::new(RefCell::new(Vec::new()));
            let w = typed_field_wiring(masked_date_spec(value, committed));

            for ch in "2026-08-0".chars() {
                key(&w, ch);
                assert_eq!(
                    w.error.get(),
                    None,
                    "in-progress prefix {:?} must not error",
                    w.text.peek()
                );
            }

            // A COMPLETE but semantically impossible date still errors
            // mid-typing (Feb 31 is aligned with the format, so the
            // mask lets it through to the real parser).
            (w.on_change)("2026-02-31".into());
            commit();
            assert_eq!(w.error.get(), Some("Invalid date".into()));

            // Misaligned garbage errors as before.
            (w.on_change)("gibberish".into());
            commit();
            assert_eq!(w.error.get(), Some("Invalid date".into()));
        });
    }

    #[test]
    fn regression_blur_raises_error_on_abandoned_prefix() {
        with_test_world(|| {
            let value = signal(None);
            let committed = Rc::new(RefCell::new(Vec::new()));
            let w = typed_field_wiring(masked_date_spec(value, committed));

            for ch in "2026-0".chars() {
                key(&w, ch);
            }
            assert_eq!(w.error.get(), None, "prefix is forgiven while focused");

            (w.on_focus_change)(false);
            commit();
            assert_eq!(
                w.error.get(),
                Some("Invalid date".into()),
                "leaving a partial date surfaces the error"
            );

            // Coming back and typing clears it again.
            (w.on_focus_change)(true);
            key(&w, '8');
            assert_eq!(w.text.peek(), "2026-08-");
            assert_eq!(w.error.get(), None);
        });
    }

    #[test]
    fn masked_tab_completes_segment_in_place() {
        with_test_world(|| {
            let value = signal(None);
            let committed = Rc::new(RefCell::new(Vec::new()));
            let w = typed_field_wiring(masked_date_spec(value, committed));

            for ch in "20261".chars() {
                key(&w, ch);
            }
            assert_eq!(w.text.peek(), "2026-1", "year auto-advanced, month 1 waits");
            // Month "1" is ambiguous — Tab commits it and stays.
            assert_eq!(tab(&w), KeyOutcome::PreventDefault);
            commit();
            assert_eq!(w.text.peek(), "2026-01-");

            // Deletion back to a bare "0" month (deletions bypass the
            // mask): "0" can't commit — Tab is swallowed, text stays.
            (w.on_change)("2026-0".into());
            commit();
            assert_eq!(tab(&w), KeyOutcome::PreventDefault);
            commit();
            assert_eq!(w.text.peek(), "2026-0");

            // Empty field: Tab falls through to the normal focus move.
            (w.on_change)(String::new());
            commit();
            assert_eq!(tab(&w), KeyOutcome::Default);
        });
    }

    #[test]
    fn unmasked_wiring_exposes_no_key_handler() {
        with_test_world(|| {
            let value = signal(None);
            let committed = Rc::new(RefCell::new(Vec::new()));
            let w = typed_field_wiring(date_spec(value, committed));
            assert!(w.on_key_down.is_none());
        });
    }
}
