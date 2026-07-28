//! Smoke app for the macOS backend's new-core boot path (P4a).
//!
//! Everything here is DIRECT vocabulary-builder calls — no `ui!`, no
//! `jsx!` — deliberately, so this crate proves the `runtime_scene`
//! registry-dispatch render path against real AppKit independent of the
//! parallel P3a macro-lowering work (the sanctioned deviation from
//! CLAUDE.md §9.2: the macro can't target the new core yet, and this
//! crate exists to gate the layer *under* the macro). The tree mirrors
//! `newcore-web-smoke` — same coverage, second backend.
//!
//! Coverage: static + reactive `text`, `button` (event → staged write →
//! dispatch-site wrapper flush), a two-way `toggle`, a structural Dyn
//! hole (closure child), a keyed list with add/remove/reverse (keyed
//! reconciliation against live NSViews), one literal `StyleRules` (the
//! `StyleOps` delegation on the native apply-style path), and a
//! `scroll_view` with an `on_scroll` author callback (the dispatch-site
//! glue's tracking-loop proof surface — see the self-test below).

use runtime_core::{Length, StyleRules, Tokenized};
use runtime_scene::{keyed, Element};
use runtime_vocabulary::builders::IntoSceneElement;
use runtime_vocabulary::{button, scroll_view, text, toggle, view};
use runtime_world::signal;

/// A minimal literal style — exercises the `StyleOps` delegation (on
/// macOS: per-node `apply_style` through Taffy, not class minting).
fn padded_column() -> StyleRules {
    StyleRules {
        padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
        padding_left: Some(Tokenized::Literal(Length::Px(16.0))),
        gap: Some(Tokenized::Literal(Length::Px(8.0))),
        ..StyleRules::default()
    }
}

/// Fixed-height scroll region (content below is much taller), so a
/// programmatic `scroll_to` produces a real offset change and a real
/// `on_scroll` delivery.
fn scroll_region() -> StyleRules {
    StyleRules {
        height: Some(Tokenized::Literal(Length::Px(120.0))),
        ..StyleRules::default()
    }
}

