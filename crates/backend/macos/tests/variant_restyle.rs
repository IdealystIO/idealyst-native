//! A reactive restyle on macOS drops what the previous style declared and
//! the new one does not, against real AppKit views.
//!
//! The case: a stylesheet variant `off { width: 0, height: 0, overflow:
//! Hidden }` / `on {}`. Flipping back to `on` re-applies rules that say
//! nothing about size or overflow; the view must size to its content and
//! stop clipping, as on web. `runtime-layout`'s
//! `tests/restyle_drops_absent_props.rs` pins the layout rule; this pins
//! that the backend's `apply_style` routes the author style through it.
//!
//! `harness = false`: AppKit views are main-thread only, and libtest runs
//! cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test variant_restyle
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::rc::Rc;

    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Length, Overflow, StyleRules};
    use runtime_vocabulary::caps::{LifecycleOps, StyleOps, ViewOps};

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn frame_of(view: &NSView) -> CGRect {
        unsafe { msg_send![view, frame] }
    }

    fn masks_to_bounds(view: &NSView) -> bool {
        let layer: Option<Retained<NSObject>> = unsafe { msg_send_id![view, layer] };
        layer.is_some_and(|l| unsafe { msg_send![&l, masksToBounds] })
    }

    /// REGRESSION (Timely's toolbar slot stayed 0×0 and clipped after its
    /// `shown` variant flipped `Off` → `On`, collapsing the button inside):
    /// the layout tree merged each restyle into the previous one, so the
    /// `off` arm's `width: 0` / `height: 0` outlived it.
    fn regression_leaving_a_collapsing_variant_restores_size_and_clip(mtm: MainThreadMarker) {
        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: 400.0, height: 300.0 },
        };
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
        let content: Retained<NSView> =
            unsafe { msg_send_id![mtm.alloc::<NSView>(), initWithFrame: rect] };
        window.setContentView(Some(&content));

        let backend = Rc::new(std::cell::RefCell::new(MacosBackend::new(mtm)));
        backend_macos::install_global_self(Rc::downgrade(&backend));
        let mut b = backend.borrow_mut();
        b.set_host_root(content);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        let mut slot = b.create_view(&a11y);
        let button = b.create_view(&a11y);
        b.apply_style(
            &button,
            &Rc::new(StyleRules {
                width: Some(Length::Px(40.0).into()),
                height: Some(Length::Px(20.0).into()),
                ..Default::default()
            }),
        );
        b.insert(&mut slot, button);
        let on = Rc::new(StyleRules::default());
        let off = Rc::new(StyleRules {
            width: Some(Length::Px(0.0).into()),
            height: Some(Length::Px(0.0).into()),
            overflow: Some(Overflow::Hidden),
            ..Default::default()
        });
        b.apply_style(&slot, &on);
        let slot_view = view_of(&slot);
        b.insert(&mut root, slot.clone());
        b.finish(root);
        b.run_layout_pass_now();
        let initial = frame_of(&slot_view);
        assert_eq!((initial.size.width, initial.size.height), (400.0, 20.0));

        b.apply_style(&slot, &off);
        b.run_layout_pass_now();
        let collapsed = frame_of(&slot_view);
        assert_eq!((collapsed.size.width, collapsed.size.height), (0.0, 0.0));
        assert!(masks_to_bounds(&slot_view), "`off` clips");

        b.apply_style(&slot, &on);
        b.run_layout_pass_now();
        let restored = frame_of(&slot_view);
        assert_eq!(
            (restored.size.width, restored.size.height),
            (initial.size.width, initial.size.height),
            "`on` declares no size: the slot sizes to its content again"
        );
        assert!(!masks_to_bounds(&slot_view), "`on` declares no overflow: no clip");
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        regression_leaving_a_collapsing_variant_restores_size_and_clip(mtm);
        println!("variant_restyle: 1 passed");
    }
}
