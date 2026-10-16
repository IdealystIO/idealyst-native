//! Smoke app for the iOS backend's boot path
//! (`backend_ios::newcore::run_in_view` → `newcore::start`) — the iOS
//! mirror of `newcore-macos-smoke`.
//!
//! Everything here is DIRECT vocabulary-builder calls — no `ui!`, no
//! `jsx!` — deliberately, so this crate proves the `runtime_scene`
//! registry-dispatch render path against real UIKit independent of the
//! macro-lowering work (the sanctioned deviation from CLAUDE.md §9.2:
//! this crate exists to gate the layer *under* the macro). The tree
//! mirrors `newcore-macos-smoke` — same coverage, third backend.
//!
//! Coverage: static + reactive `text`, `button` (event → staged write →
//! run-loop-turn driver flush), a two-way `toggle`, a structural Dyn
//! hole (closure child), a keyed list with add/remove/reverse (keyed
//! reconciliation against live UIViews), and one literal `StyleRules`
//! (the `StyleOps` delegation on the native apply-style path). Two
//! regressions ride the self-test: the root's `safe_area(TOP)` padding
//! must follow the host's real insets (they arrive after mount, once
//! the host view is in a window), and a released `scroll_view` must
//! leave the backend's `contentSize`-sync set (a stale key let the sync
//! message a recycled `UITextField` as a scroll view and abort). The
//! `image()` `src` contract (`docs/image-src.md`) rides it too: a `data:`
//! PNG and a bundled `asset!` file (which `run-sim.sh` copies into
//! `<App>.app/assets/`) must fire `on_load` 1x1; a relative path and an
//! unparseable URL must fire `on_error` (both used to leave a silent blank).
//!
//! The Swift shell lives in `host/` (AppDelegate + ViewController +
//! bridging header, mirroring the CLI-generated wrapper's
//! templates); `host/run-sim.sh` builds the staticlib, links the .app,
//! and launches it on the simulator with the self-test armed.

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::{FlexDirection, Length, SafeAreaSides, StyleRules, Tokenized};
use runtime_scene::{keyed, Element};
use runtime_vocabulary::builders::IntoSceneElement;
use runtime_vocabulary::{button, image, scroll_view, text, toggle, view};
use runtime_world::signal;

use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    /// `"<case>:load WxH"` / `"<case>:error"` from the image cases'
    /// handlers, in arrival order; the self-test compares it.
    static IMAGE_EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// A 1x1 PNG; `run-sim.sh` writes the same bytes to
/// `<App>.app/assets/smoke/dot.png` for the bundled case.
const PNG_1X1: &str = runtime_shared::image_src::vectors::PNG_1X1_BASE64;

/// What the image cases must report, sorted (sync and bundled loads land
/// at mount; no order is promised between cases).
const IMAGE_EXPECTED: &[&str] = &[
    "asset:load 1x1",
    "bad-url:error",
    "data:load 1x1",
    "relative:error",
];

/// One `image()` whose `on_load` / `on_error` append to [`IMAGE_EVENTS`].
fn image_case(case: &'static str, src: Option<&str>) -> Element {
    let b = match src {
        Some(src) => image().src(src.to_string()),
        None => image().asset(runtime_shared::asset!("smoke/dot.png")),
    };
    b.style(StyleRules { width: px(8.0), height: px(8.0), ..StyleRules::default() })
        .on_load(Rc::new(move |ev: &runtime_shared::ImageLoadEvent| {
            let line = format!("{case}:load {}x{}", ev.width, ev.height);
            IMAGE_EVENTS.with(|e| e.borrow_mut().push(line));
        }))
        .on_error(Rc::new(move || {
            IMAGE_EVENTS.with(|e| e.borrow_mut().push(format!("{case}:error")));
        }))
        .build()
}

/// The image cases, sorted: `"ok"` or what arrived instead.
fn check_images() -> String {
    let mut got = IMAGE_EVENTS.with(|e| e.borrow().clone());
    got.sort();
    if got == IMAGE_EXPECTED {
        "ok".to_string()
    } else {
        format!("{got:?}")
    }
}

