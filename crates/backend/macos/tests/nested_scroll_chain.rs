//! Nested scrollers against real AppKit: a scroll gesture along an axis the
//! inner scroller cannot scroll goes to the enclosing scroller, and stays
//! there for the rest of the gesture (`src/imp/scroll_chain.rs`).
//!
//! REGRESSION (Timely Kanban board): the board is a horizontal `scroll_view`
//! of columns, each holding a vertical list. A two-finger horizontal pan over
//! a column stayed with the column's list — AppKit hands a gesture to the
//! innermost `NSScrollView` under the pointer and that scroll view kept it —
//! so the board never moved. The web chains such a gesture to the ancestor
//! scroller.
//!
//! The events are built as `CGEvent`s (phases, momentum phases and pixel
//! deltas set on them), wrapped with `+[NSEvent eventWithCGEvent:]` and
//! delivered straight to the INNER scroll view's `scrollWheel:` — where
//! AppKit's responder chain brings a wheel event over a row. The window is
//! never ordered on screen.
//!
//! `harness = false`: AppKit views and windows are main-thread only, and
//! libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test nested_scroll_chain
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
    use std::ffi::c_void;
    use std::rc::Rc;

    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::primitives::virtualizer::VirtualLayout;
    use runtime_shared::{
        FlexDirection, Length, OverscrollBehavior, ScrollAxis, StyleRules, VirtualizerCallbacks,
    };
    use runtime_vocabulary::caps::{LifecycleOps, ScrollOps, StyleOps, ViewOps, VirtualizerOps};

    #[allow(non_upper_case_globals)]
    extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after: u8) -> i32;
    }

    /// Opaque `CGEventRef` target, encoded as `^{__CGEvent=}` so objc2's
    /// debug message check accepts it for `+eventWithCGEvent:`.
    #[repr(C)]
    struct CGEvent {
        _p: [u8; 0],
    }
    unsafe impl objc2::encode::RefEncode for CGEvent {
        const ENCODING_REF: objc2::encode::Encoding =
            objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("__CGEvent", &[]));
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreateScrollWheelEvent2(
            source: *const c_void,
            units: u32,
            wheel_count: u32,
            wheel1: i32,
            wheel2: i32,
            wheel3: i32,
        ) -> *mut CGEvent;
        fn CGEventSetIntegerValueField(event: *mut CGEvent, field: u32, value: i64);
        fn CFRelease(cf: *const c_void);
    }

    // CGScrollEventUnit.
    const UNIT_PIXEL: u32 = 0;
    const UNIT_LINE: u32 = 1;
    // CGEventField.
    const FIELD_IS_CONTINUOUS: u32 = 88;
    const FIELD_POINT_DELTA_1: u32 = 96;
    const FIELD_POINT_DELTA_2: u32 = 97;
    const FIELD_SCROLL_PHASE: u32 = 99;
    const FIELD_MOMENTUM_PHASE: u32 = 123;
    // CGScrollPhase.
    const CG_BEGAN: i64 = 1;
    const CG_CHANGED: i64 = 2;
    const CG_ENDED: i64 = 4;
    const CG_MAY_BEGIN: i64 = 128;
    // CGMomentumScrollPhase.
    const CG_MOMENTUM_BEGIN: i64 = 1;
    const CG_MOMENTUM_CONTINUE: i64 = 2;
    const CG_MOMENTUM_END: i64 = 3;

    const WINDOW_W: f64 = 400.0;
    const WINDOW_H: f64 = 300.0;
    const COLUMN_W: f32 = 200.0;
    const COLUMNS: usize = 4;
    const ROW_H: f32 = 100.0;
    const ROWS: usize = 20;

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
        Rc::new(StyleRules {
            width: px(w),
            height: px(h),
            ..Default::default()
        })
    }

    fn pump(turns: usize) {
        for _ in 0..turns {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
        }
    }

    fn window(mtm: MainThreadMarker) -> (Retained<NSWindow>, Retained<NSView>) {
        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: WINDOW_W,
                height: WINDOW_H,
            },
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

    /// A wheel event. `phase` / `momentum` of `0` is none; with both none and
    /// `UNIT_LINE` it is a notched mouse wheel. Deltas follow AppKit's sign:
    /// negative moves the offset forward (right / down).
    fn wheel_event(units: u32, dx: i32, dy: i32, phase: i64, momentum: i64) -> Retained<AnyObject> {
        unsafe {
            let cg = CGEventCreateScrollWheelEvent2(std::ptr::null(), units, 2, dy, dx, 0);
            assert!(!cg.is_null(), "CGEventCreateScrollWheelEvent2");
            if units == UNIT_PIXEL {
                CGEventSetIntegerValueField(cg, FIELD_IS_CONTINUOUS, 1);
                CGEventSetIntegerValueField(cg, FIELD_POINT_DELTA_1, dy as i64);
                CGEventSetIntegerValueField(cg, FIELD_POINT_DELTA_2, dx as i64);
            }
            CGEventSetIntegerValueField(cg, FIELD_SCROLL_PHASE, phase);
            CGEventSetIntegerValueField(cg, FIELD_MOMENTUM_PHASE, momentum);
            let ev: Retained<AnyObject> = msg_send_id![class!(NSEvent), eventWithCGEvent: cg];
            CFRelease(cg as *const c_void);
            ev
        }
    }

    /// A trackpad gesture moving `(dx, dy)` per event: mayBegin, began, a few
    /// changes, ended, then momentum. Each event goes to `target`'s
    /// `scrollWheel:`.
    fn trackpad_gesture(target: &NSView, dx: i32, dy: i32) {
        let mut events = vec![
            wheel_event(UNIT_PIXEL, 0, 0, CG_MAY_BEGIN, 0),
            wheel_event(UNIT_PIXEL, dx, dy, CG_BEGAN, 0),
        ];
        for _ in 0..4 {
            events.push(wheel_event(UNIT_PIXEL, dx, dy, CG_CHANGED, 0));
        }
        events.push(wheel_event(UNIT_PIXEL, 0, 0, CG_ENDED, 0));
        events.push(wheel_event(UNIT_PIXEL, dx, dy, 0, CG_MOMENTUM_BEGIN));
        events.push(wheel_event(UNIT_PIXEL, dx, dy, 0, CG_MOMENTUM_CONTINUE));
        events.push(wheel_event(UNIT_PIXEL, 0, 0, 0, CG_MOMENTUM_END));
        for ev in &events {
            let _: () = unsafe { msg_send![target, scrollWheel: &**ev] };
            pump(1);
        }
        pump(5);
    }

    /// A notched mouse wheel tick (no phase) delivered to `target`.
    fn mouse_wheel(target: &NSView, dx_lines: i32, dy_lines: i32) {
        let ev = wheel_event(UNIT_LINE, dx_lines, dy_lines, 0, 0);
        let _: () = unsafe { msg_send![target, scrollWheel: &*ev] };
        pump(5);
    }

    fn offset(scroll: &NSView) -> CGPoint {
        let clip: Retained<NSView> = unsafe { msg_send_id![scroll, contentView] };
        let bounds: CGRect = unsafe { msg_send![&clip, bounds] };
        bounds.origin
    }

    /// Park `scroll` mid-content so a scroll either way is visible.
    fn set_offset(scroll: &NSView, x: f64, y: f64) {
        let clip: Retained<NSView> = unsafe { msg_send_id![scroll, contentView] };
        let _: () = unsafe { msg_send![&clip, scrollToPoint: CGPoint { x, y }] };
        let _: () = unsafe { msg_send![scroll, reflectScrolledClipView: &*clip] };
    }

    /// What the board fixture left behind.
    struct Board {
        /// Owns the backend; the global handle the list cells use is weak.
        backend: Rc<RefCell<MacosBackend>>,
        window: Retained<NSWindow>,
        /// The horizontal board.
        outer: Retained<NSView>,
        /// The vertical scroller in the first column.
        inner: Retained<NSView>,
        inner_node: MacosNode,
    }

    impl Board {
        /// Park both scrollers mid-content.
        fn reset(&self) {
            set_offset(&self.outer, 100.0, 0.0);
            set_offset(&self.inner, 0.0, 400.0);
            assert_eq!(offset(&self.outer).x, 100.0, "the board parks mid-content");
            assert_eq!(offset(&self.inner).y, 400.0, "the list parks mid-content");
        }

        fn moved(&self) -> (f64, f64) {
            (
                (offset(&self.outer).x - 100.0).abs(),
                (offset(&self.inner).y - 400.0).abs(),
            )
        }

        fn close(self) {
            self.window.close();
        }
    }

    enum Inner {
        /// A vertical `scroll_view` holding one tall child.
        ScrollView,
        /// A vertical `flat_list` (`VirtualizerScrollView`).
        List,
    }

    /// A horizontal board `COLUMNS` columns wide (wider than the window);
    /// the first column holds a vertical scroller taller than its content.
    fn mount_board(mtm: MainThreadMarker, inner_kind: Inner) -> Board {
        let backend = Rc::new(RefCell::new(MacosBackend::new(mtm)));
        // List cells lay their rows out through the global backend handle.
        backend_macos::install_global_self(Rc::downgrade(&backend));
        let (window, content) = window(mtm);
        backend.borrow_mut().set_host_root(content.clone());
        let a11y = AccessibilityProps::default();

        let mut b = backend.borrow_mut();
        let mut root = b.create_view(&a11y);
        b.apply_style(&root, &sized(WINDOW_W as f32, WINDOW_H as f32));
        let mut board = b.create_scroll_view(ScrollAxis::Horizontal, None, &a11y);
        b.apply_style(&board, &sized(WINDOW_W as f32, WINDOW_H as f32));
        let mut row = b.create_view(&a11y);
        b.apply_style(
            &row,
            &Rc::new(StyleRules {
                flex_direction: Some(FlexDirection::Row),
                height: px(WINDOW_H as f32),
                ..Default::default()
            }),
        );
        let mut columns = Vec::new();
        for _ in 0..COLUMNS {
            let column = b.create_view(&a11y);
            b.apply_style(&column, &sized(COLUMN_W, WINDOW_H as f32));
            columns.push(column);
        }
        let inner_node = match inner_kind {
            Inner::ScrollView => {
                let mut list = b.create_scroll_view(ScrollAxis::Vertical, None, &a11y);
                b.apply_style(&list, &sized(COLUMN_W, WINDOW_H as f32));
                let tall = b.create_view(&a11y);
                b.apply_style(&tall, &sized(COLUMN_W, ROW_H * ROWS as f32));
                b.insert(&mut list, tall);
                list
            }
            Inner::List => {
                let backend_for_rows = backend.clone();
                let callbacks = VirtualizerCallbacks {
                    item_count: Rc::new(|| ROWS),
                    item_key: Rc::new(|i| i as u64),
                    item_size: Rc::new(|_| ROW_H),
                    measure_sizes: false,
                    mount_item: Rc::new(move |i: usize| {
                        let mut b = backend_for_rows.borrow_mut();
                        let row = b.create_view(&AccessibilityProps::default());
                        b.apply_style(&row, &sized(COLUMN_W, ROW_H));
                        (row, i as u64 + 1)
                    }),
                    release_item: Rc::new(|_| {}),
                    set_measured_size: Rc::new(|_, _| {}),
                    on_scroll: None,
                    item_changed: None,
                };
                let list = b.create_virtualizer(callbacks, 0.0, VirtualLayout::default(), &a11y);
                b.apply_style(&list, &sized(COLUMN_W, WINDOW_H as f32));
                list
            }
        };
        let mut first = columns.remove(0);
        b.insert(&mut first, inner_node.clone());
        b.insert(&mut row, first);
        for column in columns {
            b.insert(&mut row, column);
        }
        b.insert(&mut board, row);
        b.insert(&mut root, board.clone());
        b.finish(root);
        b.run_layout_pass_now();
        drop(b);

        let outer = view_of(&board);
        let inner = view_of(&inner_node);
        let _: () = unsafe { msg_send![&outer, layoutSubtreeIfNeeded] };
        pump(10);
        let _: () = unsafe { msg_send![&outer, layoutSubtreeIfNeeded] };

        // The fixture is what it claims: the board is wider than the window,
        // the list taller than its column, and the list sits in the board.
        let outer_doc: Retained<NSView> = unsafe { msg_send_id![&outer, documentView] };
        let outer_doc_frame: CGRect = unsafe { msg_send![&outer_doc, frame] };
        assert!(
            outer_doc_frame.size.width > WINDOW_W,
            "the board's content ({}) is wider than its viewport",
            outer_doc_frame.size.width
        );
        let inner_doc: Retained<NSView> = unsafe { msg_send_id![&inner, documentView] };
        let inner_doc_frame: CGRect = unsafe { msg_send![&inner_doc, frame] };
        assert!(
            inner_doc_frame.size.height > WINDOW_H,
            "the list's content ({}) is taller than its viewport",
            inner_doc_frame.size.height
        );
        let enclosing: *mut NSView = unsafe { msg_send![&inner, enclosingScrollView] };
        assert!(
            std::ptr::eq(enclosing as *const NSView, &*outer as *const NSView),
            "the list is inside the board"
        );

        Board {
            backend,
            window,
            outer,
            inner,
            inner_node,
        }
    }

    /// REGRESSION: a horizontal trackpad pan over the column's list scrolls
    /// the board — every phase of it, momentum included — and leaves the
    /// list where it was.
    fn regression_horizontal_pan_over_list_scrolls_the_board(board: &Board, what: &str) {
        board.reset();
        trackpad_gesture(&board.inner, -12, 0);
        let (outer, inner) = board.moved();
        assert!(
            outer > 1.0,
            "{what}: the horizontal pan scrolled the board (moved {outer})"
        );
        assert_eq!(inner, 0.0, "{what}: the horizontal pan left the list alone");
    }

    /// A vertical pan over the list scrolls the list, not the board.
    fn vertical_pan_over_list_scrolls_the_list(board: &Board, what: &str) {
        board.reset();
        trackpad_gesture(&board.inner, 0, -12);
        let (outer, inner) = board.moved();
        assert!(
            inner > 1.0,
            "{what}: the vertical pan scrolled the list (moved {inner})"
        );
        assert_eq!(outer, 0.0, "{what}: the vertical pan left the board alone");
    }

    /// A plain mouse wheel is vertical (the list); shift+wheel arrives as a
    /// horizontal delta (the board).
    fn mouse_wheel_and_shift_wheel_split_by_axis(board: &Board, what: &str) {
        board.reset();
        mouse_wheel(&board.inner, 0, -3);
        let (outer, inner) = board.moved();
        assert!(
            inner > 1.0,
            "{what}: a wheel tick scrolled the list (moved {inner})"
        );
        assert_eq!(outer, 0.0, "{what}: a wheel tick left the board alone");

        board.reset();
        mouse_wheel(&board.inner, -3, 0);
        let (outer, inner) = board.moved();
        assert!(
            outer > 1.0,
            "{what}: a shift+wheel tick scrolled the board (moved {outer})"
        );
        assert_eq!(inner, 0.0, "{what}: a shift+wheel tick left the list alone");
    }

    /// `overscroll-behavior: contain` on the list keeps every gesture, as in
    /// CSS; back to `auto`, chaining resumes.
    fn overscroll_contain_stops_chaining(board: &Board, what: &str) {
        let style = |behavior| {
            Rc::new(StyleRules {
                width: px(COLUMN_W),
                height: px(WINDOW_H as f32),
                overscroll_behavior: Some(behavior),
                ..Default::default()
            })
        };
        board
            .backend
            .borrow_mut()
            .apply_style(&board.inner_node, &style(OverscrollBehavior::Contain));
        board.reset();
        trackpad_gesture(&board.inner, -12, 0);
        mouse_wheel(&board.inner, -3, 0);
        assert_eq!(
            board.moved().0,
            0.0,
            "{what}: a contained list does not chain to the board"
        );

        board
            .backend
            .borrow_mut()
            .apply_style(&board.inner_node, &style(OverscrollBehavior::Auto));
        board.reset();
        trackpad_gesture(&board.inner, -12, 0);
        assert!(
            board.moved().0 > 1.0,
            "{what}: back to auto, the list chains again"
        );
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut passed = 0;
        for (kind, what) in [
            (Inner::ScrollView, "scroll_view"),
            (Inner::List, "flat_list"),
        ] {
            let board = mount_board(mtm, kind);
            regression_horizontal_pan_over_list_scrolls_the_board(&board, what);
            vertical_pan_over_list_scrolls_the_list(&board, what);
            mouse_wheel_and_shift_wheel_split_by_axis(&board, what);
            overscroll_contain_stops_chaining(&board, what);
            board.close();
            passed += 4;
        }
        println!("nested_scroll_chain: {passed} passed");
    }
}
