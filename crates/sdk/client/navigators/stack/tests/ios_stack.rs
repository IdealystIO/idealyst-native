//! The stack on a real `UINavigationController`.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator and
//! run the binary there:
//!
//! ```sh
//! cargo test -p stack-navigator --target aarch64-apple-ios-sim --test ios_stack --no-run
//! xcrun simctl spawn booted target/aarch64-apple-ios-sim/debug/deps/ios_stack-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::cell::Cell;
    use std::rc::Rc;

    use idea_theme::theme::{install_idea_theme, light_theme};
    use idea_ui::components::table::{
        ColumnId, ColumnPin, RowDrag, RowId, Table, TableColumn, TableInset, TableVariant, VirtualRows,
    };
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSString};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_core::{signal, ui, Element, IntoElement, Length, Ref, Signal, Tokenized};
    use stack_navigator::{Route, Screen, StackBuilder, StackHandle, StackNavigator};

    const HOME: Route<()> = Route::new("home", "/");
    const LIST: Route<()> = Route::new("list", "/list");
    const TASK: Route<()> = Route::new("task", "/task");
    const NEW: Route<()> = Route::new("new", "/list/new");

    fn spin(seconds: f64) {
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            static kCFRunLoopDefaultMode: *const std::ffi::c_void;
            fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, ret: u8) -> i32;
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds);
        while std::time::Instant::now() < until {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.02, 0) };
        }
    }

    fn fill() -> runtime_core::StyleRules {
        runtime_core::StyleRules {
            flex_grow: Some(Tokenized::Literal(1.0)),
            flex_basis: Some(Tokenized::Literal(Length::Px(0.0))),
            min_height: Some(Tokenized::Literal(Length::Px(0.0))),
            ..Default::default()
        }
    }

    thread_local! {
        static FOLDED: Cell<Option<Signal<bool>>> = const { Cell::new(None) };
    }

    struct App {
        app: Option<backend_ios::newcore::NewCoreApp>,
        window: Retained<UIWindow>,
        nav: Ref<StackHandle>,
        rows: Signal<Vec<String>>,
    }

    impl Drop for App {
        fn drop(&mut self) {
            if let Some(app) = self.app.take() {
                app.stop();
            }
        }
    }

    /// A table as an app's list has it: reordered by dragging its rows, in a
    /// reactive branch, over a memo.
    fn list_screen(rows: Signal<Vec<String>>) -> Element {
        let rows = runtime_core::memo(move || rows.get());
        let row_id = move |i: usize| rows.with(|r| r.get(i).cloned()).unwrap_or_default();
        let ctx: dnd::DragContext<String> = dnd::DragContext::new();
        let fields = signal(1u32);
        let reorder: Rc<dyn Fn(usize, usize)> = Rc::new(|_, _| {});
        // Folded away to no height while something else shows in its place.
        let folded = signal(false);
        FOLDED.with(|f| f.set(Some(folded)));
        let seat = move || {
            let mut s = fill();
            if folded.get() {
                s.flex_grow = Some(Tokenized::Literal(0.0));
                s.flex_basis = None;
                s.height = Some(Tokenized::Literal(Length::Px(0.0)));
                s.overflow = Some(runtime_core::Overflow::Hidden);
            }
            s
        };
        ui! {
            view(style = fill()) {
                view(style = seat) {
                    match fields.get() {
                        _ => {
                            Table(
                                variant = TableVariant::Plain,
                                inset_x = TableInset::Xs,
                                scroll_y = true,
                                scroll_x = true,
                                columns = vec![
                                    TableColumn::new("name").label("Name").fill().min_width(240.0).pinned(ColumnPin::Left),
                                    TableColumn::new("when").label("When").width(140.0),
                                    TableColumn::actions("menu"),
                                ],
                                on_reorder = Some(reorder.clone()),
                                drag_from_row = true,
                                row_drag = Some(RowDrag::new(&ctx, row_id)),
                                virtualized = Some(
                                    VirtualRows::new(
                                        move || rows.with(|r| r.len()),
                                        move |i| RowId::from(row_id(i)),
                                        move |i, _: &ColumnId| {
                                            let label = row_id(i);
                                            ui! { text { label } }
                                        },
                                    )
                                    .row_height(44.0)
                                    .on_end_reached(400.0, || {}),
                                ),
                            )
                        }
                    }
                }
            }
        }
    }

    fn mount(mtm: MainThreadMarker) -> App {
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 800.0 } };
        let window: Retained<UIWindow> = unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame] };
        let _: () = unsafe { msg_send![&window, setHidden: false] };
        let root: Retained<UIView> = unsafe { msg_send_id![mtm.alloc::<UIView>(), initWithFrame: frame] };
        unsafe { window.addSubview(&root) };
        let nav: Ref<StackHandle> = Ref::new();
        let slot = Rc::new(Cell::new(None));
        let out = slot.clone();
        let app = unsafe {
            backend_ios::newcore::run_in_view(
                Retained::as_ptr(&root) as *mut std::ffi::c_void,
                |_| {},
                move || {
                    install_idea_theme(light_theme());
                    let rows = signal((0..6).map(|i| format!("row {i}")).collect::<Vec<String>>());
                    out.set(Some(rows));
                    StackNavigator::new(&HOME)
                        .screen(HOME, |_| Screen::new(ui! { view { text { "home screen" } } }))
                        .screen(LIST, move |_| Screen::new(list_screen(rows)))
                        .screen(NEW, |_| Screen::new(ui! { view { text { "new screen" } } }))
                        .screen(TASK, |_| Screen::new(ui! { view { text { "task screen" } } }))
                        .bind(nav)
                        .into_element()
                },
            )
        };
        let app = App { app: Some(app), window, nav, rows: slot.take().unwrap() };
        spin(0.3);
        app
    }

    impl App {
        fn act(&self, f: impl FnOnce(&StackHandle)) {
            self.nav.with(|n| f(n));
            backend_ios::newcore::schedule_flush();
        }

        /// The screens on show, each by a text of its own.
        fn shows(&self) -> Vec<String> {
            self.labels().into_iter().filter(|l| l.ends_with("screen") || l == "NAME").collect()
        }

        fn rows_shown(&self) -> usize {
            self.labels().into_iter().filter(|l| l.starts_with("row")).collect::<std::collections::BTreeSet<_>>().len()
        }

        /// The swipe-back recognizer and its delegate.
        fn swipe(&self) -> (*mut AnyObject, *mut AnyObject) {
            fn nav_of(v: &UIView) -> Option<*mut AnyObject> {
                let next: *mut AnyObject = unsafe { msg_send![v, nextResponder] };
                if !next.is_null() {
                    let is: bool = unsafe { msg_send![next, isKindOfClass: objc2::class!(UINavigationController)] };
                    if is {
                        return Some(next);
                    }
                }
                unsafe { v.subviews() }.iter().find_map(|c| nav_of(c))
            }
            let nav = nav_of(&self.window).expect("the stack's navigation controller");
            unsafe {
                let recognizer: *mut AnyObject = msg_send![nav, interactivePopGestureRecognizer];
                (recognizer, msg_send![recognizer, delegate])
            }
        }

        fn swipe_delegate(&self) -> &'static str {
            unsafe { (*self.swipe().1).class().name() }
        }

        /// What the delegate answers.
        fn swipe_may_begin(&self) -> bool {
            let (recognizer, delegate) = self.swipe();
            unsafe { msg_send![delegate, gestureRecognizerShouldBegin: recognizer] }
        }

        fn add_row(&self, label: &str) {
            let mut rows = self.rows.peek();
            rows.push(label.to_string());
            self.rows.set(rows);
            backend_ios::newcore::schedule_flush();
        }

        /// Every label on screen: in the window, with a box.
        fn labels(&self) -> Vec<String> {
            fn walk(v: &UIView, out: &mut Vec<String>) {
                let hidden: bool = unsafe { msg_send![v, isHidden] };
                if hidden {
                    return;
                }
                let is_label: bool = unsafe { msg_send![v, isKindOfClass: objc2::class!(UILabel)] };
                if is_label {
                    let t: Option<Retained<NSString>> = unsafe { msg_send_id![v, text] };
                    let b: CGRect = unsafe { msg_send![v, bounds] };
                    if b.size.width > 0.0 && b.size.height > 0.0 {
                        out.extend(t.map(|t| t.to_string()));
                    }
                }
                for c in unsafe { v.subviews() }.iter() {
                    walk(&c, out);
                }
            }
            let mut out = Vec::new();
            walk(&self.window, &mut out);
            out
        }
    }

    /// REGRESSION: UIKit drops a move made in the turn of a `reset`, and can
    /// drop one made during a slide, so the screen on show was not the
    /// navigator's. Every order of moves, in one turn or mid-slide, ends on
    /// the navigator's screen, with the right ones below it.
    fn regression_back_to_back_moves_end_on_the_navigators_screen(mtm: MainThreadMarker) {
        // Each move, and how long after it the next comes.
        type Step = (&'static str, f64);
        // The moves, what they end on, and what one pop shows.
        let cases: &[(&[Step], &str, &str)] = &[
            (&[("reset", 0.0), ("list", 0.0), ("new", 0.0)], "new screen", "NAME"),
            (&[("reset", 0.0), ("list", 0.0)], "NAME", "home screen"),
            (&[("list", 0.9), ("reset", 0.0), ("list", 0.0), ("new", 0.0)], "new screen", "NAME"),
            (&[("list", 0.0), ("new", 0.0)], "new screen", "NAME"),
            (&[("list", 0.1), ("new", 0.1), ("task", 0.0)], "task screen", "new screen"),
            (&[("list", 0.1), ("pop", 0.0)], "home screen", "home screen"),
            (&[("list", 0.9), ("pop", 0.1), ("new", 0.0)], "new screen", "home screen"),
            (&[("list", 0.9), ("new", 0.9), ("pop", 0.1), ("pop", 0.0)], "home screen", "home screen"),
            (&[("list", 0.1), ("replace_new", 0.0)], "new screen", "home screen"),
            (&[("list", 0.9), ("new", 0.1), ("reset", 0.1), ("task", 0.0)], "task screen", "home screen"),
            (&[("list", 0.1), ("pop", 0.1), ("list", 0.1), ("pop", 0.1), ("new", 0.0)], "new screen", "home screen"),
        ];
        for (steps, ends_on, then) in cases {
            let a = mount(mtm);
            for (what, gap) in *steps {
                a.act(|n| match *what {
                    "reset" => n.reset(&HOME, ()),
                    "list" => n.push(&LIST, ()),
                    "new" => n.push(&NEW, ()),
                    "task" => n.push(&TASK, ()),
                    "replace_new" => n.replace(&NEW, ()),
                    "pop" => n.pop(),
                    _ => unreachable!(),
                });
                if *gap > 0.0 {
                    spin(*gap);
                }
            }
            spin(1.2);
            assert_eq!(a.shows(), [ends_on.to_string()], "{steps:?}");
            a.act(|n| n.pop());
            spin(0.9);
            assert_eq!(a.shows(), [then.to_string()], "{steps:?}, then pop");
        }
    }

    /// A row added while the list is covered is there on the way back, also
    /// when the screen on top adds it and leaves in one turn.
    fn a_row_added_under_a_pushed_screen_shows_on_the_way_back(mtm: MainThreadMarker) {
        for together in [true, false] {
            let a = mount(mtm);
            a.act(|n| n.push(&LIST, ()));
            spin(0.9);
            a.act(|n| n.push(&NEW, ()));
            spin(0.9);
            a.add_row("row 6");
            if !together {
                spin(0.3);
            }
            a.act(|n| n.pop());
            spin(0.9);
            assert_eq!(a.rows_shown(), 7, "together: {together}");
        }
    }

    /// REGRESSION: a table that changed and grew in one turn aborted the app
    /// ("RefCell already borrowed"): the layout pass resizes the grid's
    /// scroller, UIKit reports a scroll from inside it, and the grid mounted
    /// its new cells under the pass's borrow.
    fn regression_a_table_unfolded_with_a_new_row_shows_it(mtm: MainThreadMarker) {
        let a = mount(mtm);
        a.act(|n| n.push(&LIST, ()));
        spin(0.9);
        let folded = FOLDED.with(|f| f.get()).expect("the list is mounted");
        folded.set(true);
        backend_ios::newcore::schedule_flush();
        spin(0.3);
        a.add_row("row 6");
        folded.set(false);
        spin(0.5);
        assert_eq!(a.rows_shown(), 7);
    }

    /// REGRESSION: the swipe back was left to UIKit's own delegate, which
    /// refuses it while the navigation bar is hidden. That refusal needs a
    /// real touch, which a test binary cannot make: this pins that the
    /// recognizer asks OUR delegate, and what it answers.
    fn regression_the_swipe_back_begins_on_a_pushed_screen_only(mtm: MainThreadMarker) {
        let a = mount(mtm);
        assert_eq!(a.swipe_delegate(), "IdealystStackPresenterDelegate");
        assert!(!a.swipe_may_begin(), "nothing to go back to");
        a.act(|n| n.push(&NEW, ()));
        spin(0.9);
        assert!(a.swipe_may_begin());
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        regression_back_to_back_moves_end_on_the_navigators_screen(mtm);
        a_row_added_under_a_pushed_screen_shows_on_the_way_back(mtm);
        regression_a_table_unfolded_with_a_new_row_shows_it(mtm);
        regression_the_swipe_back_begins_on_a_pushed_screen_only(mtm);
        println!("ios_stack: 4 passed");
    }
}
