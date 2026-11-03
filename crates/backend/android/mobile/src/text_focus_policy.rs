//! A text field's focus callbacks — `on_focus(bool)`
//! (`TextInputOps::set_text_input_focus_handler`, also the robot's
//! `focused`) and the cancelable `on_blur` (`create_text_input`) — behind the
//! one `View.OnFocusChangeListener` a field gets (`RustFocusListener.kt`).
//!
//! Un-gated so the dispatch rules are tested on the host; the JNI half
//! (installing the listener, the `nativeFocusChanged` trampoline) lives in
//! `imp/text_focus.rs`.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::primitives::text_input::{BlurHandler, BlurOutcome};

/// The callbacks one field's focus listener runs. Leaked per field (the
/// Kotlin listener holds its address); the slots fill as the framework
/// installs them, in either order.
#[derive(Default)]
pub(crate) struct TextFocusSlots {
    pub(crate) on_focus: RefCell<Option<Rc<dyn Fn(bool)>>>,
    pub(crate) on_blur: RefCell<Option<BlurHandler>>,
}

impl TextFocusSlots {
    /// One focus change of the field. `on_focus` hears every change; a loss
    /// then asks `on_blur`. Returns `true` when `on_blur` answered
    /// [`BlurOutcome::Keep`] — the listener then takes focus back if nothing
    /// else holds it.
    ///
    /// Each callback is cloned out before it runs: an author handler may
    /// re-enter the backend (focus another field, which fires this
    /// listener again).
    pub(crate) fn focus_changed(&self, has_focus: bool) -> bool {
        let on_focus = self.on_focus.borrow().clone();
        if let Some(f) = on_focus {
            f(has_focus);
        }
        if has_focus {
            return false;
        }
        let on_blur = self.on_blur.borrow().clone();
        on_blur.is_some_and(|f| f() == BlurOutcome::Keep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn recorder() -> (Rc<RefCell<Vec<bool>>>, Rc<dyn Fn(bool)>) {
        let seen: Rc<RefCell<Vec<bool>>> = Rc::default();
        let s = seen.clone();
        (seen, Rc::new(move |f| s.borrow_mut().push(f)))
    }

    /// REGRESSION: Android implemented no `set_text_input_focus_handler`,
    /// so a field's `on_focus` never ran and the robot's `focused` was
    /// always false. Each change reaches the notifier, in order.
    #[test]
    fn regression_android_text_input_on_focus_hears_every_change() {
        let slots = TextFocusSlots::default();
        let (seen, handler) = recorder();
        *slots.on_focus.borrow_mut() = Some(handler);
        assert!(!slots.focus_changed(true));
        assert!(!slots.focus_changed(false));
        assert_eq!(seen.borrow().as_slice(), [true, false]);
    }

    /// REGRESSION: Android dropped the author's `on_blur` at create, so it
    /// never ran. It runs on a loss only, and `Keep` asks for focus back.
    #[test]
    fn regression_android_text_input_on_blur_runs_on_loss_and_can_keep() {
        let slots = TextFocusSlots::default();
        let calls = Rc::new(Cell::new(0));
        let outcome = Rc::new(Cell::new(BlurOutcome::Allow));
        {
            let (calls, outcome) = (calls.clone(), outcome.clone());
            *slots.on_blur.borrow_mut() = Some(Rc::new(move || {
                calls.set(calls.get() + 1);
                outcome.get()
            }));
        }
        assert!(!slots.focus_changed(true), "a gain never asks on_blur");
        assert_eq!(calls.get(), 0);
        assert!(!slots.focus_changed(false), "Allow lets focus go");
        outcome.set(BlurOutcome::Keep);
        assert!(slots.focus_changed(false), "Keep asks for focus back");
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn a_field_without_callbacks_lets_focus_go() {
        let slots = TextFocusSlots::default();
        assert!(!slots.focus_changed(true));
        assert!(!slots.focus_changed(false));
    }

    /// An `on_focus` that re-enters the listener (focusing another field
    /// synchronously) must not hit a held `RefCell` borrow, and a handler
    /// replaced from inside a change is the one the next change runs.
    #[test]
    fn a_reentrant_handler_does_not_hold_the_slot() {
        let slots = Rc::new(TextFocusSlots::default());
        let (seen, replacement) = recorder();
        {
            let inner = slots.clone();
            *slots.on_focus.borrow_mut() = Some(Rc::new(move |_| {
                *inner.on_focus.borrow_mut() = Some(replacement.clone());
                inner.focus_changed(false);
            }));
        }
        slots.focus_changed(true);
        assert_eq!(seen.borrow().as_slice(), [false], "the re-entered change ran the new handler");
        slots.focus_changed(true);
        assert_eq!(seen.borrow().as_slice(), [false, true]);
    }

    // Escape in a field. The behavior is the platform `TextView`'s (API 37
    // blurs the field on Escape and reports it handled) against our
    // `EditText` subclass, which needs an emulator to observe — the demo's
    // `escape_closes_a_modal_while_its_field_has_focus` is the end-to-end
    // check, and failed on API 37 before the subclass. With no JVM test
    // harness in this repo, these pin the contract at the closest host-
    // reachable layer: the Kotlin the APK compiles, and the class the
    // field is created as.

    const EDIT_TEXT_KT: &str = include_str!("../runtime/kotlin/io/idealyst/runtime/RustEditText.kt");
    const TEXT_INPUT_RS: &str = include_str!("imp/primitives/text_input.rs");
    const MODAL_OVERLAY_KT: &str = include_str!("../runtime/kotlin/io/idealyst/runtime/RustModalOverlay.kt");

    /// Code only, whitespace and `//` / KDoc lines dropped, so a statement
    /// matches however it is wrapped.
    fn code(src: &str) -> String {
        src.lines()
            .map(|l| l.split_once("//").map_or(l, |(code, _)| code))
            .filter(|l| !l.trim_start().starts_with('*') && !l.trim_start().starts_with("/**"))
            .flat_map(str::split_whitespace)
            .collect()
    }

    /// Regression: a field whose `on_key_down` claimed Escape (on the key's
    /// DOWN) still saw its modal close on the UP, because the overlay
    /// dismissed on any unclaimed up. No JVM harness runs here, so this pins
    /// the Kotlin; the robot-e2e test
    /// `a_field_that_claims_escape_keeps_its_dialog_open` shows it on an
    /// emulator.
    #[test]
    fn regression_a_claimed_escape_down_keeps_the_modal_open() {
        let kt = code(MODAL_OVERLAY_KT);
        assert!(kt.contains("privatevardownClaimedBelow=false"), "{kt}");
        assert!(kt.contains("downClaimedBelow=claimed"), "the down records whether a child claimed it: {kt}");
        assert!(kt.contains("if(claimedDown)returntrue"), "a claimed down's up does not dismiss: {kt}");
    }

    /// From API 37 a focused field took Escape to blur itself, so a modal
    /// whose field held focus never saw it and stayed open (the robot-e2e
    /// demo on `showcase_api37`). The field is a `RustEditText`, which
    /// declines an unmodified Escape in both key phases before `TextView`
    /// sees it.
    #[test]
    fn regression_a_field_passes_escape_on_to_its_modal() {
        let kt = code(EDIT_TEXT_KT);
        assert!(kt.contains("classRustEditText(context:Context):EditText(context)"), "{kt}");
        for phase in ["onKeyDown", "onKeyUp"] {
            assert!(
                kt.contains(&format!("overridefun{phase}(keyCode:Int,event:KeyEvent):Boolean=if(passesOn(keyCode,event))falseelsesuper.{phase}(keyCode,event)")),
                "{phase} must decline Escape: {kt}"
            );
        }
        assert!(kt.contains("keyCode==KeyEvent.KEYCODE_ESCAPE&&event.hasNoModifiers()"), "{kt}");
        let rs = code(TEXT_INPUT_RS);
        assert!(rs.contains("find_class(\"io/idealyst/runtime/RustEditText\")"), "text_input creates a RustEditText");
        assert!(!rs.contains("find_class(\"android/widget/EditText\")"), "no plain EditText field left");
    }
}
