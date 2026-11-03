//! The robot screenshot (`capture_screenshot`) against real AppKit: the PNG
//! is upright — row 0 is the TOP of the host root, as on screen — whatever
//! the host root is (an unflipped `NSView`, the framework's flipped host
//! from `create_host_root`, layer-backed or not) and inside scroll views.
//!
//! Every case mounts a red block at the top and a blue block at the bottom
//! of the host and samples the decoded PNG.
//!
//! `harness = false`: AppKit views and windows are main-thread only, and
//! libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test screenshot_orientation
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
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Color, Length, ScrollAxis, StyleRules};
    use runtime_vocabulary::caps::{IntrospectionOps, LifecycleOps, ScrollOps, StyleOps, TextOps, ViewOps};

    const WINDOW_W: f64 = 400.0;
    const WINDOW_H: f64 = 300.0;
    const BLOCK: f32 = 100.0;

    #[derive(Clone, Copy, Debug)]
    enum HostKind {
        /// A plain `NSView` content view (unflipped, not layer-backed).
        Plain,
        /// The framework's own host (`create_host_root`, a flipped view) —
        /// what `idealyst dev --macos` mounts into.
        Flipped,
        /// The framework's flipped host, layer-backed.
        FlippedLayerBacked,
    }

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn px(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
        Some(Length::Px(v).into())
    }

    fn window(
        mtm: MainThreadMarker,
        backend: &mut MacosBackend,
        kind: HostKind,
    ) -> (Retained<NSWindow>, Retained<NSView>) {
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
        let content: Retained<NSView> = match kind {
            HostKind::Plain => unsafe { msg_send_id![mtm.alloc::<NSView>(), initWithFrame: rect] },
            HostKind::Flipped | HostKind::FlippedLayerBacked => {
                backend.create_host_root().expect("macOS builds a host root")
            }
        };
        window.setContentView(Some(&content));
        if matches!(kind, HostKind::FlippedLayerBacked) {
            let _: () = unsafe { msg_send![&content, setWantsLayer: true] };
        }
        backend.set_host_root(content.clone());
        (window, content)
    }

    fn style(rules: StyleRules) -> Rc<StyleRules> {
        Rc::new(rules)
    }

    fn block(color: &str) -> Rc<StyleRules> {
        style(StyleRules {
            height: px(BLOCK),
            flex_shrink: Some(runtime_shared::Tokenized::Literal(0.0)),
            background: Some(Color::from(color).into()),
            ..Default::default()
        })
    }

    fn column(width: f32, height: f32, justify_between: bool) -> Rc<StyleRules> {
        style(StyleRules {
            width: px(width),
            height: px(height),
            flex_shrink: Some(runtime_shared::Tokenized::Literal(0.0)),
            justify_content: justify_between.then_some(runtime_shared::JustifyContent::SpaceBetween),
            background: Some(Color::from("#ffffff").into()),
            ..Default::default()
        })
    }

    /// Red on top, blue at the bottom, space between: a `w`×`h` column.
    fn red_over_blue(backend: &mut MacosBackend, a11y: &AccessibilityProps, w: f32, h: f32) -> MacosNode {
        let mut col = backend.create_view(a11y);
        backend.apply_style(&col, &column(w, h, true));
        let red = backend.create_view(a11y);
        backend.apply_style(&red, &block("#ff0000"));
        let blue = backend.create_view(a11y);
        backend.apply_style(&blue, &block("#0000ff"));
        backend.insert(&mut col, red);
        backend.insert(&mut col, blue);
        col
    }

    /// The decoded screenshot of the host root, sampled at `pts` (points
    /// from the host root's TOP-left corner) as `(r, g, b)` bytes.
    fn screenshot_rgb(backend: &MacosBackend, host: &NSView, pts: &[CGPoint]) -> Vec<(u8, u8, u8)> {
        let shot = Rc::new(std::cell::RefCell::new(None));
        let sink = shot.clone();
        backend.capture_screenshot(Box::new(move |r| *sink.borrow_mut() = Some(r)));
        let shot = shot.borrow_mut().take().expect("capture is synchronous").expect("capture");
        let bounds: CGRect = unsafe { msg_send![host, bounds] };
        let scale = shot.width as f64 / bounds.size.width;
        let data: Retained<AnyObject> = unsafe {
            msg_send_id![
                objc2::class!(NSData),
                dataWithBytes: shot.png.as_ptr() as *const std::ffi::c_void,
                length: shot.png.len()
            ]
        };
        let rep: Retained<AnyObject> =
            unsafe { msg_send_id![objc2::class!(NSBitmapImageRep), imageRepWithData: &*data] };
        pts.iter()
            .map(|p| {
                // `getPixel:atX:y:` counts rows from the top of the image.
                let mut px = [0usize; 4];
                let _: () = unsafe {
                    msg_send![&rep, getPixel: px.as_mut_ptr(), atX: (p.x * scale) as isize, y: (p.y * scale) as isize]
                };
                (px[0] as u8, px[1] as u8, px[2] as u8)
            })
            .collect()
    }

    fn is_red(c: (u8, u8, u8)) -> bool { c.0 > 200 && c.1 < 60 && c.2 < 60 }
    fn is_blue(c: (u8, u8, u8)) -> bool { c.0 < 60 && c.1 < 60 && c.2 > 200 }
    fn is_dark(c: (u8, u8, u8)) -> bool { (c.0 as u32 + c.1 as u32 + c.2 as u32) < 300 }

    const TOP: CGPoint = CGPoint { x: WINDOW_W / 2.0, y: BLOCK as f64 / 2.0 };
    const BOTTOM: CGPoint = CGPoint { x: WINDOW_W / 2.0, y: WINDOW_H - BLOCK as f64 / 2.0 };

    fn assert_upright(case: &str, kind: HostKind, px: &[(u8, u8, u8)]) {
        assert!(
            is_red(px[0]) && is_blue(px[1]),
            "{case} ({kind:?} host): the screenshot must show red at the top and blue at the \
             bottom, as on screen; got top {:?} bottom {:?}",
            px[0],
            px[1]
        );
    }

    /// Plain views straight under the host.
    fn plain_tree_is_upright(mtm: MainThreadMarker, kind: HostKind) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm, &mut backend, kind);
        let a11y = AccessibilityProps::default();
        let root = red_over_blue(&mut backend, &a11y, WINDOW_W as f32, WINDOW_H as f32);
        backend.finish(root);
        backend.run_layout_pass_now();
        let px = screenshot_rgb(&backend, &content, &[TOP, BOTTOM]);
        assert_upright("plain tree", kind, &px);
        window.close();
    }

    /// The same column inside a `scroll_view` on `axis` (a vertical page,
    /// a `scroll_x` table) — its documentView sits in a flipped clip view.
    fn scrolled_tree_is_upright(mtm: MainThreadMarker, kind: HostKind, axis: ScrollAxis) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm, &mut backend, kind);
        let a11y = AccessibilityProps::default();
        let mut root = backend.create_view(&a11y);
        backend.apply_style(&root, &column(WINDOW_W as f32, WINDOW_H as f32, false));
        let mut scroll = backend.create_scroll_view(axis, None, &a11y);
        backend.apply_style(
            &scroll,
            &style(StyleRules {
                width: px(WINDOW_W as f32),
                height: px(WINDOW_H as f32),
                align_items: Some(runtime_shared::AlignItems::FlexStart),
                ..Default::default()
            }),
        );
        // Wider than the viewport on a horizontal scroller, so it scrolls.
        let w = if matches!(axis, ScrollAxis::Vertical) { WINDOW_W } else { 2.0 * WINDOW_W };
        let col = red_over_blue(&mut backend, &a11y, w as f32, WINDOW_H as f32);
        backend.insert(&mut scroll, col);
        backend.insert(&mut root, scroll);
        backend.finish(root);
        backend.run_layout_pass_now();
        let px = screenshot_rgb(&backend, &content, &[TOP, BOTTOM]);
        assert_upright(&format!("{axis:?} scroll_view"), kind, &px);
        window.close();
    }

    /// Text renders upright: a label at the top-left shows ink in the top
    /// band only, and the glyphs aren't mirrored — "▀" (upper half block)
    /// inks the label's upper half, not its lower half.
    fn text_is_upright(mtm: MainThreadMarker, kind: HostKind) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm, &mut backend, kind);
        let a11y = AccessibilityProps::default();
        let mut root = backend.create_view(&a11y);
        backend.apply_style(&root, &column(WINDOW_W as f32, WINDOW_H as f32, false));
        let label = backend.create_text("▀▀▀▀▀▀▀▀", &a11y);
        backend.apply_style(
            &label,
            &style(StyleRules {
                font_size: px(48.0),
                color: Some(Color::from("#000000").into()),
                ..Default::default()
            }),
        );
        let label_v = view_of(&label);
        backend.insert(&mut root, label);
        backend.finish(root);
        backend.run_layout_pass_now();
        let f: CGRect = unsafe { msg_send![&label_v, frame] };
        assert!(f.size.height > 20.0 && f.origin.y < 1.0, "precondition: label at the top, got {f:?}");
        let x = f.size.width.min(WINDOW_W) / 2.0;
        let upper = CGPoint { x, y: f.size.height * 0.3 };
        let lower = CGPoint { x, y: f.size.height * 0.85 };
        let far = CGPoint { x, y: WINDOW_H - f.size.height * 0.3 };
        let px = screenshot_rgb(&backend, &content, &[upper, lower, far]);
        assert!(
            is_dark(px[0]) && !is_dark(px[1]) && !is_dark(px[2]),
            "text ({kind:?} host): the upper-half block must ink the label's upper half only, \
             at the top of the shot; got upper {:?} lower {:?} bottom-of-shot {:?}",
            px[0],
            px[1],
            px[2]
        );
        window.close();
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut n = 0;
        for kind in [HostKind::Plain, HostKind::Flipped, HostKind::FlippedLayerBacked] {
            plain_tree_is_upright(mtm, kind);
            scrolled_tree_is_upright(mtm, kind, ScrollAxis::Vertical);
            scrolled_tree_is_upright(mtm, kind, ScrollAxis::Horizontal);
            text_is_upright(mtm, kind);
            n += 4;
        }
        println!("screenshot_orientation: {n} passed");
    }
}
