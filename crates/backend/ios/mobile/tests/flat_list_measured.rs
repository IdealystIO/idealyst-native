//! `flat_list` rows on UIKit: laid out at their cell's width, and spaced
//! by their rendered size when the list is `ItemSize::Measured`.
//!
//! REGRESSION (CrewForge `TableCards` on the iPhone 17 simulator):
//! - every card was laid out window-wide — 402pt at x = 16 in a 370pt
//!   cell — so its right 32pt (its actions menu) was off screen: rows
//!   were computed against the viewport, not their cell;
//! - cards estimated at 132pt and 200pt tall sat 142pt apart (estimate +
//!   gap), each overlapping the next by 58pt: Measured was treated as
//!   Known and the estimate was never replaced.
//!
//! Boots the way an app does — the libdispatch scheduler and the global
//! backend handle — because the measurement resizes the cells from a
//! microtask after the layout pass.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator
//! and run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test flat_list_measured --no-run
//! xcrun simctl spawn booted target/aarch64-apple-ios-sim/debug/deps/flat_list_measured-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use backend_ios::{IosBackend, IosNode};
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Length, StyleRules, VirtualLayout, VirtualizerCallbacks};
    use runtime_vocabulary::caps::{LifecycleOps, StyleOps, ViewOps, VirtualizerOps};

    const WIN_W: f64 = 402.0;
    const WIN_H: f64 = 874.0;
    const PAD: f32 = 16.0;
    const ESTIMATE: f32 = 132.0;
    const GAP: f32 = 10.0;
    /// Rendered heights: two taller than the estimate, one shorter.
    const HEIGHTS: [f32; 3] = [200.0, 200.0, 60.0];

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

    fn window_frame(view: &UIView, window: &UIView) -> (f64, f64, f64, f64) {
        let bounds: CGRect = unsafe { msg_send![view, bounds] };
        let r: CGRect = unsafe { msg_send![view, convertRect: bounds, toView: window] };
        (r.origin.x, r.origin.y, r.size.width, r.size.height)
    }

    struct Mounted {
        window: Retained<UIWindow>,
        list: Retained<UIView>,
        rows: Rc<RefCell<Vec<Retained<UIView>>>>,
        reported: Rc<RefCell<HashMap<u64, f32>>>,
        _backend: Rc<RefCell<IosBackend>>,
    }

    /// A window with a column padded `PAD` each side holding a three-row
    /// list; `measure` picks `Measured` (true) or `Known` (false).
    fn mount(mtm: MainThreadMarker, measure: bool) -> Mounted {
        let backend = Rc::new(RefCell::new(IosBackend::new(mtm)));
        backend_ios::install_global_self(Rc::downgrade(&backend));
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: WIN_W, height: WIN_H } };
        let window: Retained<UIWindow> = unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame] };
        let _: () = unsafe { msg_send![&window, setHidden: false] };
        let host: Retained<UIView> = unsafe { msg_send_id![mtm.alloc::<UIView>(), initWithFrame: frame] };
        unsafe { window.addSubview(&host) };

        // The vocabulary's store: a reported size wins over the estimate.
        let reported: Rc<RefCell<HashMap<u64, f32>>> = Rc::default();
        let rows: Rc<RefCell<Vec<Retained<UIView>>>> = Rc::default();
        let mount_item = {
            let backend = backend.clone();
            let rows = rows.clone();
            Rc::new(move |i: usize| {
                let mut b = backend.borrow_mut();
                let a11y = AccessibilityProps::default();
                // A card: padding 10 around a block of `HEIGHTS[i] - 20`.
                let mut root = b.create_view(&a11y);
                b.apply_style(&root, &Rc::new(StyleRules { padding_top: px(10.0), padding_bottom: px(10.0), ..Default::default() }));
                let body = b.create_view(&a11y);
                b.apply_style(&body, &Rc::new(StyleRules { height: px(HEIGHTS[i] - 20.0), ..Default::default() }));
                b.insert(&mut root, body);
                rows.borrow_mut().push(view_of(&root));
                (root, i as u64)
            }) as Rc<dyn Fn(usize) -> (IosNode, u64)>
        };
        let item_size = {
            let reported = reported.clone();
            Rc::new(move |i: usize| reported.borrow().get(&(i as u64)).copied().unwrap_or(ESTIMATE))
        };
        let set_measured_size = {
            let reported = reported.clone();
            Rc::new(move |scope: u64, size: f32| {
                reported.borrow_mut().insert(scope, size);
            })
        };
        let callbacks = VirtualizerCallbacks {
            item_count: Rc::new(|| HEIGHTS.len()),
            item_key: Rc::new(|i| i as u64),
            item_size,
            measure_sizes: measure,
            mount_item,
            release_item: Rc::new(|_| {}),
            set_measured_size,
            on_scroll: None,
            item_changed: None,
        };
        let a11y = AccessibilityProps::default();
        let list = {
            let mut b = backend.borrow_mut();
            b.set_host_root(host.clone());
            let mut column = b.create_view(&a11y);
            b.apply_style(
                &column,
                &Rc::new(StyleRules {
                    width: px(WIN_W as f32),
                    height: px(WIN_H as f32),
                    padding_left: px(PAD),
                    padding_right: px(PAD),
                    ..Default::default()
                }),
            );
            let layout = VirtualLayout { main_spacing: GAP, ..VirtualLayout::default() };
            let list = b.create_virtualizer(callbacks, 0.0, layout, &a11y);
            let list_v = view_of(&list);
            b.insert(&mut column, list);
            b.finish(column);
            b.run_layout();
            list_v
        };
        // Cells mount, the pass lays them out and measures them, the
        // microtask resizes the cells, the next pass lays them out again.
        for _ in 0..6 {
            spin_runloop();
            let _: () = unsafe { msg_send![&list, layoutIfNeeded] };
            backend.borrow_mut().run_layout();
        }
        Mounted { window, list, rows, reported, _backend: backend }
    }

    fn regression_measured_rows_take_their_rendered_height_and_their_cell_width(mtm: MainThreadMarker) {
        let m = mount(mtm, true);
        let window: &UIView = &m.window;
        let list_w = window_frame(&m.list, window).2;
        assert_eq!(list_w, WIN_W - 2.0 * PAD as f64, "precondition: the list is inside the padding");
        let rows = m.rows.borrow();
        assert!(rows.len() >= HEIGHTS.len(), "every row mounted: {}", rows.len());
        // The latest mount of each row (a resize can remount a cell).
        let latest: Vec<&Retained<UIView>> = rows.iter().rev().take(HEIGHTS.len()).rev().collect();
        let mut tops: Vec<(f64, f64, f64, f64)> = latest.iter().map(|r| window_frame(r, window)).collect();
        tops.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut y = tops[0].1;
        for (i, (x, top, w, h)) in tops.iter().enumerate() {
            assert_eq!((*x, *w), (PAD as f64, list_w), "row {i} spans its cell, not the window");
            assert_eq!(*h, HEIGHTS[i] as f64, "row {i} is its rendered height");
            assert_eq!(*top, y, "row {i} starts after the previous row and the gap");
            y = top + h + GAP as f64;
        }
        let reported = m.reported.borrow();
        for (i, h) in HEIGHTS.iter().enumerate() {
            assert_eq!(reported.get(&(i as u64)), Some(h), "row {i}'s size was reported");
        }
    }

    /// A `Known` list is never measured: its rows keep the authored size.
    fn a_known_list_is_never_measured(mtm: MainThreadMarker) {
        let m = mount(mtm, false);
        assert!(m.reported.borrow().is_empty(), "nothing reported for a Known list");
        let window: &UIView = &m.window;
        let row = m.rows.borrow()[0].clone();
        let (x, _, w, h) = window_frame(&row, window);
        assert_eq!((x, w, h), (PAD as f64, WIN_W - 2.0 * PAD as f64, ESTIMATE as f64), "the cell's box");
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_ios::install_scheduler();
        regression_measured_rows_take_their_rendered_height_and_their_cell_width(mtm);
        a_known_list_is_never_measured(mtm);
        println!("flat_list_measured: 2 passed");
    }
}
