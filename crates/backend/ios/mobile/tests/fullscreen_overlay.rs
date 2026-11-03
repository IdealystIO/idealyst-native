//! A `FullScreen` portal's content is the whole window on UIKit, so an
//! `absolute; inset: 0` layer inside it draws over the whole screen.
//!
//! REGRESSION (CrewForge side drawer): `overlay(placement = FullScreen) {
//! scrim, panel }`, both `position: absolute; inset: 0`, drew nothing on
//! iOS — the content box had natural height, 0 with only absolute
//! children, and Taffy pins an absolute child to its parent, so the
//! panel, its parents and the content were all 402×0 while the robot still
//! reported the text visible. On web the panel pinned to the fixed portal
//! root and filled the window.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator
//! and run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test fullscreen_overlay --no-run
//! xcrun simctl spawn booted target/aarch64-apple-ios-sim/debug/deps/fullscreen_overlay-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::rc::Rc;

    use backend_ios::{IosBackend, IosNode};
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement};
    use runtime_shared::{Length, Position, StyleRules};
    use runtime_vocabulary::caps::{LifecycleOps, PortalOps, StyleOps, ViewOps};

    const WIN_W: f64 = 402.0;
    const WIN_H: f64 = 874.0;

    fn px(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
        Some(Length::Px(v).into())
    }

    fn view_of(node: &IosNode) -> Retained<UIView> {
        let v: &UIView = node.as_view();
        unsafe { Retained::retain(v as *const UIView as *mut UIView) }.expect("retain")
    }

    fn spin_runloop() {
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            static kCFRunLoopDefaultMode: *const std::ffi::c_void;
            fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, ret: u8) -> i32;
        }
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, 0) };
    }

    /// `view`'s frame in window coordinates.
    fn window_frame(view: &UIView, window: &UIView) -> (f64, f64, f64, f64) {
        let bounds: CGRect = unsafe { msg_send![view, bounds] };
        let r: CGRect = unsafe { msg_send![view, convertRect: bounds, toView: window] };
        (r.origin.x, r.origin.y, r.size.width, r.size.height)
    }

    fn regression_an_absolute_panel_in_a_fullscreen_overlay_fills_the_window(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: WIN_W, height: WIN_H } };
        let window: Retained<UIWindow> = unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame] };
        let _: () = unsafe { msg_send![&window, setHidden: false] };
        let host: Retained<UIView> = unsafe { msg_send_id![mtm.alloc::<UIView>(), initWithFrame: frame] };
        unsafe { window.addSubview(&host) };
        backend.set_host_root(host.clone());
        let a11y = AccessibilityProps::default();
        let app_root = backend.create_view(&a11y);
        backend.apply_style(
            &app_root,
            &Rc::new(StyleRules { width: px(WIN_W as f32), height: px(WIN_H as f32), ..Default::default() }),
        );
        backend.finish(app_root);

        let mut portal = backend.create_portal(PortalTarget::Viewport(ViewportPlacement::FullScreen), None, false, &a11y);
        // The overlay's content wrapper: no size of its own.
        let mut content = backend.create_view(&a11y);
        backend.apply_style(&content, &Rc::new(StyleRules::default()));
        let panel = backend.create_view(&a11y);
        backend.apply_style(
            &panel,
            &Rc::new(StyleRules {
                position: Some(Position::Absolute),
                top: px(0.0),
                right: px(0.0),
                bottom: px(0.0),
                left: px(0.0),
                ..Default::default()
            }),
        );
        let panel_v = view_of(&panel);
        let content_v = view_of(&content);
        backend.insert(&mut content, panel);
        backend.insert(&mut portal, content);
        backend.apply_style(&portal, &Rc::new(StyleRules::default()));
        spin_runloop();
        backend.run_layout();

        let window_v: &UIView = &window;
        assert_eq!(window_frame(&content_v, window_v), (0.0, 0.0, WIN_W, WIN_H), "the content is the window");
        assert_eq!(window_frame(&panel_v, window_v), (0.0, 0.0, WIN_W, WIN_H), "the inset-0 panel fills it");
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        regression_an_absolute_panel_in_a_fullscreen_overlay_fills_the_window(mtm);
        println!("fullscreen_overlay: 1 passed");
    }
}
