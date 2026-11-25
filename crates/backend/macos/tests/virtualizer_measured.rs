//! `flat_list` with `ItemSize::Measured` against a real AppKit
//! `NSCollectionView`: rows are spaced by the size they render at, not by
//! the estimate, and a Known list never measures. Also the data-change
//! path: a keyed change keeps surviving rows mounted, and the scroll
//! offset survives the deferred apply.
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
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
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
        /// The list's parent and the list itself.
        root: MacosNode,
        list: MacosNode,
        /// The data's row count; a test shrinks it.
        count: Rc<Cell<usize>>,
        /// Every `release_item` call. Each takes the backend's borrow, as
        /// a real row's style teardown does.
        released: Rc<RefCell<Vec<u64>>>,
        /// The data's item keys; `item_key(i)` is `keys[i]`. The scope id
        /// of a row is its key + 1.
        keys: Rc<RefCell<Vec<u64>>>,
        /// Every key `mount_item` mounted, in order.
        mounted_keys: Rc<RefCell<Vec<u64>>>,
        /// Key → the row's root view, latest mount.
        rows_by_key: Rc<RefCell<HashMap<u64, Retained<NSView>>>>,
        /// Keys whose item `item_changed` reports as changed.
        changed: Rc<RefCell<HashSet<u64>>>,
        /// A Known list's size per key; `ESTIMATE` for the rest.
        sizes: Rc<RefCell<HashMap<u64, f32>>>,
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

        /// Replace the data's keys and run a data change the way the
        /// vocabulary's data effect does: under the backend's borrow.
        fn change_keys(&self, keys: Vec<u64>) {
            self.count.set(keys.len());
            *self.keys.borrow_mut() = keys;
            self.backend.borrow_mut().virtualizer_data_changed(&self.list);
        }

        /// The cell frame hosting the row mounted for `key`.
        fn cell_frame_of_key(&self, key: u64) -> CGRect {
            let row = self.rows_by_key.borrow().get(&key).cloned().expect("row mounted");
            let cell: Retained<NSView> = unsafe { msg_send_id![&row, superview] };
            unsafe { msg_send![&cell, frame] }
        }

        fn handle(&self) -> runtime_shared::primitives::virtualizer::VirtualizerHandle {
            self.backend.borrow().make_virtualizer_handle(&self.list)
        }

        fn close(self) {
            self.window.close();
        }
    }

    /// Mount a vertical list of `heights.len()` rows estimated at
    /// `ESTIMATE`. `Some(h)` rows carry an explicit `height: h`; `None` rows
    /// are content-sized around one `SHORT`-tall child.
    fn mount_list(mtm: MainThreadMarker, heights: Vec<Option<f32>>, measure_sizes: bool) -> Mounted {
        mount_list_with(mtm, heights, measure_sizes, VirtualLayout::default(), StyleRules::default())
    }

    /// [`mount_list`] with the list's own layout and extra style rules
    /// (layered over its fixed `LIST_W` x `LIST_H` size).
    fn mount_list_with(
        mtm: MainThreadMarker,
        heights: Vec<Option<f32>>,
        measure_sizes: bool,
        virt_layout: VirtualLayout,
        list_style: StyleRules,
    ) -> Mounted {
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
        let count = Rc::new(Cell::new(heights.len()));
        let released: Rc<RefCell<Vec<u64>>> = Rc::default();
        let keys: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new((0..heights.len() as u64).collect()));
        let mounted_keys: Rc<RefCell<Vec<u64>>> = Rc::default();
        let rows_by_key: Rc<RefCell<HashMap<u64, Retained<NSView>>>> = Rc::default();
        let changed: Rc<RefCell<HashSet<u64>>> = Rc::default();
        let sizes: Rc<RefCell<HashMap<u64, f32>>> = Rc::default();
        let item_key: Rc<dyn Fn(usize) -> u64> = {
            let keys = keys.clone();
            Rc::new(move |i| keys.borrow()[i])
        };
        let mount = {
            let mount_count = count.clone();
            let backend = backend.clone();
            let rows = rows.clone();
            let children = children.clone();
            let item_key = item_key.clone();
            let mounted_keys = mounted_keys.clone();
            let rows_by_key = rows_by_key.clone();
            Rc::new(move |i: usize| {
                assert!(i < mount_count.get(), "mount_item asked past the end of the data");
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
                let key = item_key(i);
                mounted_keys.borrow_mut().push(key);
                rows_by_key.borrow_mut().insert(key, view_of(&root));
                (root, key + 1)
            }) as Rc<dyn Fn(usize) -> (MacosNode, u64)>
        };
        let item_size = {
            let overrides = overrides.clone();
            let count = count.clone();
            let (sizes, item_key) = (sizes.clone(), item_key.clone());
            Rc::new(move |i: usize| {
                // An author's size fn may index its data by `i`.
                assert!(i < count.get(), "item_size asked past the end of the data");
                let by_key = sizes.borrow().get(&item_key(i)).copied();
                by_key.or_else(|| overrides.borrow().get(&i).copied()).unwrap_or(ESTIMATE)
            })
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
            item_count: {
                let count = count.clone();
                Rc::new(move || count.get())
            },
            item_key: item_key.clone(),
            item_size,
            measure_sizes,
            mount_item: mount,
            release_item: {
                let (backend, released) = (backend.clone(), released.clone());
                Rc::new(move |id| {
                    let _b = backend.borrow_mut();
                    released.borrow_mut().push(id);
                })
            },
            set_measured_size,
            on_scroll: None,
            item_changed: Some({
                let changed = changed.clone();
                Rc::new(move |i: usize| changed.borrow().contains(&item_key(i)))
            }),
        };
        let a11y = AccessibilityProps::default();
        let (root, list) = {
            let mut b = backend.borrow_mut();
            let mut root = b.create_view(&a11y);
            b.apply_style(&root, &sized(LIST_W as f32, LIST_H as f32));
            let list = b.create_virtualizer(callbacks, 0.0, virt_layout, &a11y);
            b.apply_style(
                &list,
                &Rc::new(StyleRules {
                    width: px(LIST_W as f32),
                    height: px(LIST_H as f32),
                    ..list_style
                }),
            );
            b.insert(&mut root, list.clone());
            b.finish(root.clone());
            b.run_layout_pass_now();
            (root, list)
        };
        let scroll = view_of(&list);
        let _: () = unsafe { msg_send![&scroll, layoutSubtreeIfNeeded] };
        let cv: Retained<NSView> = unsafe { msg_send_id![&scroll, documentView] };
        let mounted = Mounted {
            backend,
            window,
            cv,
            rows,
            children,
            reports,
            root,
            list,
            count,
            released,
            keys,
            mounted_keys,
            rows_by_key,
            changed,
            sizes,
        };
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

    fn padded(h: f32, v: f32) -> StyleRules {
        StyleRules {
            padding_left: px(h),
            padding_right: px(h),
            padding_top: px(v),
            padding_bottom: px(v),
            ..Default::default()
        }
    }

    /// REGRESSION: padding on the virtualizer's own style was ignored —
    /// rows ran edge to edge (a Kanban column's `padding_horizontal: 8`
    /// card list rendered flush on macOS, inset on web). Like a padded
    /// scroll container on web, the rows sit inside the padding on every
    /// side, a row fills the cross axis less the horizontal padding, and
    /// the top + bottom padding are part of the scroll extent.
    fn regression_virtualizer_padding_insets_its_rows(mtm: MainThreadMarker) {
        const H: f32 = 8.0;
        const V: f32 = 12.0;
        let m = mount_list_with(
            mtm,
            vec![Some(TALL), Some(TALL), Some(TALL)],
            false,
            VirtualLayout::default(),
            padded(H, V),
        );
        let cv_frame: CGRect = unsafe { msg_send![&m.cv, frame] };
        let cv_w = cv_frame.size.width as f32;
        let c0 = m.cell_frame(0);
        assert_close(c0.origin.x, H, "row 0 is inset by padding_left");
        assert_close(c0.origin.y, V, "row 0 is inset by padding_top");
        assert_close(c0.size.width, cv_w - 2.0 * H, "row 0 fills the cross axis less the padding");
        assert_close(m.row_frame(0).size.width, cv_w - 2.0 * H, "row 0 root width");
        assert_close(m.cell_frame(1).origin.y, V + ESTIMATE, "row 1 follows row 0");
        assert_close(m.content_main(), V + 3.0 * ESTIMATE + V, "scroll extent includes top + bottom padding");
        m.close();
    }

    /// The grid half of the padding regression: lanes divide the cross
    /// axis less the horizontal padding, and the first lane starts at it.
    fn regression_virtualizer_padding_insets_its_lanes(mtm: MainThreadMarker) {
        const H: f32 = 10.0;
        const GAP: f32 = 6.0;
        let layout = VirtualLayout {
            lanes: runtime_shared::Lanes::Fixed(2),
            cross_spacing: GAP,
            ..VirtualLayout::default()
        };
        let m = mount_list_with(mtm, vec![Some(TALL); 4], false, layout, padded(H, 0.0));
        let cv_frame: CGRect = unsafe { msg_send![&m.cv, frame] };
        let cv_w = cv_frame.size.width as f32;
        let lane = (cv_w - 2.0 * H - GAP) / 2.0;
        let (c0, c1) = (m.cell_frame(0), m.cell_frame(1));
        assert_close(c0.origin.x, H, "lane 0 starts at padding_left");
        assert_close(c0.size.width, lane, "lane 0 width");
        assert_close(c1.origin.x, H + lane + GAP, "lane 1 follows lane 0 and the gap");
        assert_close(c1.size.width, lane, "lane 1 width");
        assert_close(c1.origin.y, 0.0, "rows 0 and 1 share a line");
        m.close();
    }

    /// REGRESSION: a data change reloaded inside `virtualizer_data_changed`,
    /// under the backend's borrow, and the rows it ended displaying were
    /// released there: "RefCell already borrowed", an abort across AppKit.
    /// The rows are released and the new count shows on a later turn.
    fn regression_a_data_change_under_the_backend_borrow_releases_rows_later(mtm: MainThreadMarker) {
        let m = mount_list(mtm, vec![Some(SHORT); 3], false);
        m.count.set(1);
        {
            let mut b = m.backend.borrow_mut();
            b.virtualizer_data_changed(&m.list);
            // Whatever lays out before the reload sees the old count.
            let layout: Retained<AnyObject> = unsafe { msg_send_id![&m.cv, collectionViewLayout] };
            let _: () = unsafe { msg_send![&layout, invalidateLayout] };
            let _: () = unsafe { msg_send![&m.cv, layoutSubtreeIfNeeded] };
            assert!(m.released.borrow().is_empty(), "nothing is released under the borrow");
        }
        m.settle();
        assert!(!m.released.borrow().is_empty(), "the dropped rows are released");
        assert_close(m.content_main(), ESTIMATE, "one row is left");
        m.close();
    }

    /// REGRESSION: taking a list out of the tree (a route change) ends
    /// displaying its rows under the backend's borrow, with the same abort.
    fn regression_removing_the_list_under_the_backend_borrow_releases_rows_later(mtm: MainThreadMarker) {
        let m = mount_list(mtm, vec![Some(SHORT); 3], false);
        {
            let mut b = m.backend.borrow_mut();
            let mut root = m.root.clone();
            b.remove_child(&mut root, &m.list);
            // AppKit ends displaying its items when the collection view is
            // next told its data is gone; force it here, under the borrow.
            m.count.set(0);
            let _: () = unsafe { msg_send![&m.cv, reloadData] };
            let _: () = unsafe { msg_send![&m.cv, layoutSubtreeIfNeeded] };
            assert!(m.released.borrow().is_empty(), "nothing is released under the borrow");
        }
        pump_for(0.1);
        m.close();
    }

    /// REGRESSION: the scroll view, its clip and the collection view each
    /// painted AppKit's opaque default over whatever the list's parent
    /// painted. The list paints nothing of its own, as on web.
    fn regression_a_list_is_transparent(mtm: MainThreadMarker) {
        let m = mount_list(mtm, vec![Some(SHORT); 3], false);
        let scroll = view_of(&m.list);
        let draws: bool = unsafe { msg_send![&scroll, drawsBackground] };
        assert!(!draws, "the scroll view");
        let clip: Retained<AnyObject> = unsafe { msg_send_id![&scroll, contentView] };
        let draws: bool = unsafe { msg_send![&clip, drawsBackground] };
        assert!(!draws, "the clip view");
        let colors: Retained<AnyObject> = unsafe { msg_send_id![&m.cv, backgroundColors] };
        let color: Retained<AnyObject> = unsafe { msg_send_id![&colors, firstObject] };
        let alpha: f64 = unsafe { msg_send![&color, alphaComponent] };
        assert_eq!(alpha, 0.0, "the collection view");
        m.close();
    }

    /// REGRESSION: the legacy scroller style ("Show scroll bars: Always")
    /// reserves a gutter in the clip, so the rows' cross size no longer
    /// matched the laid-out box. A list uses overlay scrollers whatever the
    /// setting, as `scroll_view` does.
    fn regression_a_list_uses_overlay_scrollers(mtm: MainThreadMarker) {
        const OVERLAY: isize = 1;
        let m = mount_list(mtm, vec![Some(SHORT); 3], false);
        let style: isize = unsafe { msg_send![&view_of(&m.list), scrollerStyle] };
        assert_eq!(style, OVERLAY);
        m.close();
    }

    fn ptr(v: &Retained<NSView>) -> usize {
        &**v as *const NSView as usize
    }

    /// REGRESSION: any change to a list's items rebuilt every visible row
    /// (`reloadData`): a Kanban drop remounted every card in both columns,
    /// ~40ms in one frame in a debug build. A keyed change now moves,
    /// inserts and deletes: the surviving rows keep their mounted view and
    /// scope (no release, no remount), and the order is the new one.
    fn regression_a_keyed_change_keeps_surviving_rows_mounted(mtm: MainThreadMarker) {
        let m = mount_list(mtm, vec![Some(SHORT); 3], false);
        let before: HashMap<u64, usize> =
            m.rows_by_key.borrow().iter().map(|(k, v)| (*k, ptr(v))).collect();
        assert_eq!(before.len(), 3, "precondition: three rows mounted");
        m.mounted_keys.borrow_mut().clear();

        // Key 2 moves to the front, key 1 is deleted, key 7 is inserted.
        m.change_keys(vec![2, 0, 7]);
        assert!(m.mounted_keys.borrow().is_empty(), "nothing mounts under the borrow");
        m.settle();

        assert_eq!(*m.mounted_keys.borrow(), vec![7], "only the inserted key mounts");
        assert_eq!(*m.released.borrow(), vec![1 + 1], "only the deleted key's row is released");
        for key in [0u64, 2] {
            let now = ptr(m.rows_by_key.borrow().get(&key).expect("survivor"));
            assert_eq!(now, before[&key], "key {key} keeps its mounted view");
            let row = m.rows_by_key.borrow().get(&key).cloned().unwrap();
            let host: Option<Retained<NSView>> = unsafe { msg_send_id![&row, superview] };
            assert!(host.is_some(), "key {key} is still hosted in a cell");
        }
        assert_close(m.cell_frame_of_key(2).origin.y, 0.0, "key 2 moved to the front");
        assert_close(m.cell_frame_of_key(0).origin.y, ESTIMATE, "key 0 is second");
        assert_close(m.cell_frame_of_key(7).origin.y, 2.0 * ESTIMATE, "key 7 is third");
        assert_close(m.content_main(), 3.0 * ESTIMATE, "three rows");
        m.close();
    }

    /// Rows of different Known sizes keep theirs through a keyed change, and
    /// a size that changes under the same keys in the same order is picked up
    /// (a raw `virtualizer()` has no item to compare, so no row is changed).
    fn known_sizes_follow_their_keys_and_are_asked_again(mtm: MainThreadMarker) {
        let m = mount_list(mtm, vec![Some(SHORT); 3], false);
        m.sizes.borrow_mut().extend([(0, 30.0), (1, 60.0), (2, 90.0), (9, 45.0)]);
        // Same keys: nothing to batch, the sizes alone changed.
        m.change_keys(vec![0, 1, 2]);
        m.settle();
        assert_close(m.cell_frame_of_key(1).origin.y, 30.0, "row 0's new size is taken");
        assert_close(m.content_main(), 180.0, "and the content's");
        assert!(m.released.borrow().is_empty(), "no row is rebuilt for it");

        // Reordered around an insert and a delete.
        m.change_keys(vec![9, 2, 0]);
        m.settle();
        assert_close(m.cell_frame_of_key(9).size.height, 45.0, "the inserted row");
        assert_close(m.cell_frame_of_key(2).origin.y, 45.0, "key 2 after it");
        assert_close(m.cell_frame_of_key(2).size.height, 90.0, "with its own size");
        assert_close(m.cell_frame_of_key(0).origin.y, 135.0, "key 0 last");
        assert_close(m.content_main(), 165.0, "the content");
        m.close();
    }

    /// REGRESSION: a Robot screenshot is asked for under the backend's borrow
    /// and makes AppKit lay out, which mounts the rows a changed list now
    /// shows. Their mount borrows the backend: an abort.
    fn regression_a_screenshot_mounts_no_row_under_the_backend_borrow(mtm: MainThreadMarker) {
        use runtime_vocabulary::caps::IntrospectionOps;
        let m = mount_list(mtm, vec![Some(SHORT); 3], false);
        m.mounted_keys.borrow_mut().clear();
        let shot = Rc::new(Cell::new(None));
        {
            let slot = shot.clone();
            let b = m.backend.borrow();
            let _: () = unsafe { msg_send![&m.cv, reloadData] };
            b.capture_screenshot(Box::new(move |r| slot.set(Some(r.is_ok()))));
            assert!(m.mounted_keys.borrow().is_empty(), "nothing mounts under the borrow");
        }
        m.settle();
        assert_eq!(shot.get(), Some(true), "the capture is delivered");
        assert!(!m.mounted_keys.borrow().is_empty(), "the rows mount");
        m.close();
    }

    /// A survivor whose item changed (`item_changed`) is rebuilt from its
    /// new item — released and mounted again — while an unchanged
    /// survivor in the same change keeps its row.
    fn a_changed_survivor_is_remounted_in_a_keyed_change(mtm: MainThreadMarker) {
        let m = mount_list(mtm, vec![Some(SHORT); 3], false);
        let kept = ptr(m.rows_by_key.borrow().get(&1).unwrap());
        let stale = ptr(m.rows_by_key.borrow().get(&0).unwrap());
        m.mounted_keys.borrow_mut().clear();

        m.changed.borrow_mut().insert(0);
        m.change_keys(vec![1, 0, 2]);
        m.settle();

        assert_eq!(*m.mounted_keys.borrow(), vec![0], "the changed key remounts");
        assert_eq!(*m.released.borrow(), vec![0 + 1], "its old row is released");
        assert_ne!(ptr(m.rows_by_key.borrow().get(&0).unwrap()), stale, "a fresh row");
        assert_eq!(ptr(m.rows_by_key.borrow().get(&1).unwrap()), kept, "key 1 keeps its row");
        assert_close(m.cell_frame_of_key(1).origin.y, 0.0, "key 1 moved to the front");
        assert_close(m.cell_frame_of_key(0).origin.y, ESTIMATE, "key 0 is second");
        m.close();
    }

    /// REGRESSION: a `scroll_to` made before a deferred data change applied
    /// was lost — the apply's reload put the list back at offset 0 (Timely's
    /// calendar scrolled to Today on mount, then showed 2016). The offset
    /// holds across a no-op change (the mount-time data effect), a keyed
    /// batch and a full reload.
    fn regression_scroll_to_survives_the_deferred_data_change(mtm: MainThreadMarker) {
        const ROWS: usize = 20;
        const Y: f32 = 500.0;
        let m = mount_list(mtm, vec![Some(SHORT); ROWS], false);
        let handle = m.handle();

        // The mount-time data effect: same data, applied a turn later.
        m.backend.borrow_mut().virtualizer_data_changed(&m.list);
        handle.scroll_to(0.0, Y);
        m.settle();
        assert_close(handle.scroll_offset().1 as f64, Y, "offset after a no-op change");

        // A keyed batch (one insert at the front).
        let mut keys: Vec<u64> = (0..ROWS as u64).collect();
        keys.insert(0, 100);
        keys.pop();
        handle.scroll_to(0.0, Y);
        m.change_keys(keys);
        m.settle();
        assert_close(handle.scroll_offset().1 as f64, Y, "offset after a keyed batch");

        // A full reload (no key survives).
        handle.scroll_to(0.0, Y);
        m.change_keys((200..200 + ROWS as u64).collect());
        m.settle();
        assert_close(handle.scroll_offset().1 as f64, Y, "offset after a reload");
        m.close();
    }

    /// REGRESSION (scroll snapped to the start on a layout pass): the
    /// layout pass sized every scroll view's documentView to its Taffy
    /// children's bounding box. A virtualizer's rows are the collection view's
    /// items, not Taffy children, so the pass shrank the collection view to
    /// the viewport and AppKit clamped the offset to 0. Timely's calendar,
    /// parked at Today thousands of lanes in, opened on the first lane.
    fn regression_a_layout_pass_keeps_the_scroll_offset(mtm: MainThreadMarker) {
        const ROWS: usize = 40;
        const Y: f32 = 900.0;
        let m = mount_list(mtm, vec![Some(SHORT); ROWS], false);
        let handle = m.handle();
        m.settle();
        handle.scroll_to(0.0, Y);
        m.settle();
        assert_close(handle.scroll_offset().1 as f64, Y, "offset after scroll_to");
        m.backend.borrow_mut().run_layout_pass_now();
        m.settle();
        assert_close(handle.scroll_offset().1 as f64, Y, "offset after a layout pass");
        m.close();
    }

    /// REGRESSION (rows kept a stale cross size): `tile` re-queried item
    /// sizes only when IT resized the collection view. When the collection
    /// view already matched the clip (AppKit resized it with the clip on a
    /// window resize), the flow layout kept the rows' old cross size and
    /// centred them: Timely's calendar lanes, sized for the window's first
    /// height, sat 117pt down after it grew.
    fn regression_rows_follow_a_cross_axis_resize(mtm: MainThreadMarker) {
        const NARROW: f64 = 300.0;
        let m = mount_list(mtm, vec![Some(SHORT); 4], false);
        m.settle();
        assert_close(m.cell_frame(0).size.width, LIST_W as f32, "rows start the list's width");
        // The document reaches the new width by itself, then the list does.
        let _: () = unsafe { msg_send![&m.cv, setFrameSize: CGSize { width: NARROW, height: m.cv.frame().size.height }] };
        {
            let mut b = m.backend.borrow_mut();
            b.apply_style(
                &m.list,
                &Rc::new(StyleRules { width: px(NARROW as f32), height: px(LIST_H as f32), ..Default::default() }),
            );
            b.run_layout_pass_now();
        }
        m.settle();
        assert_close(m.cell_frame(0).size.width, NARROW as f32, "rows follow the new width");
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
        let tests: [(&str, fn(MainThreadMarker)); 17] = [
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
            (
                "regression_virtualizer_padding_insets_its_lanes",
                regression_virtualizer_padding_insets_its_lanes,
            ),
            (
                "regression_virtualizer_padding_insets_its_rows",
                regression_virtualizer_padding_insets_its_rows,
            ),
            (
                "regression_a_data_change_under_the_backend_borrow_releases_rows_later",
                regression_a_data_change_under_the_backend_borrow_releases_rows_later,
            ),
            (
                "regression_removing_the_list_under_the_backend_borrow_releases_rows_later",
                regression_removing_the_list_under_the_backend_borrow_releases_rows_later,
            ),
            ("regression_a_list_is_transparent", regression_a_list_is_transparent),
            ("regression_a_list_uses_overlay_scrollers", regression_a_list_uses_overlay_scrollers),
            (
                "regression_a_keyed_change_keeps_surviving_rows_mounted",
                regression_a_keyed_change_keeps_surviving_rows_mounted,
            ),
            (
                "regression_a_screenshot_mounts_no_row_under_the_backend_borrow",
                regression_a_screenshot_mounts_no_row_under_the_backend_borrow,
            ),
            (
                "known_sizes_follow_their_keys_and_are_asked_again",
                known_sizes_follow_their_keys_and_are_asked_again,
            ),
            (
                "a_changed_survivor_is_remounted_in_a_keyed_change",
                a_changed_survivor_is_remounted_in_a_keyed_change,
            ),
            (
                "regression_scroll_to_survives_the_deferred_data_change",
                regression_scroll_to_survives_the_deferred_data_change,
            ),
            ("regression_a_layout_pass_keeps_the_scroll_offset", regression_a_layout_pass_keeps_the_scroll_offset),
            ("regression_rows_follow_a_cross_axis_resize", regression_rows_follow_a_cross_axis_resize),
        ];
        for (name, t) in tests {
            t(mtm);
            println!("test {name} ... ok");
        }
        println!("test result: ok. {} passed", tests.len());
    }
}
