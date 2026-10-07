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

//!
//! # Animating with the keyboard
//!
//! The notification also carries the keyboard's animation duration and
//! curve. The layout pass that applies a changed frame runs inside a
//! `UIView animateWithDuration:delay:options:animations:` block built from
//! them ([`uiview_animation_options`]), so UIKit animates every frame
//! change on the compositor with the keyboard's exact timing — the
//! native-app pattern, with no per-frame work on our side. Running the
//! pass from inside the block needs the backend's `&mut` inside a block
//! closure; [`run_synchronously_within`] makes that sound (see its docs).

#![cfg_attr(not(target_os = "ios"), allow(dead_code))]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// One `UIKeyboardWillChangeFrameNotification`: the keyboard's end frame
/// plus the animation that moves it there. Generic over the rect type so
/// the delivery policy is testable off-device (`R = CGRect` on iOS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct KeyboardFrameChange<R> {
    /// `UIKeyboardFrameEndUserInfoKey`, window/screen base coordinates.
    pub frame: R,
    /// `UIKeyboardAnimationDurationUserInfoKey`, seconds. `0` when the
    /// frame changes without animation (hardware keyboard attach, the end
    /// of an interactive dismissal).
    pub duration_s: f64,
    /// `UIKeyboardAnimationCurveUserInfoKey` — a `UIViewAnimationCurve`
    /// raw value. The keyboard reports `7`, a private curve outside the
    /// public enum that UIKit nevertheless honours when shifted into the
    /// options bitfield.
    pub curve: isize,
}

/// `UIViewAnimationOptionAllowUserInteraction` — keep the UI touchable
/// while the keyboard animates (a tap on another field mid-animation).
const UIVIEW_OPTION_ALLOW_USER_INTERACTION: usize = 1 << 1;
/// `UIViewAnimationOptionBeginFromCurrentState` — a reversal mid-flight
/// (keyboard dismissed while still opening) retargets from the on-screen
/// position instead of snapping to the previous animation's end.
const UIVIEW_OPTION_BEGIN_FROM_CURRENT_STATE: usize = 1 << 2;
/// `UIViewAnimationOptionCurve*` occupy bits 16..20: the option value is
/// the `UIViewAnimationCurve` shifted left by 16.
const UIVIEW_OPTION_CURVE_SHIFT: u32 = 16;
/// Mask for the curve bits (4 bits wide), so an out-of-range raw curve
/// can never spill into unrelated option bits.
const UIVIEW_OPTION_CURVE_MASK: usize = 0xF;

/// The `UIViewAnimationOptions` that reproduce the keyboard's own
/// animation for `curve` (the notification's raw `UIViewAnimationCurve`).
pub(crate) fn uiview_animation_options(curve: isize) -> usize {
    let curve_bits = (curve.max(0) as usize & UIVIEW_OPTION_CURVE_MASK) << UIVIEW_OPTION_CURVE_SHIFT;
    curve_bits | UIVIEW_OPTION_BEGIN_FROM_CURRENT_STATE | UIVIEW_OPTION_ALLOW_USER_INTERACTION
}

/// The author-facing timing for a keyboard move: the notification's real
/// duration with the cubic-bezier approximation of UIKit's keyboard curve
/// (`runtime_shared::keyboard::IOS_KEYBOARD_EASING`).
pub(crate) fn keyboard_transition(duration_s: f64) -> runtime_shared::Transition {
    let ms = (duration_s.max(0.0) * 1000.0).round() as u32;
    runtime_shared::Transition::new(ms, runtime_shared::keyboard::IOS_KEYBOARD_EASING)
}

/// Run `body(target)` from inside a callback that `invoke` is expected to
/// call synchronously — UIKit's `animations:` block, which
/// `+[UIView animateWithDuration:…]` executes before it returns so it can
/// capture the property changes made inside it. Returns whether the body
/// ran inside the callback.
///
/// Why the indirection: the block is an Objective-C object UIKit may
/// retain, so its closure must be `'static` and cannot borrow `target`.
/// It holds a raw pointer instead, in a slot that is cleared the moment
/// `invoke` returns. A synchronous invocation finds the pointer and runs
/// the body; a (hypothetical) late invocation finds the slot empty and
/// does nothing — never a dangling `&mut`. If the callback did not run
/// synchronously, the body runs right here, unanimated, so the layout
/// still lands.
pub(crate) fn run_synchronously_within<T: 'static>(
    target: &mut T,
    body: fn(&mut T),
    invoke: impl FnOnce(Rc<dyn Fn()>),
) -> bool {
    let slot: Rc<Cell<*mut T>> = Rc::new(Cell::new(target as *mut T));
    let armed = slot.clone();
    invoke(Rc::new(move || {
        let p = armed.replace(std::ptr::null_mut());
        if !p.is_null() {
            // SAFETY: `p` came from `target`, which is exclusively borrowed
            // for the whole of `invoke` and untouched by this function until
            // `invoke` returns. The slot is emptied on first use and again
            // right after `invoke` returns, so `p` is only ever dereferenced
            // during that borrow, once.
            body(unsafe { &mut *p });
        }
    }));
    let ran = slot.replace(std::ptr::null_mut()).is_null();
    if !ran {
        body(target);
    }
    ran
}

