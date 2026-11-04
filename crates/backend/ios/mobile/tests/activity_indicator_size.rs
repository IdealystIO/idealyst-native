//! A live `activity_indicator` size on a real `UIActivityIndicatorView`.
//!
//! REGRESSION: `update_activity_indicator_size` was the trait default, so
//! `size_dyn` was ignored after creation: the spinner kept its first style and
//! the layout its first measure.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator and run
//! the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test activity_indicator_size --no-run
//! xcrun simctl spawn <udid> target/aarch64-apple-ios-sim/debug/deps/activity_indicator_size-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use backend_ios::IosBackend;
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;
    use runtime_vocabulary::caps::{ActivityIndicatorOps, IntrospectionOps, LifecycleOps, ViewOps};

    pub fn run() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = IosBackend::new(mtm);
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 800.0 } };
        let window: Retained<UIWindow> = unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame] };
        let _: () = unsafe { msg_send![&window, makeKeyAndVisible] };
        let host: Retained<UIView> = unsafe { msg_send_id![mtm.alloc::<UIView>(), initWithFrame: frame] };
        unsafe { window.addSubview(&host) };
        backend.set_host_root(host.clone());

        let a11y = AccessibilityProps::default();
        let mut root = backend.create_view(&a11y);
        let spinner = backend.create_activity_indicator(ActivityIndicatorSize::Small, None, &a11y);
        backend.insert(&mut root, spinner.clone());
        backend.finish(root);
        backend.run_layout();
        let height = |b: &IosBackend| b.frame(&spinner).expect("laid out").height;
        assert_eq!(height(&backend), 20.0, "Small is UIKit's medium style");

        backend.update_activity_indicator_size(&spinner, ActivityIndicatorSize::Large);
        backend.run_layout();
        assert_eq!(height(&backend), 37.0, "the measure follows the style");

        backend.update_activity_indicator_size(&spinner, ActivityIndicatorSize::Small);
        backend.run_layout();
        assert_eq!(height(&backend), 20.0);
        println!("activity_indicator_size: ok");
    }
}