/// A minimal literal style — exercises the `StyleOps` delegation (on
/// iOS: per-node `apply_style` through Taffy, not class minting).
fn padded_column() -> StyleRules {
    StyleRules {
        padding_top: Some(Tokenized::Literal(Length::Px(64.0))),
        padding_left: Some(Tokenized::Literal(Length::Px(16.0))),
        gap: Some(Tokenized::Literal(Length::Px(8.0))),
        ..StyleRules::default()
    }
}

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

/// `accessibilityIdentifier` tag so the self-test can find a view.
fn tagged(id: &str) -> AccessibilityProps {
    AccessibilityProps {
        identifier: Some(id.to_string()),
        ..AccessibilityProps::default()
    }
}

/// The reactive-anchor shapes (`Host::create_anchor` — the `display:
/// contents` node): a closure child whose branch ROOT is a keyed list.
/// The closure itself splices (this host supports it); the keyed list at
/// its root does not — it mounts under an anchor. Web renders that
/// anchor `display: contents`; a plain view in its place is a flex item
/// the author never wrote, and these two boxes are exactly where it
/// showed: a `flex_grow` child hugged to 0pt (`fill`), and a row's
/// children stacked vertically with the gap lost (`chip-2` at (0, 30)
/// instead of (58, 0)). The self-test reads the live UIView frames and
/// hit-tests through the anchor.
fn anchor_shapes() -> Element {
    view()
        .child(
            view()
                .style(StyleRules {
                    width: px(200.0),
                    height: px(120.0),
                    ..StyleRules::default()
                })
                .a11y(tagged("fill-box"))
                .child(move || {
                    keyed(
                        || vec![1u32],
                        |n| *n,
                        |_| {
                            view()
                                .style(StyleRules {
                                    flex_grow: Some(Tokenized::Literal(1.0)),
                                    ..StyleRules::default()
                                })
                                .a11y(tagged("fill"))
                                .build()
                        },
                    )
                }),
        )
        .child(
            view()
                .style(StyleRules {
                    width: px(200.0),
                    height: px(40.0),
                    flex_direction: Some(FlexDirection::Row),
                    gap: px(8.0),
                    ..StyleRules::default()
                })
                .a11y(tagged("row-box"))
                .child(move || {
                    keyed(
                        || vec![1u32, 2],
                        |n| *n,
                        |n| {
                            view()
                                .style(StyleRules {
                                    width: px(50.0),
                                    height: px(30.0),
                                    ..StyleRules::default()
                                })
                                .a11y(tagged(&format!("chip-{n}")))
                                .build()
                        },
                    )
                }),
        )
        .build()
}

