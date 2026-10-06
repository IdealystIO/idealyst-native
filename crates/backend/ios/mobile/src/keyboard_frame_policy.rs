//! Delivery of soft-keyboard frame changes to the backend, without ever
//! dropping one.
//!
//! Un-gated (compiles on any host) so the regression tests run from any
//! platform — same pattern as `layout_drain_policy` / `splice_policy`. The
//! ios-only half (the `UIKeyboardWillChangeFrameNotification` observer, the
//! `convertRect:` overlap math, the layout pass that drains the mailbox)
//! lives in `imp`. A tighter test is not reachable: the bug needs UIKit to
//! post the notification synchronously from inside `resignFirstResponder`,
//! which only happens with a real keyboard on a simulator or device.
//!
//! # Why this exists
//!
//! The backend keeps the keyboard's coverage of the host as STATE
//! (`keyboard_overlap`) and shrinks the layout viewport by it. That state is
//! only correct if every frame change reaches it — open AND close.
//!
//! The observer used to reach the backend through `with_backend`, which
//! takes the backend with `try_borrow_mut` and silently returns `None` when
//! it is already borrowed. The keyboard OPENS from a tap, outside any
//! borrow, so the open frame landed. It usually CLOSES because the backend
//! itself unmounts the focused text input during a flush: UIKit resigns
//! first responder and posts the will-change-frame notification
//! synchronously, inside that borrow. The close frame was dropped,
//! `keyboard_overlap` kept its "open" value, and every later layout ran in
//! a viewport short by the keyboard's height (CrewForge on iPad, landscape:
//! a ~481 pt blank band until restart). Closing with the keyboard's own
//! hide key worked, because that path has no backend borrow on the stack.
//!
//! # The fix
//!
//! Every frame goes into a latest-wins mailbox first. If the backend is
//! free, the mailbox is drained and applied right away; if it is borrowed,
//! a layout pass is scheduled (that path already retries until the borrow
//! is released — see `layout_drain_policy`) and the layout pass drains the
//! mailbox before it reads the viewport. Routing the immediate case through
//! the mailbox too is what keeps the ordering right: a newer frame applied
//! directly also empties the slot, so an older deferred frame can never be
//! replayed over it later.

#![cfg_attr(not(target_os = "ios"), allow(dead_code))]

use std::cell::{Cell, RefCell};

/// Latest-wins slot holding the most recent keyboard end frame that has not
/// been applied to the backend yet. Only the newest frame matters: the
/// keyboard's coverage is a function of its END frame alone, so an
/// intermediate frame superseded before it could be applied carries no
/// information.
pub(crate) struct KeyboardFrameMailbox<R: Copy> {
    slot: Cell<Option<R>>,
}

impl<R: Copy> KeyboardFrameMailbox<R> {
    pub(crate) const fn new() -> Self {
        Self { slot: Cell::new(None) }
    }

    /// Store `frame`, replacing any older unapplied one.
    pub(crate) fn post(&self, frame: R) {
        self.slot.set(Some(frame));
    }

    /// Take the pending frame, leaving the slot empty.
    pub(crate) fn take(&self) -> Option<R> {
        self.slot.take()
    }
}

/// What [`deliver`] did with a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// The backend was free; the frame was applied now.
    Applied,
    /// The backend was borrowed; the frame waits in the mailbox and a
    /// drain (layout pass) was scheduled to apply it once the borrow ends.
    Deferred,
    /// No backend is reachable through the global self-handle
    /// (runtime-server mode, where the host drives layout synchronously
    /// via `run_layout`, or before install / after teardown). The frame
    /// waits in the mailbox for the next layout pass; no drain is
    /// scheduled because the scheduled drain could not reach a backend
    /// either.
    NoBackend,
}

