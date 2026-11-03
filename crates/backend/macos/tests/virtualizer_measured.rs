//! `flat_list` with `ItemSize::Measured` against a real AppKit
//! `NSCollectionView`: rows are spaced by the size they render at, not by
//! the estimate, and a Known list never measures.
//!
//! The vocabulary side of the contract (overrides keyed by item key,
//! `item_size` returning them) is stood in for by the callbacks below:
//! `set_measured_size` records the size, `item_size` returns it.
//!
//! `harness = false` for the same reason as `focus_attach`: AppKit views and
//! windows are main-thread only, and libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test virtualizer_measured
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::primitives::virtualizer::VirtualLayout;
    use runtime_shared::{Length, StyleRules, VirtualizerCallbacks};
    use runtime_vocabulary::caps::{LifecycleOps, StyleOps, ViewOps, VirtualizerOps};

    #[allow(non_upper_case_globals)]
    extern "C" {
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
        fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, return_after: u8) -> i32;
    }

    const LIST_W: f64 = 400.0;
    /// Shorter than the content at both the estimated and the real sizes,
    /// so the collection view's frame IS its content height.
    const LIST_H: f64 = 300.0;
    /// The estimate every row starts at (idea-ui `TableCards` estimates
    /// 132pt cards that render at 200pt).
    const ESTIMATE: f32 = 132.0;
    const TALL: f32 = 200.0;
    const SHORT: f32 = 80.0;

    /// Run the main run loop (it drains the main dispatch queue the
    /// scheduler's microtasks are posted to) for `secs`.
    fn pump_for(secs: f64) {
        let end = Instant::now() + Duration::from_secs_f64(secs);
        while Instant::now() < end {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
        }
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

    fn sized(w: f32, h: f32) -> Rc<StyleRules> {
        Rc::new(StyleRules { width: px(w), height: px(h), ..Default::default() })
    }

    fn window(mtm: MainThreadMarker) -> (Retained<NSWindow>, Retained<NSView>) {
        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: LIST_W, height: LIST_H },
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

    /// What one list run left behind.
    struct Mounted {
        /// Owns the backend; the global handle the cells use is weak.
        backend: Rc<RefCell<MacosBackend>>,
        window: Retained<NSWindow>,
        cv: Retained<NSView>,
        /// Row index → the row's root view, latest mount.
        rows: Rc<RefCell<HashMap<usize, Retained<NSView>>>>,
        /// Row index → the `SHORT` child of a content-sized row.
        children: Rc<RefCell<HashMap<usize, MacosNode>>>,
        /// Every `set_measured_size(scope_id, size)` call, in order.
        reports: Rc<RefCell<Vec<(u64, f32)>>>,
    }

    impl Mounted {
        /// Settle: lay the collection view out, run the deferred
        /// relayout(s) a report queued, lay out again.
        fn settle(&self) {
            for _ in 0..4 {
                let _: () = unsafe { msg_send![&self.cv, layoutSubtreeIfNeeded] };
                pump_for(0.05);
            }
            let _: () = unsafe { msg_send![&self.cv, layoutSubtreeIfNeeded] };
        }

        /// Force a full re-query of item sizes and re-layout, as a window
        /// resize would.
        fn force_relayout(&self) {
            let layout: Retained<AnyObject> =
                unsafe { msg_send_id![&self.cv, collectionViewLayout] };
            let _: () = unsafe { msg_send![&layout, invalidateLayout] };
            self.settle();
        }

        /// The cell (item view) frame hosting row `idx`, in the collection
        /// view's flipped space.
        fn cell_frame(&self, idx: usize) -> CGRect {
            let row = self.rows.borrow().get(&idx).cloned().expect("row mounted");
            let cell: Retained<NSView> = unsafe { msg_send_id![&row, superview] };
            unsafe { msg_send![&cell, frame] }
        }

        /// Row `idx`'s slot as the flow layout places it — valid whether
        /// or not the row is in the visible range (mounted).
        fn slot_frame(&self, idx: usize) -> CGRect {
            let layout: Retained<AnyObject> =
                unsafe { msg_send_id![&self.cv, collectionViewLayout] };
            let path: Retained<AnyObject> = unsafe {
                msg_send_id![
                    objc2::class!(NSIndexPath),
                    indexPathForItem: idx as isize,
                    inSection: 0isize
                ]
            };
            let attrs: Retained<AnyObject> =
                unsafe { msg_send_id![&layout, layoutAttributesForItemAtIndexPath: &*path] };
            unsafe { msg_send![&attrs, frame] }
        }

        fn row_frame(&self, idx: usize) -> CGRect {
            let row = self.rows.borrow().get(&idx).cloned().expect("row mounted");
            unsafe { msg_send![&row, frame] }
        }

        fn child_frame(&self, idx: usize) -> CGRect {
            let child = self.children.borrow().get(&idx).cloned().expect("child mounted");
            unsafe { msg_send![&view_of(&child), frame] }
        }

        /// A reactive restyle of row `idx`'s child, as an effect would
        /// apply it after mount.
        fn resize_child(&self, idx: usize, h: f32) {
            let child = self.children.borrow().get(&idx).cloned().expect("child mounted");
            self.backend.borrow_mut().apply_style(&child, &sized(50.0, h));
        }

        fn content_main(&self) -> f64 {
            let layout: Retained<AnyObject> =
                unsafe { msg_send_id![&self.cv, collectionViewLayout] };
            let size: CGSize = unsafe { msg_send![&layout, collectionViewContentSize] };
            size.height
        }

        fn close(self) {
            self.window.close();
        }
    }

    /// Mount a vertical list of `heights.len()` rows estimated at
    /// `ESTIMATE`. `Some(h)` rows carry an explicit `height: h`; `None` rows
    /// are content-sized around one `SHORT`-tall child.
    fn mount_list(mtm: MainThreadMarker, heights: Vec<Option<f32>>, measure_sizes: bool) -> Mounted {
        let backend = Rc::new(RefCell::new(MacosBackend::new(mtm)));
        // The cells lay their rows out through the global backend handle.
        backend_macos::install_global_self(Rc::downgrade(&backend));
        let (window, content) = window(mtm);
        backend.borrow_mut().set_host_root(content.clone());

        let rows: Rc<RefCell<HashMap<usize, Retained<NSView>>>> = Rc::default();
        let children: Rc<RefCell<HashMap<usize, MacosNode>>> = Rc::default();
        let reports: Rc<RefCell<Vec<(u64, f32)>>> = Rc::default();
        // scope_id = idx + 1 (0 is never handed out by the vocabulary).
        let overrides: Rc<RefCell<HashMap<usize, f32>>> = Rc::default();
        let count = heights.len();
        let mount = {
            let backend = backend.clone();
            let rows = rows.clone();
            let children = children.clone();
            Rc::new(move |i: usize| {
                let mut b = backend.borrow_mut();
                let a11y = AccessibilityProps::default();
                let mut root = b.create_view(&a11y);
                match heights[i] {
                    Some(h) => b.apply_style(
                        &root,
                        &Rc::new(StyleRules { height: px(h), ..Default::default() }),
                    ),
                    None => {
                        let child = b.create_view(&a11y);
                        b.apply_style(&child, &sized(50.0, SHORT));
                        children.borrow_mut().insert(i, child.clone());
                        b.insert(&mut root, child);
                    }
                }
                rows.borrow_mut().insert(i, view_of(&root));
                (root, i as u64 + 1)
            }) as Rc<dyn Fn(usize) -> (MacosNode, u64)>
        };
        let item_size = {
            let overrides = overrides.clone();
            Rc::new(move |i: usize| overrides.borrow().get(&i).copied().unwrap_or(ESTIMATE))
        };
        let set_measured_size = {
            let overrides = overrides.clone();
            let reports = reports.clone();
            Rc::new(move |scope: u64, size: f32| {
                reports.borrow_mut().push((scope, size));
                overrides.borrow_mut().insert(scope as usize - 1, size);
            })
        };
        let callbacks = VirtualizerCallbacks {
            item_count: Rc::new(move || count),
            item_key: Rc::new(|i| i as u64),
            item_size,
            measure_sizes,
            mount_item: mount,
            release_item: Rc::new(|_| {}),
            set_measured_size,
            on_scroll: None,
            item_changed: None,
        };
        let a11y = AccessibilityProps::default();
        let list = {
            let mut b = backend.borrow_mut();
            let mut root = b.create_view(&a11y);
            b.apply_style(&root, &sized(LIST_W as f32, LIST_H as f32));
            let list = b.create_virtualizer(callbacks, 0.0, VirtualLayout::default(), &a11y);
            b.apply_style(&list, &sized(LIST_W as f32, LIST_H as f32));
            b.insert(&mut root, list.clone());
            b.finish(root);
            b.run_layout_pass_now();
            list
        };
        let scroll = view_of(&list);
        let _: () = unsafe { msg_send![&scroll, layoutSubtreeIfNeeded] };
        let cv: Retained<NSView> = unsafe { msg_send_id![&scroll, documentView] };
        let mounted = Mounted { backend, window, cv, rows, children, reports };
        mounted.settle();
        mounted
    }

    fn assert_close(got: f64, want: f32, what: &str) {
        assert!((got - want as f64).abs() < 0.5, "{what}: got {got}, want {want}");
    }

    /// REGRESSION: a Measured list's rows were spaced by the estimate
    /// forever — 200pt rows estimated at 132pt each overlapped the next
    /// row by 68pt, and the content was 3 × 132 tall. Rows now report the
    /// size they render at (taller AND shorter than the estimate), and
    /// every row's slot, every later row's offset and the content size
    /// follow. A further forced relayout reports nothing new: it converges.
    fn regression_measured_rows_are_spaced_by_their_rendered_size(mtm: MainThreadMarker) {
        let m = mount_list(mtm, vec![Some(TALL), None, Some(TALL)], true);

        let (c0, c1, c2) = (m.cell_frame(0), m.cell_frame(1), m.cell_frame(2));
        assert_close(c0.origin.y, 0.0, "row 0 offset");
        assert_close(c0.size.height, TALL, "row 0 slot (taller than the estimate)");
        assert_close(c1.origin.y, TALL, "row 1 starts where row 0 really ends");
        assert_close(c1.size.height, SHORT, "row 1 slot (shorter than the estimate)");
        assert_close(c2.origin.y, TALL + SHORT, "row 2 offset");
        assert_close(c2.size.height, TALL, "row 2 slot");
        assert_close(m.row_frame(1).size.height, SHORT, "row 1 lays out to its own size");
        assert_close(m.content_main(), TALL + SHORT + TALL, "content size");
        let cv_frame: CGRect = unsafe { msg_send![&m.cv, frame] };
        assert_close(cv_frame.size.height, TALL + SHORT + TALL, "collection view height");
        // Rows were measured at, and fill, the settled cross axis (the
        // clip width, narrowed by a vertical scroller when one shows).
        assert_close(c0.size.width, cv_frame.size.width as f32, "row 0 fills the cross axis");
        assert_close(m.row_frame(0).size.width, cv_frame.size.width as f32, "row 0 root width");

        let mut reported: Vec<(u64, f32)> = m.reports.borrow().clone();
        reported.sort_by_key(|r| r.0);
        assert_eq!(reported, vec![(1, TALL), (2, SHORT), (3, TALL)], "one report per row");

        m.force_relayout();
        assert_eq!(m.reports.borrow().len(), 3, "a relayout at the measured sizes reports nothing");
        m.close();
    }

    /// A Known list's sizes are final: rows taller than `item_size` are
    /// never measured, and `set_measured_size` is never called.
    fn known_list_never_measures(mtm: MainThreadMarker) {
        let m = mount_list(mtm, vec![Some(TALL), None, Some(TALL)], false);
        assert!(m.reports.borrow().is_empty(), "Known list called set_measured_size");
        assert_close(m.cell_frame(1).origin.y, ESTIMATE, "Known rows keep item_size spacing");
        assert_close(m.content_main(), ESTIMATE * 3.0, "Known content size");
        m.force_relayout();
        assert!(m.reports.borrow().is_empty(), "Known list called set_measured_size");
        m.close();
    }

    /// REGRESSION: a reactive change inside a mounted row (its child grows
    /// from `SHORT` to 150pt) re-measures the row: the row's slot, the next
    /// row's offset and the content size follow. Rows are detached Taffy
    /// roots, and the backend's layout pass never laid them out again, so
    /// the row kept its mount-time size (and its child its stale frame).
    fn regression_measured_row_follows_a_content_change_after_mount(mtm: MainThreadMarker) {
        const GROWN: f32 = 150.0;
        let m = mount_list(mtm, vec![Some(TALL), None, Some(TALL)], true);
        assert_close(m.cell_frame(1).size.height, SHORT, "precondition: row 1 measured");

        m.resize_child(1, GROWN);
        m.settle();
        assert_close(m.child_frame(1).size.height, GROWN, "row 1's child is laid out again");
        assert_close(m.cell_frame(1).size.height, GROWN, "row 1's cell grows with its content");
        assert_close(m.slot_frame(1).size.height, GROWN, "row 1's slot grows with its content");
        // Row 2 now starts past the viewport, so it may be recycled: read
        // its slot from the flow layout, not from a mounted cell.
        assert_close(m.slot_frame(2).origin.y, TALL + GROWN, "row 2 moves down");
        assert_close(m.content_main(), TALL + GROWN + TALL, "content size");

        m.resize_child(1, SHORT);
        m.settle();
        assert_close(m.cell_frame(1).size.height, SHORT, "row 1's cell shrinks back");
        assert_close(m.slot_frame(1).size.height, SHORT, "row 1's slot shrinks back");
        assert_close(m.slot_frame(2).origin.y, TALL + SHORT, "row 2 moves back up");
        assert_close(m.cell_frame(2).origin.y, TALL + SHORT, "row 2's cell is back in view");
        let reports = m.reports.borrow().len();
        m.force_relayout();
        assert_eq!(m.reports.borrow().len(), reports, "converged: nothing more to report");
        m.close();
    }

    /// REGRESSION: in a Known list, a reactive change inside a mounted row
    /// lays the row out again (its child takes its new frame) without
    /// measuring — the slot stays `item_size`.
    fn regression_known_row_relays_out_after_a_content_change(mtm: MainThreadMarker) {
        let m = mount_list(mtm, vec![Some(TALL), None, Some(TALL)], false);
        m.resize_child(1, 100.0);
        m.settle();
        assert_close(m.child_frame(1).size.height, 100.0, "row 1's child is laid out again");
        assert_close(m.cell_frame(1).size.height, ESTIMATE, "Known slot unchanged");
        assert!(m.reports.borrow().is_empty(), "Known list called set_measured_size");
        m.close();
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "must run on the main thread (harness = false)");
        // SAFETY: checked just above — this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let tests: [(&str, fn(MainThreadMarker)); 4] = [
            (
                "regression_measured_rows_are_spaced_by_their_rendered_size",
                regression_measured_rows_are_spaced_by_their_rendered_size,
            ),
            ("known_list_never_measures", known_list_never_measures),
            (
                "regression_measured_row_follows_a_content_change_after_mount",
                regression_measured_row_follows_a_content_change_after_mount,
            ),
            (
                "regression_known_row_relays_out_after_a_content_change",
                regression_known_row_relays_out_after_a_content_change,
            ),
        ];
        for (name, t) in tests {
            t(mtm);
            println!("test {name} ... ok");
        }
        println!("test result: ok. {} passed", tests.len());
    }
}