/// The app tree. Runs inside `World::enter` (the boot path wraps it),
/// so the free `signal()` constructor works; these top-level signals
/// are world-root-owned and live for the app.
pub fn app() -> Element {
    let count = signal(0i32);
    let on = signal(false);
    let rows = signal(vec![1u32, 2, 3]);
    let next_row = signal(4u32);
    let show_scroll = signal(true);

    // Headless live-verification hook (`NEWCORE_SMOKE_SELFTEST=1`,
    // delivered by simctl as SIMCTL_CHILD_NEWCORE_SMOKE_SELFTEST):
    // from a real NSTimer on the real run loop, stage a write and later
    // assert it was COMMITTED by the flush driver (a staged `set` is
    // only observable through `get` after a `world.flush()` — on this
    // backend that means the apple-core post-dispatch hook fired after
    // the `after_ms` body and the queued flush drained), and that the
    // tree actually realized into live UIViews under the host root.
    // Exits 0 on success / 1 on failure so `simctl launch
    // --console-pty` callers can gate on the printed line. Scheduled
    // from inside the build (scheduler is installed by then; `after_ms`
    // uses an NSTimer, which is NOT mount-buffered, so it fires ~1.5 s
    // after the run loop starts).
    //
    // NOTE this self-test IS the dispatch-hook regression: no wrapped
    // UIKit callback fires here — if the timer body's staged write
    // commits, the hook → schedule_flush → flush route works live.
    #[cfg(target_os = "ios")]
    if std::env::var("NEWCORE_SMOKE_SELFTEST").as_deref() == Ok("1") {
        runtime_shared::scheduling::after_ms_detached(1500, move || {
            count.set(41); // stages — the driver must commit it
            // Release the scroll view (the dyn hole below swaps it for a
            // text); the second tick checks the backend forgot it.
            let scrolls_before = selftest::registered_scroll_views();
            show_scroll.set(false);
            runtime_shared::scheduling::after_ms_detached(700, move || {
                let committed = count.get() == 41;
                let views = selftest::live_view_count();
                let anchors = selftest::check_anchor_shapes();
                let safe_area = selftest::check_safe_area();
                let scrolls_after = selftest::registered_scroll_views();
                let images = check_images();
                // Exactly one scroll view mounted, then released: the
                // set must shrink by one (pre-fix it stayed at 1).
                let scroll_released = scrolls_before == 1 && scrolls_after == 0;
                let line = format!(
                    "[SMOKE-SELFTEST] committed={committed} views={views} anchors={anchors} \
                     safe_area={safe_area} scroll_views={scrolls_before}->{scrolls_after} \
                     images={images}"
                );
                // println! goes to stdout, which `simctl launch
                // --console-pty` captures; also NSLog via the installed
                // logger so `log show` has it.
                println!("{line}");
                runtime_shared::log_info!("{line}");
                // The static tree alone mounts well over 10 views
                // (column + 3 texts + 3 buttons + toggle + dyn hole +
                // 3 keyed rows); a low count means realize/finish
                // didn't attach.
                let ok = committed
                    && views > 10
                    && anchors == "ok"
                    && safe_area == "ok"
                    && scroll_released
                    && images == "ok";
                std::process::exit(if ok { 0 } else { 1 });
            });
        });
    }

    view()
        .style(padded_column())
        // The insets are zero at mount (the host view is not in a window
        // yet) and arrive afterwards; `check_safe_area` asserts the
        // padding followed them.
        .safe_area(SafeAreaSides::TOP)
        .child(
            view()
                .style(StyleRules { height: px(4.0), ..StyleRules::default() })
                .a11y(tagged("safe-probe")),
        )
        .child(text().content("New-core iOS smoke"))
        .child(text().content(move || format!("count = {}", count.get())))
        .child(
            button()
                .label("Increment")
                .on_press(move || count.update(|n| n + 1)),
        )
        .child(toggle().value(on).on_change(move |v| on.set(v)))
        // Structural Dyn hole: a closure child rebuilds when its reads
        // change (`SceneChild` lowers it to `dyn_element`).
        .child(move || {
            if on.get() {
                view()
                    .child(text().content("toggle is ON"))
                    .into_scene_element()
            } else {
                text().content("toggle is OFF").into_scene_element()
            }
        })
        .child(button().label("Add row").on_press(move || {
            let id = next_row.peek();
            next_row.set(id + 1);
            rows.update(move |r| {
                let mut r = r.clone();
                r.push(id);
                r
            });
        }))
        .child(
            button()
                .label("Remove first")
                .on_press(move || rows.update(|r| r.iter().copied().skip(1).collect())),
        )
        .child(
            button()
                .label("Reverse")
                .on_press(move || rows.update(|r| r.iter().rev().copied().collect())),
        )
        // Keyed list: rows keep identity across edits (4-pass reconcile).
        .child(keyed(
            move || rows.get(),
            |n| *n,
            |n| text().content(format!("row #{n}")).build(),
        ))
        .child(anchor_shapes())
        .child(
            view()
                .style(StyleRules {
                    flex_direction: Some(FlexDirection::Row),
                    gap: px(4.0),
                    ..StyleRules::default()
                })
                .child(image_case("data", Some(PNG_1X1)))
                .child(image_case("asset", None))
                .child(image_case("relative", Some("/attachments/abc")))
                .child(image_case("bad-url", Some("https://exa mple.com/a b.png"))),
        )
        // A scroll view the self-test releases (see `show_scroll`).
        .child(move || {
            if show_scroll.get() {
                scroll_view()
                    .style(StyleRules { height: px(40.0), ..StyleRules::default() })
                    .child(text().content("scrolling content"))
                    .into_scene_element()
            } else {
                text().content("scroll view released").into_scene_element()
            }
        })
        .build()
}

#[cfg(target_os = "ios")]
mod selftest {
    use std::cell::RefCell;

    use objc2::rc::Retained;
    use objc2_ui_kit::UIView;

