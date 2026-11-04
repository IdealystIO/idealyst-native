//! A live `activity_indicator` size on a real `NSProgressIndicator`.
//!
//! REGRESSION: `update_activity_indicator_size` was the trait default, so
//! `size_dyn` was ignored after creation: the spinner kept its first control size
//! and the layout its first measure.
//!
//! `harness = false`: AppKit builds windows only on the main thread.
//!
//! ```sh
//! cargo test -p backend-macos --test activity_indicator_size
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use backend_macos::MacosBackend;
    use objc2::msg_send_id;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;
    use runtime_vocabulary::caps::{ActivityIndicatorOps, IntrospectionOps, LifecycleOps, ViewOps};

    pub fn run() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
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

        let mut backend = MacosBackend::new(mtm);
        backend.set_host_root(content.clone());
        let a11y = AccessibilityProps::default();
        let mut root = backend.create_view(&a11y);
        let spinner = backend.create_activity_indicator(ActivityIndicatorSize::Small, None, &a11y);
        backend.insert(&mut root, spinner.clone());
        backend.finish(root);
        backend.run_layout_pass_now();
        let height = |b: &MacosBackend| b.frame(&spinner).expect("laid out").height;
        let small = height(&backend);
        assert!(small > 0.0 && small <= 24.0, "Small is the small control size: {small}");

        backend.update_activity_indicator_size(&spinner, ActivityIndicatorSize::Large);
        backend.run_layout_pass_now();
        let large = height(&backend);
        assert!(large >= 30.0, "the measure follows the control size: {large}");

        backend.update_activity_indicator_size(&spinner, ActivityIndicatorSize::Small);
        backend.run_layout_pass_now();
        assert_eq!(height(&backend), small);
        println!("activity_indicator_size: ok");
    }
}