/// The app tree. Runs inside `World::enter` (the boot path wraps it), so
/// the free `signal()` constructor works; these top-level signals are
/// world-root-owned and live for the app.
pub fn app() -> Element {
    let count = signal(0i32);
    let on = signal(false);
    let rows = signal(vec![1u32, 2, 3]);
    let next_row = signal(4u32);
    let scroll_y = signal(0.0f32);

    // Headless live-verification hook (`NEWCORE_SMOKE_SELFTEST=1`), two
    // phases, from real NSTimers on the real run loop:
    //
    // 1. **Hook-route commit** — stage a write from an `after_ms` body
    //    and later assert it was COMMITTED (a staged `set` is only
    //    observable through `get` after a `world.flush()`). With the
    //    P4a monitor+timer gone, the ONLY route that can commit this is
    //    the apple-core post-dispatch hook (`after_ms_inner` fires it
    //    after the body) → `schedule_flush` → main-queue microtask —
    //    i.e. this phase is the live proof of the settled flush driver,
    //    exactly like the iOS smoke's self-test. Also asserts the tree
    //    realized into live NSViews under the key window.
    // 2. **Press-wrapper commit** — drive the real NSButton
    //    target-action path (`performClick:` on the live "Increment"
    //    button) and assert the staged `count.update` committed. The
    //    removed NSEvent monitor used to cover exactly this event
    //    family; now the dispatch-site `on_click` wrapper must
    //    schedule the flush itself.
    // 3. **Tracking-loop scroll commit** (the P4a-removal crux) — prove
    //    a signal write staged by the `on_scroll` author callback
    //    commits WHILE a nested `NSEventTrackingRunLoopMode` run-loop
    //    turn (the run-loop state an AppKit control tracking loop /
    //    scroll drag creates) is still running. See
    //    `selftest::run_tracking_loop_scroll_test`.
    //
    // Exits 0 on success / 1 on failure so a CI-ish caller can gate.
    #[cfg(target_os = "macos")]
    if std::env::var("NEWCORE_SMOKE_SELFTEST").as_deref() == Ok("1") {
        // Commit observer for phase 3: an effect (world-root-owned —
        // created during the build, inside `world.enter`) that records
        // the run-loop mode at the moment the scroll write COMMITS.
        // Effects run inside `world.flush()`, so this fires exactly
        // when the flush driver commits — the observable we need.
        runtime_world::effect(move || {
            let y = scroll_y.get();
            if y > 0.0 {
                selftest::record_scroll_commit(y);
            }
        });

        runtime_core::scheduling::after_ms_detached(1500, move || {
            count.set(41); // stages — the dispatch hook must commit it
            runtime_core::scheduling::after_ms_detached(700, move || {
                let committed = count.get() == 41;
                let views = selftest::live_view_count();
                // Phase 2: real target-action press through the
                // wrapped Action (stages 41 → 42; the wrapper's
                // scheduled flush must commit it by the next check).
                let pressed = selftest::press_button("Increment");
                runtime_core::scheduling::after_ms_detached(400, move || {
                    let press_committed = count.get() == 42;
                    let tracking = selftest::run_tracking_loop_scroll_test();
                    let ok = committed
                        && views > 10
                        && pressed
                        && press_committed
                        && tracking.committed_during_tracking
                        && tracking.mode_at_commit == "NSEventTrackingRunLoopMode";
                    println!(
                        "[SMOKE-SELFTEST] committed={committed} views={views} \
                         press_commit={press_committed} tracking_commit={} \
                         mode_at_commit={} scroll_y={} verdict={}",
                        tracking.committed_during_tracking,
                        tracking.mode_at_commit,
                        tracking.scroll_y,
                        if ok { "PASS" } else { "FAIL" }
                    );
                    // The static tree alone mounts well over 10 views
                    // (column + texts + buttons + toggle + dyn hole +
                    // scroll view + keyed rows); a low count means
                    // realize/finish didn't attach.
                    std::process::exit(if ok { 0 } else { 1 });
                });
            });
        });
    }

    view()
        .style(padded_column())
        .child(text().content("New-core macOS smoke"))
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
        // Scroll surface: reactive readout + a fixed-height scroll_view
        // whose on_scroll writes a signal — the author-callback shape
        // the dispatch-site glue wraps, and the tracking-loop test's
        // trigger (the self-test drives it via the captured handle).
        .child(text().content(move || format!("scroll y = {:.0}", scroll_y.get())))
        .child(
            scroll_view()
                .style(scroll_region())
                .on_scroll(move |_x, y| scroll_y.set(y))
                .on_handle(|h| selftest::store_scroll_handle(h))
                .children(
                    (1..=30)
                        .map(|n| text().content(format!("scroll line {n}")).build())
                        .collect(),
                ),
        )
        .build()
}

#[cfg(target_os = "macos")]
mod selftest {
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;

    use objc2_app_kit::{NSApplication, NSView};
    use objc2_foundation::{MainThreadMarker, NSString};
    use runtime_core::primitives::scroll_view::ScrollViewHandle;