/// Which `keyboard_avoiding_view`s one avoider pass moves. A keyboard move
/// runs an `Instant` pass first (outside any animation) and then, when the
/// notification carries an animation, an `Animated` pass inside the
/// keyboard's `UIView` animation block.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AvoiderPass {
    /// Outside any animation: the `animated = false` avoiders, or every
    /// avoider when `all` (the change carried no animation, or a post-layout
    /// reconcile).
    Instant { all: bool },
    /// Inside the keyboard's animation block: the `animated = true` ones.
    Animated,
}

impl AvoiderPass {
    pub(crate) fn includes(self, avoid: runtime_shared::KeyboardAvoid) -> bool {
        match self {
            AvoiderPass::Instant { all } => all || !avoid.animated,
            AvoiderPass::Animated => avoid.animated,
        }
    }
}

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

    /// An animated keyboard move splits avoiders between the two passes with
    /// no overlap and no gap; an unanimated move (or a reconcile) applies
    /// every avoider in the instant pass.
    #[test]
    fn avoider_passes_partition_by_the_animated_flag() {
        use runtime_shared::{KeyboardAvoid, KeyboardAvoidBehavior::*};
        let animated = KeyboardAvoid { behavior: Padding, animated: true };
        let instant = KeyboardAvoid { behavior: Translate, animated: false };
        assert!(!AvoiderPass::Instant { all: false }.includes(animated));
        assert!(AvoiderPass::Instant { all: false }.includes(instant));
        assert!(AvoiderPass::Animated.includes(animated));
        assert!(!AvoiderPass::Animated.includes(instant));
        assert!(AvoiderPass::Instant { all: true }.includes(animated));
        assert!(AvoiderPass::Instant { all: true }.includes(instant));
    }

    /// The options bitfield reproduces the keyboard's curve and keeps the
    /// animation interruptible and touch-transparent.
    #[test]
    fn keyboard_curve_maps_into_uiview_option_bits() {
        // UIKit's keyboard reports the private curve 7.
        assert_eq!(uiview_animation_options(7), (7 << 16) | (1 << 2) | (1 << 1));
        // Public curves map the same way (EaseInOut = 0, Linear = 3).
        assert_eq!(uiview_animation_options(0), (1 << 2) | (1 << 1));
        assert_eq!(uiview_animation_options(3) >> 16, 3);
        // Garbage never spills outside the 4 curve bits.
        assert_eq!(uiview_animation_options(0x1234) >> 16, 0x4);
        assert_eq!(uiview_animation_options(-1), (1 << 2) | (1 << 1));
    }

    #[test]
    fn keyboard_transition_carries_the_reported_duration() {
        let t = keyboard_transition(0.25);
        assert_eq!(t.duration_ms, 250);
        assert_eq!(t.easing, runtime_shared::keyboard::IOS_KEYBOARD_EASING);
        assert_eq!(keyboard_transition(-1.0).duration_ms, 0);
    }

    /// UIKit runs the `animations:` block synchronously: the body runs
    /// inside it, exactly once.
    #[test]
    fn synchronous_block_runs_the_body_inside_it() {
        let mut runs = 0u32;
        let inside = Cell::new(false);
        let ran = run_synchronously_within(
            &mut runs,
            |r| *r += 1,
            |block| {
                inside.set(true);
                block();
                block(); // a second call is inert
                inside.set(false);
            },
        );
        assert!(ran);
        assert_eq!(runs, 1);
    }

    /// If the block were NOT run synchronously, the body still runs (so the
    /// layout lands, unanimated), and a late invocation is inert instead of
    /// dereferencing a dangling backend pointer.
    #[test]
    fn deferred_block_falls_back_and_late_call_is_inert() {
        let mut runs = 0u32;
        let kept: RefCell<Option<Rc<dyn Fn()>>> = RefCell::new(None);
        let ran = run_synchronously_within(&mut runs, |r| *r += 1, |block| {
            *kept.borrow_mut() = Some(block);
        });
        assert!(!ran);
        assert_eq!(runs, 1, "fallback ran the body directly");
        (kept.borrow().as_ref().unwrap())();
        assert_eq!(runs, 1, "the late block call did nothing");
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
