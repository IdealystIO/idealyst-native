//! `PressActivation::Down` pressables against real AppKit
//! (`PressableOps::create_pressable_on_down` → `make_press_down_handler` on a
//! `FlippedView`), alongside the activation paths a release-mode pressable
//! shares with it.
//!
//! Events are real `NSEvent`s sent to the view's own `mouseDown:` /
//! `mouseDragged:` / `mouseUp:` / `rightMouseDown:` / `keyDown:` overrides, so
//! the `FlippedView` dispatch (button classification, the accepted-press
//! routing, super-bubbling to the ancestor) is the code under test, not just
//! the handler closure. The ancestor is a `view` carrying an `on_touch`
//! handler: whatever reaches it is what would reach a row a menu button sits
//! on.
//!
//! `harness = false`: AppKit builds windows only on the main thread (see
//! `focus_attach.rs`).
//!
//! ```sh
//! cargo test -p backend-macos --test press_down
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSString};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{TouchEvent, TouchPhase, TouchResponse};
    use runtime_vocabulary::caps::{InputOps, PressableOps, ViewOps};

    // NSEventType values.
    const LEFT_DOWN: usize = 1;
    const LEFT_UP: usize = 2;
    const RIGHT_DOWN: usize = 3;
    const LEFT_DRAGGED: usize = 6;
    const KEY_DOWN: usize = 10;
    const FLAG_CONTROL: usize = 1 << 18;
    const KEY_SPACE: u16 = 49;
    const KEY_RETURN: u16 = 36;

    const SIZE: f64 = 100.0;

    struct Fixture {
        window: Retained<NSWindow>,
        node: MacosNode,
        view: Retained<NSView>,
        presses: Rc<Cell<u32>>,
        /// Phases the ancestor's `on_touch` handler saw.
        ancestor: Rc<RefCell<Vec<TouchPhase>>>,
    }

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn frame(v: &NSView) {
        let r = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: SIZE, height: SIZE } };
        let _: () = unsafe { msg_send![v, setFrame: r] };
    }

    /// A pressable (press-down or release) inside a `view` whose `on_touch`
    /// handler consumes everything it is offered, in a window.
    fn fixture(mtm: MainThreadMarker, backend: &mut MacosBackend, down: bool) -> Fixture {
        let presses = Rc::new(Cell::new(0u32));
        let p = presses.clone();
        let on_press: Rc<dyn Fn()> = Rc::new(move || p.set(p.get() + 1));
        let node = if down {
            backend.create_pressable_on_down(on_press, &AccessibilityProps::default())
        } else {
            backend.create_pressable(on_press, &AccessibilityProps::default())
        };
        let row = backend.create_view(&AccessibilityProps::default());
        let ancestor = Rc::new(RefCell::new(Vec::new()));
        let a = ancestor.clone();
        backend.install_touch_handler(
            &row,
            Rc::new(move |ev: &TouchEvent| {
                a.borrow_mut().push(ev.phase);
                TouchResponse::CONSUMED
            }),
        );
        let rect = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 300.0 } };
        let window: Retained<NSWindow> = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSWindow>(),
                rect,
                NSWindowStyleMask::Titled,
                NSBackingStoreType::NSBackingStoreBuffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        let content: Retained<NSView> = unsafe { msg_send_id![mtm.alloc::<NSView>(), initWithFrame: rect] };
        window.setContentView(Some(&content));
        let view = view_of(&node);
        let row_view = view_of(&row);
        frame(&view);
        frame(&row_view);
        unsafe {
            row_view.addSubview(&view);
            content.addSubview(&row_view);
        }
        Fixture { window, node, view, presses, ancestor }
    }

    fn window_number(w: &NSWindow) -> isize {
        unsafe { msg_send![w, windowNumber] }
    }

    /// A mouse event at window-base point `(x, y)` (inside the pressable,
    /// which sits at the content view's bottom-left corner in base coords).
    fn mouse(kind: usize, window: &NSWindow, x: f64, y: f64, flags: usize) -> Retained<AnyObject> {
        let ctx: *mut AnyObject = std::ptr::null_mut();
        unsafe {
            msg_send_id![
                class!(NSEvent),
                mouseEventWithType: kind,
                location: CGPoint { x, y },
                modifierFlags: flags,
                timestamp: 0.0f64,
                windowNumber: window_number(window),
                context: ctx,
                eventNumber: 0isize,
                clickCount: 1isize,
                pressure: 1.0f32,
            ]
        }
    }

    fn key(window: &NSWindow, key_code: u16) -> Retained<AnyObject> {
        let s = NSString::from_str(" ");
        let ctx: *mut AnyObject = std::ptr::null_mut();
        unsafe {
            msg_send_id![
                class!(NSEvent),
                keyEventWithType: KEY_DOWN,
                location: CGPoint { x: 0.0, y: 0.0 },
                modifierFlags: 0usize,
                timestamp: 0.0f64,
                windowNumber: window_number(window),
                context: ctx,
                characters: &*s,
                charactersIgnoringModifiers: &*s,
                isARepeat: false,
                keyCode: key_code,
            ]
        }
    }

    fn down(fx: &Fixture, x: f64, y: f64, flags: usize) {
        let e = mouse(LEFT_DOWN, &fx.window, x, y, flags);
        let _: () = unsafe { msg_send![&*fx.view, mouseDown: &*e] };
    }
    fn drag(fx: &Fixture, x: f64, y: f64) {
        let e = mouse(LEFT_DRAGGED, &fx.window, x, y, 0);
        let _: () = unsafe { msg_send![&*fx.view, mouseDragged: &*e] };
    }
    fn up(fx: &Fixture, x: f64, y: f64) {
        let e = mouse(LEFT_UP, &fx.window, x, y, 0);
        let _: () = unsafe { msg_send![&*fx.view, mouseUp: &*e] };
    }

    /// A primary press fires once AT the down; the drag (far past tap slop,
    /// off the control) and the release fire nothing more, and no phase of
    /// the press reaches the ancestor's `on_touch`.
    fn primary_press_fires_once_at_down(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        let fx = fixture(mtm, backend, true);
        down(&fx, 10.0, 10.0, 0);
        assert_eq!(fx.presses.get(), 1, "fires on the mouse-down, before any release");
        drag(&fx, 300.0, 250.0);
        up(&fx, 300.0, 250.0);
        assert_eq!(fx.presses.get(), 1, "the drag and the release fire nothing more");
        assert!(fx.ancestor.borrow().is_empty(), "no phase of the press reaches the ancestor: {:?}", fx.ancestor.borrow());

        // A second press is a second action.
        down(&fx, 10.0, 10.0, 0);
        up(&fx, 10.0, 10.0);
        assert_eq!(fx.presses.get(), 2);
        fx.window.close();
    }

    /// A right-click and a Ctrl-click (macOS's secondary click, delivered
    /// through `mouseDown:`) fire nothing, and are swallowed rather than
    /// passed to the ancestor (same as a release-mode pressable).
    fn secondary_press_does_not_fire(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        let fx = fixture(mtm, backend, true);
        let e = mouse(RIGHT_DOWN, &fx.window, 10.0, 10.0, 0);
        let _: () = unsafe { msg_send![&*fx.view, rightMouseDown: &*e] };
        down(&fx, 10.0, 10.0, FLAG_CONTROL);
        up(&fx, 10.0, 10.0);
        assert_eq!(fx.presses.get(), 0, "a secondary press never fires");
        assert!(fx.ancestor.borrow().is_empty(), "a secondary press is swallowed: {:?}", fx.ancestor.borrow());
        fx.window.close();
    }

    /// Space and Return on the (focusable) control each fire once; so do
    /// AXPress and the handle's `click`. Checked for both modes: these are
    /// single discrete activations with no down/up split.
    fn keyboard_ax_and_handle_click_fire_once(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        for down_mode in [true, false] {
            let fx = fixture(mtm, backend, down_mode);
            let focusable: bool = unsafe { msg_send![&*fx.view, acceptsFirstResponder] };
            assert!(focusable, "down={down_mode}: a pressable is a tab stop");

            let e = key(&fx.window, KEY_SPACE);
            let _: () = unsafe { msg_send![&*fx.view, keyDown: &*e] };
            assert_eq!(fx.presses.get(), 1, "down={down_mode}: Space fires once");
            let e = key(&fx.window, KEY_RETURN);
            let _: () = unsafe { msg_send![&*fx.view, keyDown: &*e] };
            assert_eq!(fx.presses.get(), 2, "down={down_mode}: Return fires once");

            let handled: bool = unsafe { msg_send![&*fx.view, accessibilityPerformPress] };
            assert!(handled, "down={down_mode}: AXPress is handled");
            assert_eq!(fx.presses.get(), 3, "down={down_mode}: AXPress fires once");

            backend.make_pressable_handle(&fx.node).click();
            assert_eq!(fx.presses.get(), 4, "down={down_mode}: handle click fires once");
            fx.window.close();
        }
    }

    /// Control: a release-mode pressable still fires on the release, not the
    /// down — the two modes are distinct handlers on the same view kind.
    fn release_mode_still_fires_on_release(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        let fx = fixture(mtm, backend, false);
        down(&fx, 10.0, 10.0, 0);
        assert_eq!(fx.presses.get(), 0, "release mode does not fire on the down");
        up(&fx, 12.0, 12.0);
        assert_eq!(fx.presses.get(), 1, "release mode fires on the release");
        fx.window.close();
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "press_down must run on the main thread (harness = false)");
        // SAFETY: checked just above — this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let mut backend = MacosBackend::new(mtm);
        let tests: [(&str, fn(MainThreadMarker, &mut MacosBackend)); 4] = [
            ("primary_press_fires_once_at_down", primary_press_fires_once_at_down),
            ("secondary_press_does_not_fire", secondary_press_does_not_fire),
            ("keyboard_ax_and_handle_click_fire_once", keyboard_ax_and_handle_click_fire_once),
            ("release_mode_still_fires_on_release", release_mode_still_fires_on_release),
        ];
        for (name, t) in tests {
            t(mtm, &mut backend);
            println!("test {name} ... ok");
        }
        println!("test result: ok. {} passed", tests.len());
    }
}