/// Deliver one keyboard end frame. Never drops it: the frame either gets
/// applied now or stays in `mailbox` until the next layout pass drains it.
///
/// `apply` is the backend's overlap update; `schedule_drain` is called only
/// on the deferred path and must arrange for the mailbox to be drained once
/// the current borrow is released (the iOS backend passes
/// `schedule_layout_pass`, whose drain retries while the backend is busy).
pub(crate) fn deliver<B, R: Copy>(
    mailbox: &KeyboardFrameMailbox<R>,
    frame: R,
    backend: Option<&RefCell<B>>,
    apply: impl FnOnce(&mut B, R),
    schedule_drain: impl FnOnce(),
) -> Delivery {
    mailbox.post(frame);
    let Some(cell) = backend else {
        return Delivery::NoBackend;
    };
    match cell.try_borrow_mut() {
        Ok(mut b) => {
            if let Some(latest) = mailbox.take() {
                apply(&mut b, latest);
            }
            Delivery::Applied
        }
        Err(_) => {
            schedule_drain();
            Delivery::Deferred
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in for `IosBackend`: just the state the bug corrupts. Frames
    /// are modelled as the overlap height they produce (the `convertRect:`
    /// intersection is UIKit-only and irrelevant to delivery).
    #[derive(Default)]
    struct FakeBackend {
        keyboard_overlap: f32,
    }

    const KEYBOARD_OPEN: f32 = 481.0;
    const KEYBOARD_CLOSED: f32 = 0.0;

    fn apply(b: &mut FakeBackend, overlap: f32) {
        b.keyboard_overlap = overlap;
    }

    /// What the layout pass does before reading the viewport.
    fn layout_pass(b: &mut FakeBackend, mailbox: &KeyboardFrameMailbox<f32>) {
        if let Some(f) = mailbox.take() {
            apply(b, f);
        }
    }

    /// The CrewForge report: keyboard opens from a tap (backend free), then
    /// closes because a flush unmounted the focused field — UIKit posts the
    /// close frame while the backend is mutably borrowed. The close must
    /// survive and be applied by the layout pass that follows, restoring the
    /// full viewport.
    #[test]
    fn regression_keyboard_close_during_backend_borrow_is_not_dropped() {
        let backend = RefCell::new(FakeBackend::default());
        let mailbox = KeyboardFrameMailbox::new();
        let scheduled = Cell::new(0);

        let open = deliver(&mailbox, KEYBOARD_OPEN, Some(&backend), apply, || {
            scheduled.set(scheduled.get() + 1)
        });
        assert_eq!(open, Delivery::Applied);
        assert_eq!(backend.borrow().keyboard_overlap, KEYBOARD_OPEN);
        assert_eq!(scheduled.get(), 0, "an applied frame needs no extra pass");

        {
            // The flush that removes the focused TextInput.
            let _flush = backend.borrow_mut();
            let close = deliver(&mailbox, KEYBOARD_CLOSED, Some(&backend), apply, || {
                scheduled.set(scheduled.get() + 1)
            });
            assert_eq!(close, Delivery::Deferred);
            assert_eq!(scheduled.get(), 1, "a deferred frame must schedule a drain");
        }

        // The scheduled layout pass, once the flush's borrow is gone.
        layout_pass(&mut backend.borrow_mut(), &mailbox);
        assert_eq!(
            backend.borrow().keyboard_overlap,
            KEYBOARD_CLOSED,
            "the close frame was dropped: the viewport stays short by the keyboard height"
        );
    }

    /// A frame deferred under a borrow must not be replayed over a NEWER
    /// frame that was applied directly in the meantime (keyboard closed
    /// during a flush, then reopened by a tap before the pass ran).
    #[test]
    fn newer_frame_applied_directly_supersedes_an_older_deferred_one() {
        let backend = RefCell::new(FakeBackend::default());
        let mailbox = KeyboardFrameMailbox::new();
        backend.borrow_mut().keyboard_overlap = KEYBOARD_OPEN;

        {
            let _flush = backend.borrow_mut();
            let d = deliver(&mailbox, KEYBOARD_CLOSED, Some(&backend), apply, || {});
            assert_eq!(d, Delivery::Deferred);
        }
        let d = deliver(&mailbox, KEYBOARD_OPEN, Some(&backend), apply, || {});
        assert_eq!(d, Delivery::Applied);

        layout_pass(&mut backend.borrow_mut(), &mailbox);
        assert_eq!(backend.borrow().keyboard_overlap, KEYBOARD_OPEN);
    }

    /// Several frames under one borrow (show + resize, e.g. the predictive
    /// bar toggling): only the last one is applied.
    #[test]
    fn frames_deferred_under_one_borrow_apply_latest_only() {
        let backend = RefCell::new(FakeBackend::default());
        let mailbox = KeyboardFrameMailbox::new();
        {
            let _flush = backend.borrow_mut();
            deliver(&mailbox, 300.0, Some(&backend), apply, || {});
            deliver(&mailbox, KEYBOARD_OPEN, Some(&backend), apply, || {});
        }
        layout_pass(&mut backend.borrow_mut(), &mailbox);
        assert_eq!(backend.borrow().keyboard_overlap, KEYBOARD_OPEN);
        assert_eq!(mailbox.take(), None);
    }

    /// No backend through the global handle (runtime-server mode): keep the
    /// frame for the host's synchronous `run_layout`, and do not schedule a
    /// drain that could only abandon.
    #[test]
    fn no_backend_keeps_frame_for_the_next_layout_pass() {
        let mailbox = KeyboardFrameMailbox::new();
        let scheduled = Cell::new(false);
        let d = deliver::<FakeBackend, f32>(&mailbox, KEYBOARD_OPEN, None, apply, || {
            scheduled.set(true)
        });
        assert_eq!(d, Delivery::NoBackend);
        assert!(!scheduled.get());

        let mut b = FakeBackend::default();
        layout_pass(&mut b, &mailbox);
        assert_eq!(b.keyboard_overlap, KEYBOARD_OPEN);
    }
}
