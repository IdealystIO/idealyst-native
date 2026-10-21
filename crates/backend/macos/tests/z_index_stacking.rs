//! Sibling `z_index` against real AppKit: `apply_style` writes
//! `StyleRules::stacking_z()` to `layer.zPosition` (Core Animation paints
//! by it; an animated `AnimProp::ZIndex` overrides it), and `FlippedView.hitTest:` hands a click to the sibling painted
//! on top — AppKit's own walk goes by subview index and ignores
//! `zPosition`.
//!
//! Also covered: a reactive anchor (`display: contents`) lets its
//! children stack against the anchor's siblings; portals mounted in the
//! host content view (AppKit's class) take clicks in paint order; the
//! robot screenshot shows the screen's paint order; and `release_subtree`
//! drops discarded views' animation state.
//!
//! `harness = false` for the same reason as `focus_attach`: AppKit views and
//! windows are main-thread only, and libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test z_index_stacking
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
    use runtime_shared::{Length, Position, ScrollAxis, StyleRules};
    use runtime_shared::animation::AnimProp;
    use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement};
    use runtime_shared::primitives::virtualizer::VirtualLayout;
    use runtime_shared::{Color, VirtualizerCallbacks};
    use runtime_vocabulary::caps::{
        AnimationOps, IntrospectionOps, LifecycleOps, PortalOps, PresenceOps, ScrollOps, StyleOps,
        ViewOps, VirtualizerOps,
    };

    const WINDOW_W: f64 = 400.0;
    const WINDOW_H: f64 = 300.0;
    const BOX: f32 = 100.0;

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn z_of(view: &NSView) -> f64 {
        let layer: *mut AnyObject = unsafe { msg_send![view, layer] };
        assert!(!layer.is_null(), "a styled view is layer-backed");
        unsafe { msg_send![layer, zPosition] }
    }

    fn px(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
        Some(Length::Px(v).into())
    }

    /// An absolutely-positioned `BOX`-square at the parent's top-left —
    /// every such sibling overlaps the others completely.
    fn overlay_box(z_index: Option<i32>) -> Rc<StyleRules> {
        Rc::new(StyleRules {
            position: Some(Position::Absolute),
            top: px(0.0),
            left: px(0.0),
            width: px(BOX),
            height: px(BOX),
            z_index,
            ..Default::default()
        })
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

    /// Hit-test `pt` (in `space`'s own coordinates) the way AppKit does for
    /// a click: from the window's content view.
    fn hit(content: &NSView, space: &NSView, pt: CGPoint) -> Option<Retained<NSView>> {
        let in_content: CGPoint =
            unsafe { msg_send![content, convertPoint: pt, fromView: space] };
        let in_super: CGPoint = match unsafe { content.superview() } {
            Some(sup) => unsafe { msg_send![&sup, convertPoint: in_content, fromView: content] },
            None => in_content,
        };
        unsafe { msg_send_id![content, hitTest: in_super] }
    }

    fn is_within(view: &NSView, ancestor: &NSView) -> bool {
        unsafe { msg_send![view, isDescendantOf: ancestor] }
    }

    fn sized_root(backend: &mut MacosBackend, a11y: &AccessibilityProps) -> MacosNode {
        let root = backend.create_view(a11y);
        backend.apply_style(
            &root,
            &Rc::new(StyleRules {
                width: px(WINDOW_W as f32),
                height: px(WINDOW_H as f32),
                ..Default::default()
            }),
        );
        root
    }

    /// REGRESSION (z_index on Apple): two overlapping siblings where the
    /// FIRST carries `z_index: 2`. It must paint on top (`zPosition` 2 vs 0)
    /// AND receive the click — AppKit's index-ordered walk would give the
    /// click to the second sibling, which paints underneath. Dropping the
    /// `z_index` restores both.
    fn regression_raised_first_sibling_paints_and_hits_on_top(mtm: MainThreadMarker) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm);
        backend.set_host_root(content.clone());
        let a11y = AccessibilityProps::default();

        let mut root = sized_root(&mut backend, &a11y);
        let first = backend.create_view(&a11y);
        let second = backend.create_view(&a11y);
        backend.apply_style(&first, &overlay_box(Some(2)));
        backend.apply_style(&second, &overlay_box(None));
        let (root_v, first_v, second_v) = (view_of(&root), view_of(&first), view_of(&second));
        backend.insert(&mut root, first.clone());
        backend.insert(&mut root, second.clone());
        backend.finish(root);
        backend.run_layout_pass_now();

        assert_eq!(z_of(&first_v), 2.0, "z_index 2 paints through layer.zPosition");
        assert_eq!(z_of(&second_v), 0.0, "an unset z_index is 0");
        let centre = CGPoint { x: BOX as f64 / 2.0, y: BOX as f64 / 2.0 };
        let got = hit(&content, &root_v, centre);
        assert!(
            got.as_ref().is_some_and(|v| is_within(v, &first_v)),
            "the raised FIRST sibling paints on top, so it must receive the click; got {got:?}"
        );

        // Dropping the z_index restores tree order: the later sibling is on top.
        backend.apply_style(&first, &overlay_box(None));
        assert_eq!(z_of(&first_v), 0.0, "removing z_index writes 0 back");
        let got = hit(&content, &root_v, centre);
        assert!(
            got.as_ref().is_some_and(|v| is_within(v, &second_v)),
            "with equal z the later sibling is on top and receives the click; got {got:?}"
        );
        window.close();
    }

    /// REGRESSION (sticky clobbered z_index): a sticky child's registration
    /// against its scroll view used to write `zPosition = 1` itself. Realize
    /// applies the style BEFORE `insert`, so the registration retried from
    /// `insert` ran after `apply_style` and replaced an explicit
    /// `z_index: 3` with 1. The raise is `stacking_z()` now: 3 with the
    /// explicit value, `STICKY_Z` without it, 0 once it leaves Sticky.
    fn regression_sticky_registration_keeps_explicit_z_index(mtm: MainThreadMarker) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm);
        backend.set_host_root(content.clone());
        let a11y = AccessibilityProps::default();

        let mut root = sized_root(&mut backend, &a11y);
        let mut scroll = backend.create_scroll_view(ScrollAxis::Vertical, None, &a11y);
        backend.apply_style(
            &scroll,
            &Rc::new(StyleRules { height: px(200.0), ..Default::default() }),
        );
        let header = backend.create_view(&a11y);
        let sticky = |z_index| {
            Rc::new(StyleRules {
                position: Some(Position::Sticky),
                top: px(0.0),
                height: px(40.0),
                z_index,
                ..Default::default()
            })
        };
        backend.apply_style(&header, &sticky(Some(3)));
        let header_v = view_of(&header);
        backend.insert(&mut scroll, header.clone());
        backend.insert(&mut root, scroll);
        backend.finish(root);
        backend.run_layout_pass_now();

        assert_eq!(z_of(&header_v), 3.0, "the sticky registration must not overwrite z_index 3");

        backend.apply_style(&header, &sticky(None));
        assert_eq!(
            z_of(&header_v),
            runtime_shared::style::STICKY_Z as f64,
            "Sticky without z_index raises to STICKY_Z"
        );

        backend.apply_style(
            &header,
            &Rc::new(StyleRules { height: px(40.0), ..Default::default() }),
        );
        assert_eq!(z_of(&header_v), 0.0, "leaving Sticky drops the raise");
        window.close();
    }

    /// An animated `AnimProp::ZIndex` overrides the static `z_index`: a
    /// restyle (theme swap, state change) re-applies the static value
    /// underneath but must not cancel the animated `zPosition` — the same
    /// precedence web gets from the animated inline `z-index`.
    fn animated_z_index_survives_a_restyle(mtm: MainThreadMarker) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm);
        backend.set_host_root(content.clone());
        let a11y = AccessibilityProps::default();
        let mut root = sized_root(&mut backend, &a11y);
        let animated = backend.create_view(&a11y);
        backend.apply_style(&animated, &overlay_box(Some(1)));
        let animated_v = view_of(&animated);
        backend.insert(&mut root, animated.clone());
        backend.finish(root);
        backend.run_layout_pass_now();
        assert_eq!(z_of(&animated_v), 1.0);
        backend.set_animated_f32(&animated, AnimProp::ZIndex, 5.0);
        assert_eq!(z_of(&animated_v), 5.0, "the animation writes zPosition");
        backend.apply_style(&animated, &overlay_box(Some(1)));
        assert_eq!(z_of(&animated_v), 5.0, "a restyle must not cancel the animated z");
        window.close();
    }

    fn colored(z_index: Option<i32>, left: f32, width: f32, color: &str) -> Rc<StyleRules> {
        Rc::new(StyleRules {
            position: Some(Position::Absolute),
            top: px(0.0),
            left: px(left),
            width: px(width),
            height: px(BOX),
            z_index,
            background: Some(Color::from(color).into()),
            ..Default::default()
        })
    }

    /// The robot screenshot of the host root, sampled at `pts` (points from
    /// the host root's top-left corner) as `(r, g, b)` bytes.
    fn screenshot_rgb(backend: &MacosBackend, host: &NSView, pts: &[CGPoint]) -> Vec<(u8, u8, u8)> {
        let shot = std::rc::Rc::new(std::cell::RefCell::new(None));
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
                // `getPixel:atX:y:` counts rows from the top, as `pts` do.
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
    fn is_yellow(c: (u8, u8, u8)) -> bool { c.0 > 200 && c.1 > 200 && c.2 < 60 }

    /// REGRESSION (robot screenshot vs screen): `cacheDisplayInRect:`
    /// rasterizes through `renderInContext:`, which ignores `zPosition` — the
    /// shot showed the LATER sibling on top while the screen showed the
    /// raised first one.
    fn regression_screenshot_paints_raised_sibling_on_top(mtm: MainThreadMarker) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm);
        backend.set_host_root(content.clone());
        let a11y = AccessibilityProps::default();
        let mut root = sized_root(&mut backend, &a11y);
        let red = backend.create_view(&a11y);
        let blue = backend.create_view(&a11y);
        backend.apply_style(&red, &colored(Some(2), 0.0, BOX, "#ff0000"));
        backend.apply_style(&blue, &colored(None, 0.0, BOX, "#0000ff"));
        let root_v = view_of(&root);
        backend.insert(&mut root, red);
        backend.insert(&mut root, blue);
        backend.finish(root);
        backend.run_layout_pass_now();
        let before = unsafe { root_v.subviews() }.len();
        let c = CGPoint { x: BOX as f64 / 2.0, y: BOX as f64 / 2.0 };
        let px = screenshot_rgb(&backend, &content, &[c]);
        assert!(is_red(px[0]), "the screenshot must show the raised sibling on top: {px:?}");
        assert_eq!(unsafe { root_v.subviews() }.len(), before, "capture leaves the tree as it was");
        window.close();
    }

    /// REGRESSION (anchor stacking): a reactive anchor is `display: contents`
    /// on web, so `z_index` on its child stacks against the ANCHOR'S
    /// siblings. As a plain layer the anchor sorted its children only among
    /// themselves: `[anchor{left z 0, right z 2}, cover z 1]` drew `cover`
    /// over both and took both clicks. Now the right child (z 2) is above
    /// `cover` and the left one (z 0) below it — in paint and for clicks.
    fn regression_anchor_children_stack_against_anchor_siblings(mtm: MainThreadMarker) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm);
        backend.set_host_root(content.clone());
        let a11y = AccessibilityProps::default();
        let mut root = sized_root(&mut backend, &a11y);
        let mut anchor = backend.create_anchor();
        let low = backend.create_view(&a11y);
        let high = backend.create_view(&a11y);
        let cover = backend.create_view(&a11y);
        backend.apply_style(&low, &colored(None, 0.0, BOX, "#00ff00"));
        backend.apply_style(&high, &colored(Some(2), BOX, BOX, "#0000ff"));
        backend.apply_style(&cover, &colored(Some(1), 0.0, 2.0 * BOX, "#ffff00"));
        let (root_v, anchor_v, high_v, cover_v) =
            (view_of(&root), view_of(&anchor), view_of(&high), view_of(&cover));
        backend.insert(&mut anchor, low);
        backend.insert(&mut anchor, high);
        backend.insert(&mut root, anchor);
        backend.insert(&mut root, cover);
        backend.finish(root);
        backend.run_layout_pass_now();

        let left = CGPoint { x: BOX as f64 / 2.0, y: BOX as f64 / 2.0 };
        let right = CGPoint { x: 1.5 * BOX as f64, y: BOX as f64 / 2.0 };
        let got = hit(&content, &root_v, left);
        assert!(
            got.as_ref().is_some_and(|v| is_within(v, &cover_v)),
            "left: `cover` (z 1) is above the anchor's z-0 child; got {got:?}"
        );
        let got = hit(&content, &root_v, right);
        assert!(
            got.as_ref().is_some_and(|v| is_within(v, &high_v)),
            "right: the anchor's z-2 child is above `cover` (z 1); got {got:?}"
        );
        let px = screenshot_rgb(&backend, &content, &[left, right]);
        assert!(is_yellow(px[0]) && is_blue(px[1]), "paint: left yellow, right blue; got {px:?}");
        // AppKit layer-backs the anchor when its parent's layer tree syncs
        // (the capture above flushed it).
        let layer: *mut AnyObject = unsafe { msg_send![&anchor_v, layer] };
        assert!(!layer.is_null(), "the anchor is layer-backed under a layer-backed parent");
        let is_transform: bool =
            unsafe { msg_send![layer, isKindOfClass: objc2::class!(CATransformLayer)] };
        assert!(is_transform, "the anchor's layer must not flatten its children");
        window.close();
    }

    /// REGRESSION (portal stacking): portal containers mount in the host
    /// content view, AppKit's class, which hit-tests by index. A portal
    /// mounted FIRST with `z_index: 5` painted above a later portal yet the
    /// later one took the click. Dropping the z hands it back to mount order.
    fn regression_raised_portal_takes_the_click(mtm: MainThreadMarker) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm);
        backend.set_host_root(content.clone());
        let a11y = AccessibilityProps::default();
        let app_root = sized_root(&mut backend, &a11y);
        backend.finish(app_root);
        let full = || {
            Rc::new(StyleRules {
                width: px(WINDOW_W as f32),
                height: px(WINDOW_H as f32),
                ..Default::default()
            })
        };
        let mut first = backend.create_portal(PortalTarget::Viewport(ViewportPlacement::FullScreen), None, false, &a11y);
        let first_card = backend.create_view(&a11y);
        backend.apply_style(&first_card, &full());
        let first_card_v = view_of(&first_card);
        backend.insert(&mut first, first_card);
        backend.apply_style(&first, &Rc::new(StyleRules { z_index: Some(5), ..Default::default() }));
        let mut second = backend.create_portal(PortalTarget::Viewport(ViewportPlacement::FullScreen), None, false, &a11y);
        let second_card = backend.create_view(&a11y);
        backend.apply_style(&second_card, &full());
        let second_card_v = view_of(&second_card);
        backend.insert(&mut second, second_card);
        backend.apply_style(&second, &Rc::new(StyleRules::default()));
        backend.run_layout_pass_now();

        let centre = CGPoint { x: WINDOW_W / 2.0, y: WINDOW_H / 2.0 };
        let got = hit(&content, &content, centre);
        assert!(
            got.as_ref().is_some_and(|v| is_within(v, &first_card_v)),
            "the raised first portal paints on top, so it must take the click; got {got:?}"
        );
        backend.apply_style(&first, &Rc::new(StyleRules::default()));
        let got = hit(&content, &content, centre);
        assert!(
            got.as_ref().is_some_and(|v| is_within(v, &second_card_v)),
            "at equal z the later-mounted portal is on top; got {got:?}"
        );
        window.close();
    }

    /// REGRESSION (animation state leak): per-view animation state outlived
    /// discarded views — `release_subtree` was a no-op on macOS — so a view
    /// the allocator later put at the same address inherited a stale
    /// animated `ZIndex` (its static `z_index` then never applied) or
    /// translate.
    fn regression_release_subtree_drops_animation_state(mtm: MainThreadMarker) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm);
        backend.set_host_root(content.clone());
        let a11y = AccessibilityProps::default();
        let mut root = sized_root(&mut backend, &a11y);
        let mut row = backend.create_view(&a11y);
        let cell = backend.create_view(&a11y);
        backend.insert(&mut row, cell.clone());
        backend.insert(&mut root, row.clone());
        backend.finish(root);
        // The styled root carries state of its own (static transform slots).
        let base = backend.animated_state_count();
        backend.set_animated_f32(&row, AnimProp::ZIndex, 3.0);
        backend.set_animated_f32(&cell, AnimProp::TranslateX, 10.0);
        assert_eq!(backend.animated_state_count(), base + 2);
        backend.release_subtree(&row);
        assert_eq!(
            backend.animated_state_count(),
            base,
            "discarding the row drops its subtree's state, and only its subtree's"
        );
        window.close();
    }

    /// REGRESSION (presence stacking): the presence placeholder runs its own
    /// geometric hit search (it must descend through collapsed wrappers),
    /// which walked subviews by index — a `z_index`-raised child lost the
    /// click, and an anchor's children could not stack against the
    /// anchor's siblings. `[raised z 2, plain]` overlapping, then
    /// `[anchor{left z 0, right z 2}, cover z 1]`.
    fn regression_presence_hit_follows_paint_order(mtm: MainThreadMarker) {
        let mut backend = MacosBackend::new(mtm);
        let (window, content) = window(mtm);
        backend.set_host_root(content.clone());
        let a11y = AccessibilityProps::default();
        let mut root = sized_root(&mut backend, &a11y);
        let mut placeholder = backend.create_presence_placeholder(&a11y);
        let raised = backend.create_view(&a11y);
        let plain = backend.create_view(&a11y);
        backend.apply_style(&raised, &colored(Some(2), 0.0, BOX, "#ff0000"));
        backend.apply_style(&plain, &colored(None, 0.0, BOX, "#0000ff"));
        let mut anchor = backend.create_anchor();
        let low = backend.create_view(&a11y);
        let high = backend.create_view(&a11y);
        let cover = backend.create_view(&a11y);
        // A second band, below the first.
        let band = |z: Option<i32>, left: f32, width: f32| {
            Rc::new(StyleRules {
                position: Some(Position::Absolute),
                top: px(BOX),
                left: px(left),
                width: px(width),
                height: px(BOX),
                z_index: z,
                ..Default::default()
            })
        };
        backend.apply_style(&low, &band(None, 0.0, BOX));
        backend.apply_style(&high, &band(Some(2), BOX, BOX));
        backend.apply_style(&cover, &band(Some(1), 0.0, 2.0 * BOX));
        let (root_v, raised_v, high_v, cover_v) =
            (view_of(&root), view_of(&raised), view_of(&high), view_of(&cover));
        backend.insert(&mut placeholder, raised);
        backend.insert(&mut placeholder, plain);
        backend.insert(&mut anchor, low);
        backend.insert(&mut anchor, high);
        backend.insert(&mut placeholder, anchor);
        backend.insert(&mut placeholder, cover);
        backend.insert(&mut root, placeholder);
        backend.finish(root);
        backend.run_layout_pass_now();

        let pick = |x: f64, y: f64| hit(&content, &root_v, CGPoint { x, y });
        let got = pick(BOX as f64 / 2.0, BOX as f64 / 2.0);
        assert!(
            got.as_ref().is_some_and(|v| is_within(v, &raised_v)),
            "presence: the raised first child takes the click; got {got:?}"
        );
        let got = pick(BOX as f64 / 2.0, 1.5 * BOX as f64);
        assert!(
            got.as_ref().is_some_and(|v| is_within(v, &cover_v)),
            "presence: `cover` (z 1) is above the anchor's z-0 child; got {got:?}"
        );
        let got = pick(1.5 * BOX as f64, 1.5 * BOX as f64);
        assert!(
            got.as_ref().is_some_and(|v| is_within(v, &high_v)),
            "presence: the anchor's z-2 child is above `cover` (z 1); got {got:?}"
        );
        window.close();
    }

    const ROW: f32 = 50.0;

    fn row_style(z_index: Option<i32>) -> Rc<StyleRules> {
        Rc::new(StyleRules {
            width: px(WINDOW_W as f32),
            height: px(ROW),
            z_index,
            ..Default::default()
        })
    }

    /// REGRESSION (virtualized row stacking): on web a virtualized list's
    /// rows are plain siblings, so `z_index` on row 0 puts it over the row 1
    /// it overlaps. Here each row sits in its own collection-view item view
    /// and the item views are the siblings: the item root's `zPosition`
    /// only ordered it inside its own item view, so row 1 painted over
    /// row 0 and took the click. The item view now carries its item's z,
    /// the collection view hit-tests item views in that order, and a later
    /// restyle follows. The rows overlap by half a row (a negative
    /// main-axis gap).
    fn regression_virtualized_row_z_index_orders_rows(mtm: MainThreadMarker) {
        let backend = Rc::new(std::cell::RefCell::new(MacosBackend::new(mtm)));
        let (window, content) = window(mtm);
        backend.borrow_mut().set_host_root(content.clone());
        let a11y = AccessibilityProps::default();
        let rows: Rc<std::cell::RefCell<Vec<(MacosNode, Retained<NSView>)>>> = Rc::default();
        let mount = {
            let backend = backend.clone();
            let rows = rows.clone();
            Rc::new(move |i: usize| {
                let mut b = backend.borrow_mut();
                let root = b.create_view(&AccessibilityProps::default());
                b.apply_style(&root, &row_style(if i == 0 { Some(2) } else { None }));
                rows.borrow_mut().push((root.clone(), view_of(&root)));
                (root, i as u64)
            }) as Rc<dyn Fn(usize) -> (MacosNode, u64)>
        };
        let callbacks = VirtualizerCallbacks {
            item_count: Rc::new(|| 2),
            item_key: Rc::new(|i| i as u64),
            item_size: Rc::new(|_| ROW),
            measure_sizes: false,
            mount_item: mount,
            release_item: Rc::new(|_| {}),
            set_measured_size: Rc::new(|_, _| {}),
            on_scroll: None,
            item_changed: None,
        };
        let layout = VirtualLayout { main_spacing: -ROW / 2.0, ..VirtualLayout::default() };
        let list = {
            let mut b = backend.borrow_mut();
            let mut root = sized_root(&mut b, &a11y);
            let list = b.create_virtualizer(callbacks, 0.0, layout, &a11y);
            b.apply_style(
                &list,
                &Rc::new(StyleRules {
                    width: px(WINDOW_W as f32),
                    height: px(WINDOW_H as f32),
                    ..Default::default()
                }),
            );
            b.insert(&mut root, list.clone());
            b.finish(root);
            b.run_layout_pass_now();
            list
        };
        let scroll_v = view_of(&list);
        let cv: Retained<NSView> = unsafe { msg_send_id![&scroll_v, documentView] };
        let _: () = unsafe { msg_send![&scroll_v, layoutSubtreeIfNeeded] };
        let _: () = unsafe { msg_send![&cv, layoutSubtreeIfNeeded] };
        let _: () = unsafe { msg_send![objc2::class!(CATransaction), flush] };

        let rows = rows.borrow().clone();
        assert_eq!(rows.len(), 2, "both rows mounted");
        let item_of = |v: &NSView| -> Retained<NSView> { unsafe { msg_send_id![v, superview] } };
        let (row0, row1) = (&rows[0], &rows[1]);
        let f0: CGRect = unsafe { msg_send![&item_of(&row0.1), frame] };
        let f1: CGRect = unsafe { msg_send![&item_of(&row1.1), frame] };
        assert!(
            f1.origin.y < f0.origin.y + f0.size.height && f1.origin.y > f0.origin.y,
            "precondition: the rows overlap ({f0:?} / {f1:?})"
        );
        assert_eq!(z_of(&item_of(&row0.1)), 2.0, "row 0's item view carries its item's z");
        assert_eq!(z_of(&item_of(&row1.1)), 0.0, "row 1's item view stays at 0");
        // Where the rows overlap, in the collection view's (flipped) space.
        let pt = CGPoint { x: WINDOW_W / 2.0, y: f1.origin.y + 1.0 };
        let got = hit(&content, &cv, pt);
        assert!(
            got.as_ref().is_some_and(|v| is_within(v, &row0.1)),
            "row 0 (z 2) paints over row 1, so it takes the click; got {got:?}"
        );
        backend.borrow_mut().apply_style(&row0.0, &row_style(None));
        assert_eq!(z_of(&item_of(&row0.1)), 0.0, "the item view follows a restyle");
        let got = hit(&content, &cv, pt);
        assert!(
            got.as_ref().is_some_and(|v| is_within(v, &row1.1)),
            "with equal z the later row is on top; got {got:?}"
        );
        // A relayout re-applies the flow layout's attributes; the row keeps
        // its z through it.
        backend.borrow_mut().apply_style(&row0.0, &row_style(Some(2)));
        let cv_layout: Retained<AnyObject> = unsafe { msg_send_id![&cv, collectionViewLayout] };
        let _: () = unsafe { msg_send![&cv_layout, invalidateLayout] };
        let _: () = unsafe { msg_send![&cv, layoutSubtreeIfNeeded] };
        let _: () = unsafe { msg_send![objc2::class!(CATransaction), flush] };
        assert_eq!(z_of(&item_of(&row0.1)), 2.0, "a relayout keeps the row's z");
        backend.borrow_mut().set_animated_f32(&row1.0, AnimProp::ZIndex, 5.0);
        assert_eq!(z_of(&item_of(&row1.1)), 5.0, "the item view follows an animated z");
        window.close();
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        regression_raised_first_sibling_paints_and_hits_on_top(mtm);
        regression_sticky_registration_keeps_explicit_z_index(mtm);
        animated_z_index_survives_a_restyle(mtm);
        regression_screenshot_paints_raised_sibling_on_top(mtm);
        regression_anchor_children_stack_against_anchor_siblings(mtm);
        regression_raised_portal_takes_the_click(mtm);
        regression_release_subtree_drops_animation_state(mtm);
        regression_presence_hit_follows_paint_order(mtm);
        regression_virtualized_row_z_index_orders_rows(mtm);
        println!("z_index_stacking: 9 passed");
    }
}
