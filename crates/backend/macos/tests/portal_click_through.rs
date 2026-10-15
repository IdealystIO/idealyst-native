//! A click-through overlay (`overlay().click_through(true)`, idea-ui's
//! `ToastHost`) against real AppKit: the portal's `pointer_events: None`
//! style must neither move the content nor swallow clicks in its band.
//!
//! The vocabulary mounts a portal as `create_portal` → children →
//! `apply_style(portal, {pointer_events: None})`. That style lands on the
//! portal CONTAINER, which also carries the target's placement layout.
//!
//! `harness = false` for the same reason as `focus_attach`: AppKit views and
//! windows are main-thread only, and libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test portal_click_through
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
    use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement};
    use runtime_shared::{Length, PointerEvents, StyleRules};
    use runtime_vocabulary::caps::{LifecycleOps, PortalOps, StyleOps, ViewOps};

    const WINDOW_W: f64 = 400.0;
    const WINDOW_H: f64 = 300.0;
    /// Height of the portal's content strip — the toast host's empty band.
    const BAND_H: f32 = 56.0;

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn style(rules: StyleRules) -> Rc<StyleRules> {
        Rc::new(rules)
    }

    struct Mounted {
        window: Retained<NSWindow>,
        content: Retained<NSView>,
        container: Retained<NSView>,
        strip: Retained<NSView>,
        card: Retained<NSView>,
    }

    /// The ToastHost shape: a `Bottom` click-through portal whose content is
    /// a full-width strip `BAND_H` tall, holding one `Auto` card in its
    /// left half.
    fn mount(mtm: MainThreadMarker, backend: &mut MacosBackend) -> Mounted {
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
        backend.set_host_root(content.clone());

        let a11y = AccessibilityProps::default();
        // The app root. The layout pass computes nothing (portals
        // included) until `finish` stashes it.
        let app_root = backend.create_view(&a11y);
        backend.finish(app_root);
        let mut portal = backend.create_portal(
            PortalTarget::Viewport(ViewportPlacement::Bottom),
            None,
            false,
            &a11y,
        );
        let mut strip = backend.create_view(&a11y);
        let card = backend.create_view(&a11y);
        backend.apply_style(
            &strip,
            &style(StyleRules {
                height: Some(Length::Px(BAND_H).into()),
                ..Default::default()
            }),
        );
        backend.apply_style(
            &card,
            &style(StyleRules {
                width: Some(Length::Px((WINDOW_W / 2.0) as f32).into()),
                height: Some(Length::Px(BAND_H).into()),
                pointer_events: Some(PointerEvents::Auto),
                ..Default::default()
            }),
        );
        let (strip_v, card_v) = (view_of(&strip), view_of(&card));
        backend.insert(&mut strip, card);
        backend.insert(&mut portal, strip);
        // What `mount_portal` does after realizing the children for
        // `click_through(true)` (`builders/portal.rs`).
        backend.apply_style(
            &portal,
            &style(StyleRules { pointer_events: Some(PointerEvents::None), ..Default::default() }),
        );
        backend.run_layout_pass_now();

        Mounted { window, content, container: view_of(&portal), strip: strip_v, card: card_v }
    }

    /// Hit-test `pt` (in the portal container's own, flipped, coordinates)
    /// the way AppKit does for a click: from the window's content view.
    fn hit_from_content(m: &Mounted, pt: CGPoint) -> Option<Retained<NSView>> {
        let in_content: CGPoint =
            unsafe { msg_send![&m.content, convertPoint: pt, fromView: &*m.container] };
        let in_super: CGPoint = match unsafe { m.content.superview() } {
            Some(sup) => unsafe { msg_send![&sup, convertPoint: in_content, fromView: &*m.content] },
            None => in_content,
        };
        unsafe { msg_send_id![&m.content, hitTest: in_super] }
    }

    fn is_within(view: &NSView, ancestor: &NSView) -> bool {
        unsafe { msg_send![view, isDescendantOf: ancestor] }
    }

    /// REGRESSION (CrewForge kiosk, 2026-10-15): a click-through portal's
    /// style replaced the container's placement layout — `set_style` resets
    /// an unset `justify_content` to `FlexStart` — so a `Bottom` toast host
    /// drew its strip (and its dead band) at the TOP of the window.
    fn regression_click_through_style_keeps_bottom_placement(m: &Mounted) {
        let frame: CGRect = unsafe { msg_send![&m.strip, frame] };
        let expected_y = WINDOW_H - BAND_H as f64;
        assert!(
            (frame.origin.y - expected_y).abs() < 0.5 && (frame.size.height - BAND_H as f64).abs() < 0.5,
            "the Bottom placement must survive the portal's click-through style: strip frame {frame:?}, \
             expected y = {expected_y}"
        );
    }

    /// The strip's empty half passes the click through; the `Auto` card keeps it.
    fn click_through_band_passes_except_auto_card(m: &Mounted) {
        let band_mid_y = WINDOW_H - BAND_H as f64 / 2.0;
        let empty = hit_from_content(m, CGPoint { x: WINDOW_W * 0.75, y: band_mid_y });
        assert!(
            empty.as_ref().is_none_or(|v| !is_within(v, &m.container)),
            "a click on the strip's empty area must fall through the portal, got {empty:?}"
        );
        let on_card = hit_from_content(m, CGPoint { x: WINDOW_W * 0.25, y: band_mid_y });
        assert!(
            on_card.as_ref().is_some_and(|v| is_within(v, &m.card)),
            "a click on the `Auto` card must land on it, got {on_card:?}"
        );
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = MacosBackend::new(mtm);
        let m = mount(mtm, &mut backend);
        regression_click_through_style_keeps_bottom_placement(&m);
        click_through_band_passes_except_auto_card(&m);
        m.window.close();
        println!("portal_click_through: 2 passed");
    }
}