    thread_local! {
        /// The Swift host's root view, stashed by `ios_main` so the
        /// self-test can count the live UIView tree without a
        /// UIApplication dependency (the framework's own views all
        /// mount under it via `set_host_root` → `finish`).
        pub(crate) static ROOT_VIEW: RefCell<Option<Retained<UIView>>> =
            const { RefCell::new(None) };
    }

    fn count_recursive(view: &UIView) -> usize {
        // SAFETY: main-thread UIKit read of the live view hierarchy
        // (NSTimer callbacks run on the main run loop).
        let subviews = unsafe { view.subviews() };
        1 + subviews.iter().map(|v| count_recursive(&v)).sum::<usize>()
    }

    /// Total UIViews under the host root (exclusive of the root
    /// itself). Runs on the main thread (NSTimer callback).
    pub fn live_view_count() -> usize {
        ROOT_VIEW.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|root| count_recursive(root) - 1)
                .unwrap_or(0)
        })
    }

    fn identifier_of(view: &UIView) -> Option<String> {
        let id: Option<Retained<objc2_foundation::NSString>> =
            unsafe { objc2::msg_send_id![view, accessibilityIdentifier] };
        id.map(|s| s.to_string())
    }

    fn find_by_identifier(view: &UIView, id: &str) -> Option<Retained<UIView>> {
        if identifier_of(view).as_deref() == Some(id) {
            return unsafe { Retained::retain(view as *const UIView as *mut UIView) };
        }
        for sub in unsafe { view.subviews() }.iter() {
            if let Some(found) = find_by_identifier(&sub, id) {
                return Some(found);
            }
        }
        None
    }

    fn frame_in(view: &UIView, ancestor: &UIView) -> objc2_foundation::CGRect {
        let bounds: objc2_foundation::CGRect = unsafe { objc2::msg_send![view, bounds] };
        unsafe { objc2::msg_send![view, convertRect: bounds, toView: ancestor] }
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.5
    }

    /// UIKit's `UIEdgeInsets` (objc2-ui-kit 0.2 doesn't ship it).
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct UIEdgeInsets {
        top: f64,
        left: f64,
        bottom: f64,
        right: f64,
    }

    unsafe impl objc2::encode::Encode for UIEdgeInsets {
        const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
            "UIEdgeInsets",
            &[f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING],
        );
    }

    /// Scroll views the backend's `contentSize` sync still walks.
    pub fn registered_scroll_views() -> usize {
        backend_ios::with_backend(|b| b.registered_scroll_view_count()).unwrap_or(usize::MAX)
    }

    /// The root's `safe_area(TOP)` padding tracks the host's live insets,
    /// and the world's `safe_area_insets()` carries the same value:
    /// `"ok"`, or the first failing check.
    pub fn check_safe_area() -> String {
        ROOT_VIEW.with(|slot| {
            let slot = slot.borrow();
            let Some(root) = slot.as_ref() else {
                return "no-root".to_string();
            };
            let host: UIEdgeInsets = unsafe { objc2::msg_send![&**root, safeAreaInsets] };
            // Every simulator this runs on has a status bar; zero here
            // means the host never got real insets and the check below
            // would pass vacuously.
            if host.top <= 0.0 {
                return format!("host top inset is {}", host.top);
            }
            // NSTimer callbacks run outside `World::enter`; read through
            // the mounted world.
            let Some(world) = backend_ios::newcore::mounted_world() else {
                return "no-world".to_string();
            };
            let world_top =
                world.enter(|| runtime_vocabulary::safe_area::safe_area_insets().peek().top);
            if !near(world_top as f64, host.top) {
                return format!("world inset {world_top} != host inset {}", host.top);
            }
            let Some(probe) = find_by_identifier(root, "safe-probe") else {
                return "probe-missing".to_string();
            };
            // padded_column's 64pt top padding plus the safe-area inset.
            let y = frame_in(&probe, root).origin.y;
            if !near(y, host.top + 64.0) {
                return format!("probe at y={y}, want {}", host.top + 64.0);
            }
            "ok".to_string()
        })
    }

    /// Verify the reactive-anchor shapes in `anchor_shapes` laid out as
    /// web does (the anchor is `display: contents`), and that the anchor
    /// is hit-transparent: `"ok"`, or the first failing check.
    pub fn check_anchor_shapes() -> String {
        ROOT_VIEW.with(|slot| {
            let slot = slot.borrow();
            let Some(root) = slot.as_ref() else {
                return "no-root".to_string();
            };
            let find = |id: &str| find_by_identifier(root, id);
            let (Some(fill_box), Some(fill), Some(row_box), Some(chip1), Some(chip2)) = (
                find("fill-box"),
                find("fill"),
                find("row-box"),
                find("chip-1"),
                find("chip-2"),
            ) else {
                return "views-missing".to_string();
            };
            // A `flex_grow: 1` child under the anchor fills the 120pt
            // box (a plain-view anchor hugged it to 0).
            let f = frame_in(&fill, &fill_box);
            if !near(f.size.height, 120.0) || !near(f.origin.y, 0.0) {
                return format!("fill: {}x{} at y={}", f.size.width, f.size.height, f.origin.y);
            }
            // Two 50pt chips under the anchor sit in the ROW with the
            // 8pt gap (a plain-view anchor stacked them in a column).
            let c1 = frame_in(&chip1, &row_box);
            let c2 = frame_in(&chip2, &row_box);
            if !near(c1.origin.x, 0.0) || !near(c2.origin.x, 58.0) || !near(c2.origin.y, 0.0) {
                return format!("chips: chip-1 at ({},{}) chip-2 at ({},{})", c1.origin.x, c1.origin.y, c2.origin.x, c2.origin.y);
            }
            // Hit-testing through the anchor: the point at chip-2's
            // center resolves to chip-2, not to the anchor view.
            let center = objc2_foundation::CGPoint {
                x: c2.origin.x + c2.size.width / 2.0,
                y: c2.origin.y + c2.size.height / 2.0,
            };
            let hit: Option<Retained<UIView>> =
                unsafe { objc2::msg_send_id![&*row_box, hitTest: center, withEvent: std::ptr::null::<objc2::runtime::AnyObject>()] };
            match hit.as_deref().and_then(identifier_of) {
                Some(id) if id == "chip-2" => {}
                other => return format!("hit-test: {other:?}"),
            }
            "ok".to_string()
        })
    }
}

