//! The robot's `visible` on macOS (`IntrospectionOps::is_shown`) against
//! real AppKit: a view in a window, not hidden by itself or an ancestor and
//! not faded out (alpha, or layer opacity — how the framework animates) is
//! shown; one that has left its window is not. Before, macOS had no
//! visibility read and the robot's "visible" meant "registered".
//!
//! `harness = false`: AppKit views and windows are main-thread only, and
//! libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test robot_visibility
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_vocabulary::caps::{IntrospectionOps, LifecycleOps, ViewOps};

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    pub fn run() {
        // harness = false: `main` runs on the main thread.
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
        let mut parent = backend.create_view(&a11y);
        let child = backend.create_view(&a11y);
        backend.insert(&mut parent, child.clone());
        backend.insert(&mut root, parent.clone());
        backend.finish(root);

        assert_eq!(backend.is_shown(&child), Some(true), "in the window, nothing hiding it");

        let p = view_of(&parent);
        let _: () = unsafe { msg_send![&p, setHidden: true] };
        assert_eq!(backend.is_shown(&child), Some(false), "an ancestor is hidden");
        let _: () = unsafe { msg_send![&p, setHidden: false] };

        let _: () = unsafe { msg_send![&p, setAlphaValue: 0.0f64] };
        assert_eq!(backend.is_shown(&child), Some(false), "an ancestor is fully transparent");
        let _: () = unsafe { msg_send![&p, setAlphaValue: 1.0f64] };

        let c = view_of(&child);
        let _: () = unsafe { msg_send![&c, setWantsLayer: true] };
        let layer: *mut objc2::runtime::AnyObject = unsafe { msg_send![&c, layer] };
        let _: () = unsafe { msg_send![layer, setOpacity: 0.0f32] };
        assert_eq!(backend.is_shown(&child), Some(false), "faded out on the layer");
        let _: () = unsafe { msg_send![layer, setOpacity: 1.0f32] };
        assert_eq!(backend.is_shown(&child), Some(true));

        let _: () = unsafe { msg_send![&p, removeFromSuperview] };
        assert_eq!(backend.is_shown(&child), Some(false), "no longer in a window");
        println!("robot_visibility: ok");
    }
}
