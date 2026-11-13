//! `PressActivation::Down` pressables on real UIKit
//! (`PressableOps::create_pressable_on_down`), plus the activation paths a
//! release-mode pressable shares with it.
//!
//! What a test process can't do is make UIKit run a real touch through the
//! gesture system: `UITouch` / `UIEvent` have no public constructors that
//! carry a phase and location, and a recognizer only changes state inside
//! UIKit's own event dispatch. So the press itself is covered in the
//! closest reachable pieces:
//!
//! - the native wiring: the view carries exactly one
//!   `UILongPressGestureRecognizer`, with `minimumPressDuration = 0`,
//!   `cancelsTouchesInView = NO`, a primary-only button mask, and the
//!   mount-time stale-touch delegate gate;
//! - its action (`invokeOnBegan:`) driven through every recognizer state a
//!   press goes through: it fires on `Began` only;
//! - the raw-touch path: `touchesBegan:` / `touchesEnded:` on the pressable
//!   fire nothing and do not reach an ancestor's `on_touch` (with a control
//!   proving the same calls DO reach the ancestor through a plain view).
//!
//! VoiceOver activation (`accessibilityActivate`) and the handle's `click`
//! are run for real, in both modes.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator
//! and run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test press_down --no-run
//! xcrun simctl spawn booted target/aarch64-apple-ios-sim/debug/deps/press_down-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use backend_ios::{IosBackend, IosNode};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{TouchEvent, TouchPhase, TouchResponse};
    use runtime_vocabulary::caps::{InputOps, PressableOps, ViewOps};

    // UIGestureRecognizerState.
    const POSSIBLE: isize = 0;
    const BEGAN: isize = 1;
    const CHANGED: isize = 2;
    const ENDED: isize = 3;
    const CANCELLED: isize = 4;

    pub struct FakeStateIvars {
        state: Cell<isize>,
    }

    declare_class!(
        /// A recognizer whose `state` the test sets — the state UIKit would
        /// have moved it to when it sends the action.
        struct FakeStateRecognizer;

        unsafe impl ClassType for FakeStateRecognizer {
            type Super = objc2_ui_kit::UIGestureRecognizer;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "IdealystPressDownTestFakeStateRecognizer";
        }

        impl DeclaredClass for FakeStateRecognizer {
            type Ivars = FakeStateIvars;
        }

        unsafe impl FakeStateRecognizer {
            #[method(state)]
            fn state(&self) -> isize {
                self.ivars().state.get()
            }
        }
    );

    fn fake_recognizer(mtm: MainThreadMarker) -> Retained<FakeStateRecognizer> {
        let this = mtm.alloc::<FakeStateRecognizer>();
        let this = this.set_ivars(FakeStateIvars { state: Cell::new(POSSIBLE) });
        unsafe { msg_send_id![super(this), init] }
    }

    fn frame(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } }
    }

    struct Fixture {
        _window: Retained<UIWindow>,
        node: IosNode,
        presses: Rc<Cell<u32>>,
        ancestor: Rc<RefCell<Vec<TouchPhase>>>,
    }

    /// A pressable (press-down or release) inside a `view` whose `on_touch`
    /// logs every phase it is offered, in a window.
    fn fixture(mtm: MainThreadMarker, backend: &mut IosBackend, down: bool) -> Fixture {
        let presses = Rc::new(Cell::new(0u32));
        let p = presses.clone();
        let on_press: Rc<dyn Fn()> = Rc::new(move || p.set(p.get() + 1));
        let a11y = AccessibilityProps::default();
        let node = if down {
            backend.create_pressable_on_down(on_press, &a11y)
        } else {
            backend.create_pressable(on_press, &a11y)
        };
        let row = backend.create_view(&a11y);
        let ancestor = Rc::new(RefCell::new(Vec::new()));
        let a = ancestor.clone();
        backend.install_touch_handler(
            &row,
            Rc::new(move |ev: &TouchEvent| {
                a.borrow_mut().push(ev.phase);
                TouchResponse::CONSUMED
            }),
        );
        let window: Retained<UIWindow> =
            unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame(0.0, 0.0, 400.0, 800.0)] };
        let _: () = unsafe { msg_send![&window, makeKeyAndVisible] };
        unsafe {
            let _: () = msg_send![row.as_view(), setFrame: frame(0.0, 0.0, 400.0, 200.0)];
            let _: () = msg_send![node.as_view(), setFrame: frame(10.0, 10.0, 100.0, 44.0)];
            let _: () = msg_send![row.as_view(), addSubview: node.as_view()];
            let _: () = msg_send![&window, addSubview: row.as_view()];
        }
        Fixture { _window: window, node, presses, ancestor }
    }

    fn recognizers(view: &UIView) -> Vec<Retained<AnyObject>> {
        let arr: *mut AnyObject = unsafe { msg_send![view, gestureRecognizers] };
        if arr.is_null() {
            return Vec::new();
        }
        let n: usize = unsafe { msg_send![arr, count] };
        (0..n).map(|i| unsafe { msg_send_id![arr, objectAtIndex: i] }).collect()
    }

    fn touch_set() -> Retained<AnyObject> {
        let touch: Retained<AnyObject> = unsafe { msg_send_id![class!(UITouch), new] };
        unsafe { msg_send_id![class!(NSSet), setWithObject: &*touch] }
    }

    fn raw_press(view: &UIView) {
        let touches = touch_set();
        let nil: *mut AnyObject = std::ptr::null_mut();
        unsafe {
            let _: () = msg_send![view, touchesBegan: &*touches, withEvent: nil];
            let _: () = msg_send![view, touchesMoved: &*touches, withEvent: nil];
            let _: () = msg_send![view, touchesEnded: &*touches, withEvent: nil];
        }
    }

    /// The native wiring: one zero-duration long-press recognizer that
    /// leaves nested content's touches alone, takes the primary button only,
    /// and refuses a touch that began before the control existed.
    fn wired_as_a_zero_duration_long_press(mtm: MainThreadMarker, backend: &mut IosBackend) {
        let fx = fixture(mtm, backend, true);
        let grs = recognizers(fx.node.as_view());
        assert_eq!(grs.len(), 1, "exactly one recognizer");
        let gr = &grs[0];
        let is_long: bool =
            unsafe { msg_send![&**gr, isKindOfClass: class!(UILongPressGestureRecognizer)] };
        assert!(is_long, "a UILongPressGestureRecognizer");
        let min: f64 = unsafe { msg_send![&**gr, minimumPressDuration] };
        assert_eq!(min, 0.0, "begins at touch-down");
        let cancels: bool = unsafe { msg_send![&**gr, cancelsTouchesInView] };
        assert!(!cancels, "nested interactive content keeps its touches");
        // UIEventButtonMaskPrimary = 1 << 0.
        let mask: isize = unsafe { msg_send![&**gr, buttonMaskRequired] };
        assert_eq!(mask, 1, "primary button only");

        // Stale-touch gate: a touch stamped at 0 (before this control was
        // created) is refused.
        let delegate: Retained<AnyObject> = unsafe { msg_send_id![&**gr, delegate] };
        let stale: Retained<AnyObject> = unsafe { msg_send_id![class!(UITouch), new] };
        let ok: Bool = unsafe {
            msg_send![&*delegate, gestureRecognizer: &**gr, shouldReceiveTouch: &*stale]
        };
        assert!(!ok.as_bool(), "a touch that began before mount can't press it");
        let begin: Bool = unsafe { msg_send![&*delegate, gestureRecognizerShouldBegin: &**gr] };
        assert!(begin.as_bool(), "an on-window control may begin");
        assert_eq!(fx.presses.get(), 0);
    }

    /// The recognizer's action across one press — Possible, Began, Changed
    /// (a move), Ended — fires once, at Began. A cancelled press after it
    /// fires nothing more.
    fn action_fires_on_began_only(mtm: MainThreadMarker, backend: &mut IosBackend) {
        let fx = fixture(mtm, backend, true);
        let grs = recognizers(fx.node.as_view());
        let target: Retained<AnyObject> = unsafe { msg_send_id![&*grs[0], delegate] };
        let fake = fake_recognizer(mtm);
        let send = |state: isize| {
            fake.ivars().state.set(state);
            let _: () = unsafe { msg_send![&*target, invokeOnBegan: &*fake] };
        };
        send(POSSIBLE);
        assert_eq!(fx.presses.get(), 0);
        send(BEGAN);
        assert_eq!(fx.presses.get(), 1, "fires at the down");
        send(CHANGED);
        send(CHANGED);
        send(ENDED);
        assert_eq!(fx.presses.get(), 1, "moves and the release fire nothing more");
        send(BEGAN);
        send(CANCELLED);
        assert_eq!(fx.presses.get(), 2, "a second press is a second action");
    }

    /// Raw touches on the pressable fire nothing (the recognizer is the
    /// press) and stop there: the ancestor's `on_touch` never sees them.
    /// Control: the same calls on a plain child view DO reach the ancestor,
    /// so the empty log is the pressable consuming, not a dead path.
    fn raw_touches_do_not_reach_ancestor(mtm: MainThreadMarker, backend: &mut IosBackend) {
        raw_touches_do_not_reach_ancestor_in(mtm, backend, true);
    }

    /// Regression: a release-mode pressable (Button, IconButton) let its raw
    /// touches bubble, so a tap on a table row's menu button opened the menu
    /// AND reached the row's `on_touch` tap, which navigated. It consumes
    /// them as the press-down pressable does.
    fn regression_release_pressable_touches_do_not_reach_ancestor(mtm: MainThreadMarker, backend: &mut IosBackend) {
        raw_touches_do_not_reach_ancestor_in(mtm, backend, false);
    }

    fn raw_touches_do_not_reach_ancestor_in(mtm: MainThreadMarker, backend: &mut IosBackend, on_down: bool) {
        let fx = fixture(mtm, backend, on_down);
        raw_press(fx.node.as_view());
        assert_eq!(fx.presses.get(), 0, "raw touches don't fire; the recognizer does");
        assert!(fx.ancestor.borrow().is_empty(), "nothing reaches the ancestor: {:?}", fx.ancestor.borrow());

        let plain = backend.create_view(&AccessibilityProps::default());
        let row: Retained<UIView> = unsafe { msg_send_id![fx.node.as_view(), superview] };
        unsafe {
            let _: () = msg_send![plain.as_view(), setFrame: frame(150.0, 10.0, 100.0, 44.0)];
            let _: () = msg_send![&*row, addSubview: plain.as_view()];
        }
        raw_press(plain.as_view());
        assert_eq!(
            *fx.ancestor.borrow(),
            [TouchPhase::Began, TouchPhase::Moved, TouchPhase::Ended],
            "control: an unconsumed touch bubbles to the ancestor"
        );
    }

    /// The press-down pressable owns its finger until it lifts, so it is
    /// the only thing that can say where the finger goes: it reports the
    /// rest of the press to `held_press`, which is what lets a menu it
    /// opened pick the row the finger lifts over. Before this, dragging a
    /// touch into the menu did nothing. The down itself is not reported
    /// (the action already fired), and a release-mode pressable reports
    /// nothing at all.
    fn regression_press_down_reports_the_held_fingers_travel(mtm: MainThreadMarker, backend: &mut IosBackend) {
        use runtime_shared::held_press::{add_listener, HeldPressPhase};
        let seen: Rc<RefCell<Vec<HeldPressPhase>>> = Rc::default();
        let s = seen.clone();
        let _listener = add_listener(move |ev| s.borrow_mut().push(ev.phase));

        let fx = fixture(mtm, backend, true);
        raw_press(fx.node.as_view());
        assert_eq!(*seen.borrow(), [HeldPressPhase::Moved, HeldPressPhase::Released]);

        seen.borrow_mut().clear();
        let release_mode = fixture(mtm, backend, false);
        raw_press(release_mode.node.as_view());
        assert!(seen.borrow().is_empty(), "a release-mode pressable reports nothing: {:?}", seen.borrow());
    }

    /// VoiceOver activation and the handle's `click` fire once, in both
    /// modes. `accessibilityActivate` answers YES so UIKit doesn't also
    /// synthesize a tap the recognizer would take as a second press.
    fn voiceover_and_handle_click_fire_once(mtm: MainThreadMarker, backend: &mut IosBackend) {
        for down in [true, false] {
            let fx = fixture(mtm, backend, down);
            let handled: bool = unsafe { msg_send![fx.node.as_view(), accessibilityActivate] };
            assert!(handled, "down={down}: accessibilityActivate is handled");
            assert_eq!(fx.presses.get(), 1, "down={down}: VoiceOver activation fires once");
            backend.make_pressable_handle(&fx.node).click();
            assert_eq!(fx.presses.get(), 2, "down={down}: handle click fires once");
        }
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_ios::install_scheduler();
        let mut backend = IosBackend::new(mtm);
        let tests: [(&str, fn(MainThreadMarker, &mut IosBackend)); 6] = [
            ("wired_as_a_zero_duration_long_press", wired_as_a_zero_duration_long_press),
            ("action_fires_on_began_only", action_fires_on_began_only),
            ("raw_touches_do_not_reach_ancestor", raw_touches_do_not_reach_ancestor),
            (
                "regression_release_pressable_touches_do_not_reach_ancestor",
                regression_release_pressable_touches_do_not_reach_ancestor,
            ),
            (
                "regression_press_down_reports_the_held_fingers_travel",
                regression_press_down_reports_the_held_fingers_travel,
            ),
            ("voiceover_and_handle_click_fire_once", voiceover_and_handle_click_fire_once),
        ];
        for (name, t) in tests {
            t(mtm, &mut backend);
            println!("test {name} ... ok");
        }
        println!("press_down: {} passed", tests.len());
    }
}
