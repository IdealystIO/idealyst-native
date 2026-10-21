//! A two-axis `scroll_view` (`ScrollAxis::Both`) against real AppKit: the
//! `NSScrollView` carries both scrollers, its documentView spans the
//! content on BOTH axes (so it can pan either way), and a `Position::Sticky`
//! corner (`top: 0` + `left: 0`) pins on both axes at once — the table
//! header's corner cell inside a scroller that owns both directions.
//!
//! `harness = false`: AppKit views and windows are main-thread only, and
//! libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test two_axis_scroll
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
    use objc2::{msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Length, Position, ScrollAxis, StyleRules};
    use runtime_vocabulary::caps::{LifecycleOps, ScrollOps, StyleOps, ViewOps};

    const WINDOW_W: f64 = 400.0;
    const WINDOW_H: f64 = 300.0;

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn px(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
        Some(Length::Px(v).into())
    }

    fn window(mtm: MainThreadMarker) -> (Retained<NSWindow>, Retained<NSView>) {
        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: WINDOW_W, height: WINDOW_H },
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
        (window, content)
    }

    fn two_axis_scroller_pans_both_ways_and_pins_a_corner(mtm: MainThreadMarker) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm);
        backend.set_host_root(content.clone());
        let a11y = AccessibilityProps::default();

        let mut root = backend.create_view(&a11y);
        backend.apply_style(
            &root,
            &Rc::new(StyleRules {
                width: px(WINDOW_W as f32),
                height: px(WINDOW_H as f32),
                ..Default::default()
            }),
        );
        let mut scroll = backend.create_scroll_view(ScrollAxis::Both, None, &a11y);
        backend.apply_style(
            &scroll,
            &Rc::new(StyleRules {
                align_items: Some(runtime_shared::AlignItems::FlexStart),
                ..Default::default()
            }),
        );
        // The corner: pinned to the top AND the left.
        let corner = backend.create_view(&a11y);
        backend.apply_style(
            &corner,
            &Rc::new(StyleRules {
                position: Some(Position::Sticky),
                top: px(0.0),
                left: px(0.0),
                width: px(60.0),
                height: px(30.0),
                ..Default::default()
            }),
        );
        // Content wider and taller than the 400×300 viewport.
        let body = backend.create_view(&a11y);
        backend.apply_style(
            &body,
            &Rc::new(StyleRules {
                width: px(1000.0),
                height: px(900.0),
                flex_shrink: Some(runtime_shared::Tokenized::Literal(0.0)),
                ..Default::default()
            }),
        );
        let scroll_v = view_of(&scroll);
        let corner_v = view_of(&corner);
        backend.insert(&mut scroll, corner.clone());
        backend.insert(&mut scroll, body);
        backend.insert(&mut root, scroll.clone());
        backend.finish(root);
        backend.run_layout_pass_now();

        let has_v: bool = unsafe { msg_send![&scroll_v, hasVerticalScroller] };
        let has_h: bool = unsafe { msg_send![&scroll_v, hasHorizontalScroller] };
        assert!(has_v && has_h, "a two-axis scroller carries both scrollers");
        let doc: Retained<NSView> = unsafe { msg_send_id![&scroll_v, documentView] };
        let doc_frame: CGRect = unsafe { msg_send![&doc, frame] };
        assert!(
            doc_frame.size.width >= 1000.0 && doc_frame.size.height >= 900.0,
            "the documentView spans the content on both axes, got {:?}",
            doc_frame.size
        );

        backend.make_scroll_view_handle(&scroll).scroll_to(120.0, 80.0);
        backend.run_layout_pass_now();
        assert_eq!(
            backend.node_scroll(&scroll),
            (120.0, 80.0),
            "the scroller moved on both axes"
        );
        let corner_frame: CGRect = unsafe { msg_send![&corner_v, frame] };
        assert!(
            (corner_frame.origin.x - 120.0).abs() < 0.5 && (corner_frame.origin.y - 80.0).abs() < 0.5,
            "the top+left sticky corner pins at the scroll offset on both axes, got {:?}",
            corner_frame.origin
        );
        window.close();
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        two_axis_scroller_pans_both_ways_and_pins_a_corner(mtm);
        println!("two_axis_scroll: 1 passed");
    }
}
