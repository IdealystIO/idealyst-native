//! Hardware-keyboard focus on a real UIKit window: Tab traversal among
//! text inputs, and an Escape for an open overlay when nothing is focused.
//!
//! REGRESSION (robot E2E `tab_moves_focus_between_fields` on iOS): the
//! robot's key dispatch had no focus traversal, so `press_key("Tab")` left
//! focus in the first field. `IntrospectionOps::move_focus` now walks the
//! window's text inputs (`imp::focus_traversal`) — the same stops a real
//! hardware Tab moves between on iPhone.
//!
//! REGRESSION (real keyboard): with no field focused and no app key
//! listener, no view held first responder, so UIKit gathered key commands
//! from no view of ours and an Escape left an open overlay open. A
//! dismissable portal now has the backend make its overlay responder first
//! responder when nothing holds it — on open, and when a field resigns —
//! without taking focus from a field. UIKit's routing of a real key press
//! to that responder's command needs a real key event, which a test
//! process can't make (verified by hand with `idb ui key 41` on a booted
//! simulator); the test checks who holds first responder, the command it
//! offers, and runs its action.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator
//! and run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test key_focus --no-run
//! xcrun simctl spawn <udid> target/aarch64-apple-ios-sim/debug/deps/key_focus-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::cell::Cell;
    use std::rc::Rc;

    use backend_ios::{IosBackend, IosNode};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSString};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement};
    use runtime_vocabulary::caps::{IntrospectionOps, PortalOps, TextInputOps, ViewOps};
    use runtime_vocabulary::escape_dismiss;

    #[link(name = "UIKit", kind = "framework")]
    extern "C" {
        static UIKeyInputEscape: &'static NSString;
    }

    fn spin_runloop() {
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            static kCFRunLoopDefaultMode: *const std::ffi::c_void;
            fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, ret: u8) -> i32;
        }
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, 0) };
    }

    /// A key window with a host root the backend mounts into.
    fn host(mtm: MainThreadMarker, backend: &mut IosBackend) -> (Retained<UIWindow>, Retained<UIView>) {
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 800.0 } };
        let window: Retained<UIWindow> = unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame] };
        let _: () = unsafe { msg_send![&window, makeKeyAndVisible] };
        let host: Retained<UIView> = unsafe { msg_send_id![mtm.alloc::<UIView>(), initWithFrame: frame] };
        unsafe { window.addSubview(&host) };
        backend.set_host_root(host.clone());
        (window, host)
    }

    /// Put `node` in `host` at row `row`.
    fn place(host: &UIView, node: &IosNode, row: f64) {
        let v: &UIView = node.as_view();
        let frame = CGRect { origin: CGPoint { x: 10.0, y: 10.0 + row * 50.0 }, size: CGSize { width: 200.0, height: 40.0 } };
        let _: () = unsafe { msg_send![v, setFrame: frame] };
        let _: () = unsafe { msg_send![host, addSubview: v] };
    }

    fn first_responder_in(view: &UIView) -> Option<Retained<UIView>> {
        unsafe {
            if msg_send![view, isFirstResponder] {
                return Retained::retain(view as *const UIView as *mut UIView);
            }
            let subviews: Retained<AnyObject> = msg_send_id![view, subviews];
            let count: usize = msg_send![&*subviews, count];
            (0..count).find_map(|i| {
                let child: *mut UIView = msg_send![&*subviews, objectAtIndex: i];
                child.as_ref().and_then(first_responder_in)
            })
        }
    }

    fn input(backend: &mut IosBackend) -> IosNode {
        backend.create_text_input("", None, Rc::new(|_| {}), None, None, false, &AccessibilityProps::default())
    }

    fn regression_tab_moves_between_the_window_text_inputs(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let (_window, host) = host(mtm, &mut backend);
        let a11y = AccessibilityProps::default();
        let first = input(&mut backend);
        let disabled = input(&mut backend);
        let hidden = input(&mut backend);
        let area = backend.create_text_area("", None, true, None, None, Rc::new(|_| {}), None, &a11y);
        let last = input(&mut backend);
        for (row, node) in [&first, &disabled, &hidden, &area, &last].into_iter().enumerate() {
            place(&host, node, row as f64);
        }
        let _: () = unsafe { msg_send![disabled.as_view(), setEnabled: false] };
        let _: () = unsafe { msg_send![hidden.as_view(), setHidden: true] };

        let focused = |node: &IosNode| backend.is_focused(node) == Some(true);
        let tab = |through: &IosNode, forward: bool| {
            let inject = backend.move_focus(through, forward).expect("iOS moves focus");
            inject().expect("traversal runs");
        };

        // Nothing focused: forward starts at the first stop.
        tab(&first, true);
        assert!(focused(&first), "Tab from nowhere focuses the first field");
        tab(&first, true);
        assert!(focused(&area), "Tab skips the disabled and the hidden field");
        tab(&area, true);
        assert!(focused(&last), "a text area is a stop too");
        tab(&last, true);
        assert!(focused(&first), "past the last stop Tab wraps to the first");
        tab(&first, false);
        assert!(focused(&last), "Shift+Tab wraps back to the last");
        tab(&last, false);
        assert!(focused(&area), "Shift+Tab moves back");
        let _: bool = unsafe { msg_send![area.as_view(), resignFirstResponder] };
        println!("key_focus: tab traversal ok");
    }

    fn escape(responder: &UIView) -> bool {
        let escape = unsafe { UIKeyInputEscape.to_string() };
        unsafe {
            let list: *mut AnyObject = msg_send![responder, keyCommands];
            if list.is_null() {
                return false;
            }
            let count: usize = msg_send![list, count];
            (0..count).any(|i| {
                let command: *mut AnyObject = msg_send![list, objectAtIndex: i];
                let input: *mut NSString = msg_send![command, input];
                let action: Option<objc2::runtime::Sel> = msg_send![command, action];
                (*input).to_string() == escape && action.is_some_and(|a| a.name() == "idealystEscapeDismiss:")
            })
        }
    }

    fn regression_escape_reaches_an_open_overlay_with_nothing_focused(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let (window, host) = host(mtm, &mut backend);
        let a11y = AccessibilityProps::default();
        let field = input(&mut backend);
        place(&host, &field, 0.0);
        spin_runloop();
        assert!(first_responder_in(&window).is_none(), "nothing focused to begin with");

        // Open a dismissable overlay, as the vocabulary's portal handler
        // does: register it with the Escape stack, then create it.
        let dismissed = Rc::new(Cell::new(0));
        let d = dismissed.clone();
        let on_dismiss: Rc<dyn Fn()> = Rc::new(move || d.set(d.get() + 1));
        let registration = escape_dismiss::register(on_dismiss.clone(), Rc::new(Cell::new(false)));
        let portal =
            backend.create_portal(PortalTarget::Viewport(ViewportPlacement::FullScreen), Some(on_dismiss), false, &a11y);
        spin_runloop();
        let responder = first_responder_in(&window).expect("a responder of the backend's takes first responder");
        let is_field: bool = unsafe { msg_send![&*responder, isKindOfClass: objc2::class!(UITextField)] };
        assert!(!is_field, "not a field: no keyboard shows");
        assert!(escape(&responder), "it offers Escape");
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = unsafe { msg_send![&*responder, idealystEscapeDismiss: nil] };
        assert_eq!(dismissed.get(), 1, "Escape closes the overlay");

        // A field the user focuses takes over, and when it resigns with
        // the overlay still open, the responder comes back.
        let became: bool = unsafe { msg_send![field.as_view(), becomeFirstResponder] };
        assert!(became);
        spin_runloop();
        assert!(backend.is_focused(&field) == Some(true), "the field keeps the focus it took");
        let _: bool = unsafe { msg_send![field.as_view(), resignFirstResponder] };
        spin_runloop();
        let back = first_responder_in(&window).expect("the responder reclaims first responder");
        assert!(escape(&back));

        // A field already focused when an overlay opens keeps its focus.
        let became: bool = unsafe { msg_send![field.as_view(), becomeFirstResponder] };
        assert!(became);
        let second =
            backend.create_portal(PortalTarget::Viewport(ViewportPlacement::FullScreen), Some(Rc::new(|| {})), false, &a11y);
        spin_runloop();
        assert!(backend.is_focused(&field) == Some(true), "opening an overlay takes no focus from a field");
        let _: bool = unsafe { msg_send![field.as_view(), resignFirstResponder] };

        // No dismissable overlay: nothing is claimed.
        drop(registration);
        let _: bool = unsafe { msg_send![&*back, resignFirstResponder] };
        let _: bool = unsafe { msg_send![field.as_view(), becomeFirstResponder] };
        let _: bool = unsafe { msg_send![field.as_view(), resignFirstResponder] };
        spin_runloop();
        assert!(first_responder_in(&window).is_none(), "without an overlay nothing takes first responder");
        let _ = (portal, second);
        println!("key_focus: escape responder ok");
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        regression_tab_moves_between_the_window_text_inputs(mtm);
        regression_escape_reaches_an_open_overlay_with_nothing_focused(mtm);
        println!("key_focus: 2 passed");
    }
}