// ===========================================================================
// C entry points for the Swift shell (host/). Same ABI as the
// CLI-generated wrapper (`crates/tools/build/ios`)
// so the checked-in Swift sources mirror the shipped templates.
// ===========================================================================

#[cfg(target_os = "ios")]
mod entry {
    use std::cell::RefCell;
    use std::ffi::c_void;

    use backend_ios::newcore::NewCoreApp;

    thread_local! {
        /// The mounted app must outlive `ios_main` returning (dropping it
        /// unmounts); `ios_teardown` takes it.
        static APP: RefCell<Option<NewCoreApp>> = const { RefCell::new(None) };
    }

    /// C-exported entry point called by the Swift host from
    /// `viewDidLoad`.
    ///
    /// # Safety
    /// - Must be invoked on the main thread.
    /// - `root_view` must be a non-null, valid `UIView *`.
    #[no_mangle]
    pub unsafe extern "C" fn ios_main(root_view: *mut c_void) {
        std::panic::set_hook(Box::new(|info| {
            eprintln!("RUST PANIC: {info}");
        }));

        // Stash the root view for the self-test's live view count.
        let view = unsafe {
            objc2::rc::Retained::retain(root_view as *mut objc2_ui_kit::UIView)
                .expect("ios_main: root_view must be non-null")
        };
        super::selftest::ROOT_VIEW.with(|slot| *slot.borrow_mut() = Some(view));

        APP.with(|slot| slot.borrow_mut().take());
        // The whole boot — scheduler/logger installs, backend + host
        // root wiring, mount-buffering brackets, realize + finish +
        // flush-driver install — lives in `run_in_view` (the iOS
        // counterpart of `host_appkit::newcore::run`).
        let app = unsafe { backend_ios::newcore::run_in_view(root_view, |_| {}, super::app) };
        APP.with(|slot| *slot.borrow_mut() = Some(app));
    }

    /// Tear down the active mount. Idempotent — a no-op if nothing is
    /// mounted.
    #[no_mangle]
    pub unsafe extern "C" fn ios_teardown() {
        if let Some(app) = APP.with(|slot| slot.borrow_mut().take()) {
            app.stop();
        }
    }
}
