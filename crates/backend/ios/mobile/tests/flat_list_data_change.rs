//! A `flat_list` whose data changes, on real UIKit: the change is applied on
//! a later turn as one keyed batch that keeps every row whose item did not
//! change, and reloads when it can't be.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator and
//! run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test flat_list_data_change --no-run
//! xcrun simctl spawn booted target/aarch64-apple-ios-sim/debug/deps/flat_list_data_change-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::rc::Rc;

    use backend_ios::{IosBackend, IosNode};
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Length, StyleRules, VirtualLayout, VirtualizerCallbacks};
    use runtime_vocabulary::caps::{IntrospectionOps, LifecycleOps, StyleOps, ViewOps, VirtualizerOps};

    const WIN_W: f64 = 400.0;
    const WIN_H: f64 = 600.0;
    const ROW: f32 = 50.0;

    fn px(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
        Some(Length::Px(v).into())
    }

    fn view_of(node: &IosNode) -> Retained<UIView> {
        let v: &UIView = node.as_view();
        unsafe { Retained::retain(v as *const UIView as *mut UIView) }.expect("retain")
    }

    fn spin() {
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            static kCFRunLoopDefaultMode: *const std::ffi::c_void;
            fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, ret: u8) -> i32;
        }
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, 0) };
    }

    struct Mounted {
        backend: Rc<RefCell<IosBackend>>,
        _window: Retained<UIWindow>,
        list: IosNode,
        cv: Retained<UIView>,
        keys: Rc<RefCell<Vec<u64>>>,
        sizes: Rc<RefCell<HashMap<u64, f32>>>,
        changed: Rc<RefCell<HashSet<u64>>>,
        /// Every mount and release, by key.
        mounted: Rc<RefCell<Vec<u64>>>,
        released: Rc<RefCell<Vec<u64>>>,
        rows: Rc<RefCell<HashMap<u64, Retained<UIView>>>>,
        gone: Rc<Cell<bool>>,
    }

    impl Mounted {
        fn settle(&self) {
            for _ in 0..4 {
                spin();
                let _: () = unsafe { msg_send![&self.cv, layoutIfNeeded] };
                self.backend.borrow_mut().run_layout();
            }
        }

        /// Replace the keys and run a data change the way the vocabulary's
        /// data effect does: under the backend's borrow.
        fn change(&self, keys: Vec<u64>) {
            *self.keys.borrow_mut() = keys;
            self.backend.borrow_mut().virtualizer_data_changed(&self.list);
        }

        fn clear_log(&self) {
            self.mounted.borrow_mut().clear();
            self.released.borrow_mut().clear();
        }

        fn ptr(&self, key: u64) -> usize {
            &**self.rows.borrow().get(&key).expect("mounted") as *const UIView as usize
        }

        /// `(y, height)` of the cell hosting `key`'s row; panics when the
        /// row is in no cell of the list.
        fn cell(&self, key: u64) -> (f64, f64) {
            let row = self.rows.borrow().get(&key).cloned().expect("mounted");
            let content: Option<Retained<UIView>> = unsafe { msg_send_id![&row, superview] };
            let content = content.unwrap_or_else(|| panic!("key {key}'s row is in no cell"));
            let cell: Retained<UIView> = unsafe { msg_send_id![&content, superview] };
            let in_list: bool = unsafe { msg_send![&cell, isDescendantOfView: &*self.cv] };
            assert!(in_list, "key {key}'s cell is in the list");
            let f: CGRect = unsafe { msg_send![&cell, frame] };
            (f.origin.y, f.size.height)
        }

        fn offset(&self) -> f64 {
            let o: CGPoint = unsafe { msg_send![&self.cv, contentOffset] };
            o.y
        }

        fn content(&self) -> f64 {
            let s: CGSize = unsafe { msg_send![&self.cv, contentSize] };
            s.height
        }
    }

    fn mount(mtm: MainThreadMarker, count: u64) -> Mounted {
        let backend = Rc::new(RefCell::new(IosBackend::new(mtm)));
        backend_ios::install_global_self(Rc::downgrade(&backend));
        let frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: WIN_W, height: WIN_H },
        };
        let window: Retained<UIWindow> =
            unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame] };
        let _: () = unsafe { msg_send![&window, setHidden: false] };
        let host: Retained<UIView> =
            unsafe { msg_send_id![mtm.alloc::<UIView>(), initWithFrame: frame] };
        unsafe { window.addSubview(&host) };

        let keys = Rc::new(RefCell::new((0..count).collect::<Vec<u64>>()));
        let sizes: Rc<RefCell<HashMap<u64, f32>>> = Rc::default();
        let changed: Rc<RefCell<HashSet<u64>>> = Rc::default();
        let mounted: Rc<RefCell<Vec<u64>>> = Rc::default();
        let released: Rc<RefCell<Vec<u64>>> = Rc::default();
        let rows: Rc<RefCell<HashMap<u64, Retained<UIView>>>> = Rc::default();
        let scope_keys: Rc<RefCell<HashMap<u64, u64>>> = Rc::default();
        let next_scope = Rc::new(Cell::new(1u64));
        let gone = Rc::new(Cell::new(false));
        let item_key: Rc<dyn Fn(usize) -> u64> = {
            let keys = keys.clone();
            Rc::new(move |i| keys.borrow()[i])
        };
        let callbacks = VirtualizerCallbacks {
            item_count: {
                let keys = keys.clone();
                Rc::new(move || keys.borrow().len())
            },
            item_key: item_key.clone(),
            item_size: {
                let (sizes, item_key) = (sizes.clone(), item_key.clone());
                Rc::new(move |i| sizes.borrow().get(&item_key(i)).copied().unwrap_or(ROW))
            },
            measure_sizes: false,
            mount_item: {
                let (backend, item_key) = (backend.clone(), item_key.clone());
                let (mounted, rows) = (mounted.clone(), rows.clone());
                let (scope_keys, next_scope) = (scope_keys.clone(), next_scope.clone());
                Rc::new(move |i| {
                    let mut b = backend.borrow_mut();
                    let row = b.create_view(&AccessibilityProps::default());
                    let key = item_key(i);
                    mounted.borrow_mut().push(key);
                    rows.borrow_mut().insert(key, view_of(&row));
                    let scope = next_scope.replace(next_scope.get() + 1);
                    scope_keys.borrow_mut().insert(scope, key);
                    (row, scope)
                })
            },
            release_item: {
                let (backend, released) = (backend.clone(), released.clone());
                let gone = gone.clone();
                Rc::new(move |scope| {
                    // The vocabulary's teardown drains every row before it
                    // releases the list, under the backend's borrow.
                    if gone.get() {
                        return;
                    }
                    // A real row's style teardown borrows the backend.
                    drop(backend.borrow_mut());
                    released.borrow_mut().push(scope_keys.borrow()[&scope]);
                })
            },
            set_measured_size: Rc::new(|_, _| {}),
            on_scroll: None,
            item_changed: {
                let (changed, item_key) = (changed.clone(), item_key.clone());
                Some(Rc::new(move |i| changed.borrow().contains(&item_key(i))))
            },
        };
        let a11y = AccessibilityProps::default();
        let (list, cv) = {
            let mut b = backend.borrow_mut();
            b.set_host_root(host);
            let mut root = b.create_view(&a11y);
            let full = StyleRules {
                width: px(WIN_W as f32),
                height: px(WIN_H as f32),
                ..Default::default()
            };
            b.apply_style(&root, &Rc::new(full.clone()));
            let list = b.create_virtualizer(callbacks, 0.0, VirtualLayout::default(), &a11y);
            b.apply_style(&list, &Rc::new(full));
            let cv = view_of(&list);
            b.insert(&mut root, list.clone());
            b.finish(root);
            b.run_layout();
            (list, cv)
        };
        let m = Mounted {
            backend,
            _window: window,
            list,
            cv,
            keys,
            sizes,
            changed,
            mounted,
            released,
            rows,
            gone,
        };
        m.settle();
        m
    }

    /// REGRESSION: any change rebuilt every visible row (`reloadData`). A
    /// keyed change moves, inserts and deletes: surviving rows keep their
    /// view and scope, a deleted row is released at once (its cell sits in
    /// UIKit's reuse pool, maybe for good), and the order is the new one.
    fn regression_a_keyed_change_keeps_surviving_rows(mtm: MainThreadMarker) {
        let m = mount(mtm, 3);
        let (zero, two) = (m.ptr(0), m.ptr(2));
        m.clear_log();
        m.change(vec![2, 0, 7]);
        assert!(m.mounted.borrow().is_empty() && m.released.borrow().is_empty(), "nothing under the borrow");
        m.settle();
        assert_eq!(*m.mounted.borrow(), vec![7], "only the inserted key mounts");
        assert_eq!(*m.released.borrow(), vec![1], "only the deleted key is released");
        assert_eq!((m.ptr(0), m.ptr(2)), (zero, two), "survivors keep their views");
        let row = ROW as f64;
        assert_eq!((m.cell(2).0, m.cell(0).0, m.cell(7).0), (0.0, row, 2.0 * row), "the new order");
        assert_eq!(m.content(), 3.0 * row);
        // Nothing is inserted to reuse the deleted row's cell.
        m.clear_log();
        m.change(vec![2, 7]);
        m.settle();
        assert_eq!(*m.released.borrow(), vec![0], "released though its cell is not reused");
        assert!(m.mounted.borrow().is_empty());
    }

    /// A survivor whose item changed is rebuilt; its old row is released and
    /// an unchanged survivor keeps its own.
    fn a_changed_survivor_is_rebuilt(mtm: MainThreadMarker) {
        let m = mount(mtm, 3);
        let one = m.ptr(1);
        m.clear_log();
        m.changed.borrow_mut().insert(0);
        m.change(vec![1, 0, 2]);
        m.settle();
        assert_eq!(*m.mounted.borrow(), vec![0], "the changed key remounts");
        assert_eq!(*m.released.borrow(), vec![0], "its old row is released, once");
        assert_eq!(m.ptr(1), one, "key 1 keeps its row");
        assert_eq!((m.cell(1).0, m.cell(0).0), (0.0, ROW as f64));
    }

    /// The scroll offset holds across a no-op change, a keyed batch and a
    /// reload.
    fn the_scroll_offset_survives_a_change(mtm: MainThreadMarker) {
        let m = mount(mtm, 60);
        let to = CGPoint { x: 0.0, y: 900.0 };
        let _: () = unsafe { msg_send![&m.cv, setContentOffset: to, animated: false] };
        m.settle();
        let at = m.offset();
        assert!(at > 800.0, "scrolled: {at}");
        m.change((0..60).collect());
        m.settle();
        assert_eq!(m.offset(), at, "a no-op change");
        let mut keys: Vec<u64> = (0..59).collect();
        keys.insert(0, 100);
        m.change(keys);
        m.settle();
        assert_eq!(m.offset(), at, "a keyed batch");
        m.change((200..260).collect());
        m.settle();
        assert_eq!(m.offset(), at, "a reload");
    }

    /// Sizes are asked for again under the same keys, and follow their keys
    /// through a batch.
    fn sizes_follow_their_keys(mtm: MainThreadMarker) {
        let m = mount(mtm, 3);
        m.sizes.borrow_mut().extend([(0, 30.0), (1, 60.0), (2, 90.0), (9, 45.0)]);
        m.clear_log();
        m.change(vec![0, 1, 2]);
        m.settle();
        assert_eq!(m.cell(1), (30.0, 60.0), "new sizes, same keys");
        assert!(m.released.borrow().is_empty(), "no row is rebuilt for it");
        m.change(vec![9, 2, 0]);
        m.settle();
        assert_eq!((m.cell(9), m.cell(2), m.cell(0)), ((0.0, 45.0), (45.0, 90.0), (135.0, 30.0)));
        assert_eq!(m.content(), 165.0);
    }

    /// Changes that can't be batched, or come in a burst, or shrink the data
    /// with a layout in the gap, end at the live data.
    fn any_change_ends_at_the_live_data(mtm: MainThreadMarker) {
        let m = mount(mtm, 10);
        m.change((0..3).collect());
        // A layout between the change and its apply asks about rows 3..10.
        let _: () = unsafe { msg_send![&m.cv, setNeedsLayout] };
        let _: () = unsafe { msg_send![&m.cv, layoutIfNeeded] };
        m.change(vec![2, 2, 5]);
        m.settle();
        assert_eq!(m.content(), 3.0 * ROW as f64, "repeated keys reload");
        m.change((0..60).collect());
        m.settle();
        // Scrolled in the gap: rows the data no longer has come into view.
        m.change((0..3).collect());
        let to = CGPoint { x: 0.0, y: 900.0 };
        let _: () = unsafe { msg_send![&m.cv, setContentOffset: to, animated: false] };
        let _: () = unsafe { msg_send![&m.cv, layoutIfNeeded] };
        m.settle();
        assert_eq!(m.content(), 3.0 * ROW as f64);
        m.change(vec![7, 2]);
        m.change(vec![4, 3, 2, 1]);
        m.settle();
        assert_eq!(m.content(), 4.0 * ROW as f64);
        assert_eq!((m.cell(4).0, m.cell(1).0), (0.0, 3.0 * ROW as f64));
        m.change(Vec::new());
        m.settle();
        assert_eq!(m.content(), 0.0);
        m.change(vec![8, 9]);
        m.settle();
        assert_eq!(m.cell(9).0, ROW as f64);
        // Removed with a change pending: nothing is applied.
        m.change(vec![9, 8]);
        m.gone.set(true);
        m.backend.borrow_mut().release_virtualizer(&m.list);
        m.clear_log();
        spin();
        assert!(m.mounted.borrow().is_empty());
    }

    /// REGRESSION: a Robot screenshot is asked for under the backend's
    /// borrow and makes UIKit lay out, which mounts the rows a changed list
    /// now shows. Their mount borrows the backend: an abort.
    fn regression_a_screenshot_mounts_no_row_under_the_backend_borrow(mtm: MainThreadMarker) {
        let m = mount(mtm, 3);
        m.clear_log();
        let shot = Rc::new(Cell::new(false));
        {
            let slot = shot.clone();
            let b = m.backend.borrow();
            let _: () = unsafe { msg_send![&m.cv, reloadData] };
            b.capture_screenshot(Box::new(move |_| slot.set(true)));
            // UIKit's forced layout can't be provoked in a process with no
            // render server, so this is the deferral itself.
            assert!(!shot.get(), "captured on a later turn, with no borrow held");
        }
        m.settle();
        assert!(shot.get(), "the capture is delivered");
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_ios::install_scheduler();
                regression_a_keyed_change_keeps_surviving_rows(mtm);
                a_changed_survivor_is_rebuilt(mtm);
                the_scroll_offset_survives_a_change(mtm);
                sizes_follow_their_keys(mtm);
                any_change_ends_at_the_live_data(mtm);
                regression_a_screenshot_mounts_no_row_under_the_backend_borrow(mtm);
        println!("flat_list_data_change: 6 passed");
    }
}