    // ── CoreFoundation run-loop FFI ─────────────────────────────────
    // `CFRunLoopRunInMode` is how AppKit itself runs a control
    // tracking loop's nested turns (mode = NSEventTrackingRunLoopMode,
    // toll-free-bridged NSString ↔ CFString). No objc2 wrapper exists
    // for these; the raw C API is the honest surface.
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRunLoopRunInMode(
            mode: *const c_void,
            seconds: f64,
            return_after_source_handled: u8,
        ) -> i32;
        fn CFRunLoopCopyCurrentMode(rl: *const c_void) -> *const c_void;
        fn CFRunLoopGetCurrent() -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }

    thread_local! {
        static SCROLL_HANDLE: RefCell<Option<ScrollViewHandle>> = const { RefCell::new(None) };
        /// True while the self-test is inside its nested
        /// tracking-mode run — the window in which the commit must
        /// land for the tracking-loop proof to pass.
        static IN_TRACKING_RUN: Cell<bool> = const { Cell::new(false) };
        static COMMIT_SEEN: Cell<bool> = const { Cell::new(false) };
        static COMMIT_DURING_TRACKING: Cell<bool> = const { Cell::new(false) };
        static COMMIT_MODE: RefCell<String> = const { RefCell::new(String::new()) };
        static COMMIT_SCROLL_Y: Cell<f32> = const { Cell::new(0.0) };
    }

    /// Ref-fill from the scroll_view builder (runs at mount).
    pub fn store_scroll_handle(h: ScrollViewHandle) {
        SCROLL_HANDLE.with(|s| *s.borrow_mut() = Some(h));
    }

    /// Called from the world effect observing `scroll_y` — i.e. from
    /// INSIDE `world.flush()`, at the exact moment the staged
    /// `on_scroll` write commits. Records the run-loop mode CF reports
    /// at that instant and whether the nested tracking run was still
    /// active.
    pub fn record_scroll_commit(y: f32) {
        if COMMIT_SEEN.with(|c| c.replace(true)) {
            return; // first commit is the evidence; later ones are noise
        }
        COMMIT_SCROLL_Y.with(|c| c.set(y));
        COMMIT_DURING_TRACKING.with(|c| c.set(IN_TRACKING_RUN.with(|t| t.get())));
        let mode = unsafe {
            let cf = CFRunLoopCopyCurrentMode(CFRunLoopGetCurrent());
            if cf.is_null() {
                String::from("<none>")
            } else {
                // Toll-free bridge: CFStringRef == NSString.
                let s = (*(cf as *const NSString)).to_string();
                CFRelease(cf);
                s
            }
        };
        COMMIT_MODE.with(|m| *m.borrow_mut() = mode);
    }

    pub struct TrackingResult {
        pub committed_during_tracking: bool,
        pub mode_at_commit: String,
        pub scroll_y: f32,
    }

    /// The tracking-loop proof (module docs, phase 2).
    ///
    /// Why this shape: an AppKit control tracking loop (scroller-knob
    /// drag, NSButton/NSSlider press, menu tracking) is a nested
    /// `CFRunLoopRunInMode(NSEventTrackingRunLoopMode, …)` turn driven
    /// from event dispatch — the run-loop state that (a) bypassed
    /// NSEvent local monitors (the reason the removed P4a driver
    /// carried a timer safety net) and (b) pauses default-mode
    /// sources, the classic way a scheduled "later" callback silently
    /// stalls until the drag ends. This test reproduces that exact
    /// run-loop state deterministically (real momentum-phase wheel
    /// events can't be synthesized without posting through the window
    /// server, which needs accessibility grants a headless self-test
    /// can't assume) and asserts the settled flush driver commits
    /// INSIDE it:
    ///
    /// 1. `scroll_to` moves the clip view; AppKit posts the bounds
    ///    change synchronously and the backend queues the author
    ///    `on_scroll` delivery as ONE microtask (the Apple on_scroll
    ///    async invariant — `dispatch_async(main_queue)`).
    /// 2. We then enter the nested tracking-mode run. For the commit
    ///    to land inside it, libdispatch must drain the main queue
    ///    during that mode: first the delivery microtask (author
    ///    callback runs DURING the tracking loop and stages the write
    ///    + schedules the flush via the dispatch-site wrapper), then
    ///    the flush microtask (`world.flush()` → the observing effect
    ///    → `record_scroll_commit`, which snapshots
    ///    `CFRunLoopCopyCurrentMode`).
    /// 3. Success = the commit was recorded while `IN_TRACKING_RUN`
    ///    was set AND CF reported `NSEventTrackingRunLoopMode` at
    ///    commit time. This holds because the main GCD queue is a
    ///    common-modes run-loop source and the tracking mode is a
    ///    common mode; if that ever regressed, this test fails and
    ///    the flush must be rescheduled the way apple-core schedules
    ///    common-modes timers (NOT by resurrecting the monitor).
    pub fn run_tracking_loop_scroll_test() -> TrackingResult {
        let handle = SCROLL_HANDLE.with(|s| s.borrow().clone());
        let Some(handle) = handle else {
            return TrackingResult {
                committed_during_tracking: false,
                mode_at_commit: String::from("<no scroll handle>"),
                scroll_y: 0.0,
            };
        };

        // Queue the on_scroll delivery (async microtask) BEFORE the
        // nested run: the callback must then execute inside it.
        handle.scroll_to(0.0, 60.0);

        // A real tracking loop's mode is kept non-empty by AppKit's
        // event source. This synthetic run needs its own source or
        // CFRunLoopRunInMode returns immediately (kCFRunLoopRunFinished
        // — nothing to run); apple-core's `after_ms` NSTimers are added
        // in COMMON modes, so one pending no-op timer keeps the
        // tracking mode alive for the whole window.
        runtime_core::scheduling::after_ms_detached(600, || {});

        IN_TRACKING_RUN.with(|t| t.set(true));
        let tracking_mode = NSString::from_str("NSEventTrackingRunLoopMode");
        unsafe {
            // returnAfterSourceHandled=false: run the full window (the
            // commit is expected well before the 0.7 s deadline).
            CFRunLoopRunInMode(
                &*tracking_mode as *const NSString as *const c_void,
                0.7,
                0,
            );
        }
        IN_TRACKING_RUN.with(|t| t.set(false));

        TrackingResult {
            committed_during_tracking: COMMIT_DURING_TRACKING.with(|c| c.get()),
            mode_at_commit: COMMIT_MODE.with(|m| m.borrow().clone()),
            scroll_y: COMMIT_SCROLL_Y.with(|c| c.get()),
        }
    }

    /// Find the live NSButton titled `title` under the key window and
    /// `performClick:` it — the REAL AppKit target-action dispatch
    /// (the same path a user click takes past event routing), which
    /// invokes the dispatch-site-wrapped `Action::fire`. Returns false
    /// if no such button is in the hierarchy.
    pub fn press_button(title: &str) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let app = NSApplication::sharedApplication(mtm);
        let windows = app.windows();
        let Some(window) = windows.iter().next() else {
            return false;
        };
        let Some(content) = window.contentView() else {
            return false;
        };
        fn find_and_click(view: &NSView, want: &str) -> bool {
            let is_button: bool = unsafe {
                objc2::msg_send![view, isKindOfClass: objc2::class!(NSButton)]
            };
            if is_button {
                let ns_title: Option<objc2::rc::Retained<NSString>> =
                    unsafe { objc2::msg_send_id![view, title] };
                if ns_title.map(|t| t.to_string()).as_deref() == Some(want) {
                    let _: () = unsafe {
                        objc2::msg_send![view, performClick: std::ptr::null::<NSView>()]
                    };
                    return true;
                }
            }
            let subviews = unsafe { view.subviews() };
            subviews.iter().any(|v| find_and_click(&v, want))
        }
        find_and_click(&content, title)
    }

    fn count_recursive(view: &NSView) -> usize {
        // SAFETY: main-thread AppKit read of the live view hierarchy
        // (NSTimer callbacks run on the main run loop).
        let subviews = unsafe { view.subviews() };
        1 + subviews
            .iter()
            .map(|v| count_recursive(v))
            .sum::<usize>()
    }

    /// Total NSViews under the app's window content view. Runs on the
    /// main thread (NSTimer callback).
    pub fn live_view_count() -> usize {
        let Some(mtm) = MainThreadMarker::new() else {
            return 0;
        };
        let app = NSApplication::sharedApplication(mtm);
        let windows = app.windows();
        let Some(window) = windows.iter().next() else {
            return 0;
        };
        window
            .contentView()
            .map(|v| count_recursive(&v))
            .unwrap_or(0)
    }
}

#[cfg(not(target_os = "macos"))]
mod selftest {
    use runtime_core::primitives::scroll_view::ScrollViewHandle;

    /// Cross-host stub (the builder's `on_handle` closure must compile
    /// everywhere; the AppKit self-test only exists on macOS).
    pub fn store_scroll_handle(_h: ScrollViewHandle) {}
}

#[cfg(target_os = "macos")]
fn main() {
    let opts = host_appkit::RunOptions {
        title: "New-core macOS smoke".to_string(),
        width: 480.0,
        height: 520.0,
    };
    if let Err(e) = host_appkit::newcore::run(app, opts) {
        eprintln!("[newcore-macos-smoke] failed to boot: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    // Cross-host stub so workspace-wide `cargo check` passes anywhere;
    // the AppKit run loop only exists on macOS.
    eprintln!("newcore-macos-smoke only runs on macOS");
}
