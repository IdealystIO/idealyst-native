//! A horizontal `scroll_view` against real AppKit: its content row is as
//! wide as its tabs, and a click on a tab past the first screenful lands on
//! that tab once the strip is scrolled to it.
//!
//! REGRESSION (robot-e2e demo tab strip): the row was stretched to the
//! scroller's width, so its later tabs were laid out outside the row's
//! frame. `NSView.hitTest:` only descends into a subview whose frame holds
//! the point, so the click went to the scroller, not the tab.
//!
//! `harness = false`: AppKit views and windows are main-thread only, and
//! libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test hscroll_hit_test
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
    use runtime_shared::{FlexDirection, Length, ScrollAxis, StyleRules};
    use runtime_vocabulary::caps::{LifecycleOps, ScrollOps, StyleOps, ViewOps};

    const WINDOW_W: f64 = 400.0;
    const WINDOW_H: f64 = 300.0;
    const TAB_W: f32 = 80.0;
    const TABS: usize = 12;

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

    fn regression_hscroll_tab_past_the_viewport_takes_the_click(mtm: MainThreadMarker) {
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
        let mut scroll = backend.create_scroll_view(ScrollAxis::Horizontal, None, &a11y);
        // The row carries no width or alignment of its own: the default is
        // what's under test.
        let mut row = backend.create_view(&a11y);
        backend.apply_style(
            &row,
            &Rc::new(StyleRules {
                flex_direction: Some(FlexDirection::Row),
                ..Default::default()
            }),
        );
        let mut tabs = Vec::new();
        for _ in 0..TABS {
            let tab = backend.create_view(&a11y);
            backend.apply_style(
                &tab,
                &Rc::new(StyleRules {
                    width: px(TAB_W),
                    height: px(30.0),
                    ..Default::default()
                }),
            );
            tabs.push(view_of(&tab));
            backend.insert(&mut row, tab);
        }
        let row_v = view_of(&row);
        backend.insert(&mut scroll, row);
        backend.insert(&mut root, scroll.clone());
        backend.finish(root);
        backend.run_layout_pass_now();

        let row_frame: CGRect = unsafe { msg_send![&row_v, frame] };
        let content_w = TABS as f64 * TAB_W as f64;
        assert_eq!(row_frame.size.width, content_w, "the row is as wide as its tabs");

        // Scroll the last tab into the viewport, then click its centre.
        backend.make_scroll_view_handle(&scroll).scroll_to((content_w - WINDOW_W) as f32, 0.0);
        backend.run_layout_pass_now();
        let last = tabs.last().unwrap();
        let bounds: CGRect = unsafe { msg_send![&**last, bounds] };
        let centre = CGPoint {
            x: bounds.origin.x + bounds.size.width / 2.0,
            y: bounds.origin.y + bounds.size.height / 2.0,
        };
        let in_content: CGPoint =
            unsafe { msg_send![&**last, convertPoint: centre, toView: &*content] };
        assert!(
            in_content.x >= 0.0 && in_content.x < WINDOW_W,
            "the last tab is scrolled into the window, at x {}",
            in_content.x
        );
        // `hitTest:` takes the point in the receiver's SUPERVIEW space.
        let superview: *mut NSView = unsafe { msg_send![&*content, superview] };
        let point: CGPoint = if superview.is_null() {
            in_content
        } else {
            unsafe { msg_send![&*content, convertPoint: in_content, toView: superview] }
        };
        let hit: *mut NSView = unsafe { msg_send![&*content, hitTest: point] };
        assert!(
            std::ptr::eq(hit as *const NSView, &**last as *const NSView),
            "a click on the last tab lands on it, not on the scroller (hit {hit:?})"
        );
        window.close();
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        regression_hscroll_tab_past_the_viewport_takes_the_click(mtm);
        println!("hscroll_hit_test: 1 passed");
    }
}
