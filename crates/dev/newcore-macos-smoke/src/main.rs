//! Smoke app for the macOS backend's boot path
//! (`host_appkit::newcore::run` → `backend_macos::newcore::start`).
//!
//! Everything here is DIRECT vocabulary-builder calls — no `ui!`, no
//! `jsx!` — deliberately, so this crate proves the `runtime_scene`
//! registry-dispatch render path against real AppKit independent of the
//! macro-lowering layer (the sanctioned deviation from CLAUDE.md §9.2:
//! this crate exists to gate the layer *under* the macro). The tree mirrors
//! `newcore-web-smoke` — same coverage, second backend.
//!
//! Coverage: static + reactive `text`, `button` (event → staged write →
//! dispatch-site wrapper flush), a two-way `toggle`, a structural Dyn
//! hole (closure child), a keyed list with add/remove/reverse (keyed
//! reconciliation against live NSViews), one literal `StyleRules` (the
//! `StyleOps` delegation on the native apply-style path), a
//! `scroll_view` with an `on_scroll` author callback (the dispatch-site
//! glue's tracking-loop proof surface — see the self-test below), and a
//! breakpoint-reactive text fed by the live viewport source (self-test
//! phase 4 resizes the real NSWindow across the Xl threshold and
//! asserts the bucket + the on-screen NSTextField followed).

use runtime_shared::{Length, StyleRules, Tokenized};
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
    // Live-viewport surface (self-test phase 4): the per-world
    // breakpoint bucket, captured at the sanctioned position (inside
    // the build — the ctx's memo pins the breakpoint table here). The
    // reactive text below re-renders when a window resize crosses a
    // bucket boundary: AppKit `setFrameSize:` → both viewport sinks →
    // the world ctx → this memo → the f-string closure.
    let bp = runtime_vocabulary::viewport::viewport_ctx().breakpoint();

    // Headless live-verification hook (`NEWCORE_SMOKE_SELFTEST=1`),
    // four phases, from real NSTimers on the real run loop:
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
    // 4. **Live viewport → breakpoint recompute** (regression: native
    //    breakpoints frozen at their seed) — resize the real
    //    NSWindow across the default 1280 pt Xl threshold and assert
    //    both the captured bucket `ReadSignal` AND the on-screen
    //    reactive NSTextField ("breakpoint = Xl") followed: the full
    //    `setFrameSize:` → dual-sink → world-ctx → memo → f-string
    //    chain against live AppKit.
    // 5. **Embedded wgpu simulator** — the tree carries a
    //    `graphics` surface whose `on_ready` mounts
    //    `host_macos_desktop::mount_newcore` (a tiny wgpu scene
    //    realized into THIS app's world — see `mod embed`). Asserts
    //    the mount landed and the embed's raf-driven tick signal
    //    ADVANCES between two samples: the embedded scene's staged
    //    writes commit through the app's own flush driver, the
    //    one-world-per-thread contract of `start_in_world`. Render
    //    evidence is the host's "[host-macos-desktop] first frame
    //    presented" stderr line.
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

        runtime_shared::scheduling::after_ms_detached(1500, move || {
            // Presentation prerequisite for phase 5 (see
            // `bring_window_front`'s docs): surface the window so the
            // embedded wgpu host's CAMetalLayer can acquire drawables.
            selftest::bring_window_front();
            count.set(41); // stages — the dispatch hook must commit it
            runtime_shared::scheduling::after_ms_detached(700, move || {
                let committed = count.get() == 41;
                let views = selftest::live_view_count();
                // Phase 2: real target-action press through the
                // wrapped Action (stages 41 → 42; the wrapper's
                // scheduled flush must commit it by the next check).
                let pressed = selftest::press_button("Increment");
                runtime_shared::scheduling::after_ms_detached(400, move || {
                    let press_committed = count.get() == 42;
                    let tracking = selftest::run_tracking_loop_scroll_test();
                    // Phase 4: live viewport → breakpoint recompute
                    // (regression: native breakpoints frozen
                    // at their seed). The 480 pt launch width
                    // classifies Xs on the default table; resize the
                    // REAL window across the 1280 pt Xl threshold and
                    // the AppKit autoresize chain must drive
                    // `LayoutObserverView::setFrameSize:` → both
                    // viewport sinks → the world ctx → the bucket memo
                    // → the reactive breakpoint text in the live tree.
                    let bp_before = bp.get();
                    let resized = selftest::resize_window(1300.0, 600.0);
                    runtime_shared::scheduling::after_ms_detached(500, move || {
                        let bp_after = bp.get();
                        let bp_flipped = bp_before == runtime_shared::Breakpoint::Xs
                            && bp_after == runtime_shared::Breakpoint::Xl;
                        let bp_text_live = selftest::find_text("breakpoint = Xl");
                        // Phase 5: embedded wgpu simulator
                        // (`host_macos_desktop::mount_newcore` — see
                        // `mod embed`). Sample the embed's tick signal
                        // now and again after a delay: an advancing
                        // count proves the embedded scene's scoped raf
                        // loop stages writes that the APP's flush
                        // driver commits (one world, one driver). The
                        // "[host-macos-desktop] first frame presented"
                        // stderr line is the render-side evidence.
                        let embed_mounted = embed::is_mounted();
                        let ticks_before = embed::ticks_now().unwrap_or(0);
                        runtime_shared::scheduling::after_ms_detached(600, move || {
                            let ticks_after = embed::ticks_now().unwrap_or(0);
                            let embed_animates =
                                embed_mounted && ticks_after > ticks_before;
                            let ok = committed
                                && views > 10
                                && pressed
                                && press_committed
                                && tracking.committed_during_tracking
                                && tracking.mode_at_commit == "NSEventTrackingRunLoopMode"
                                && resized
                                && bp_flipped
                                && bp_text_live
                                && embed_animates;
                            println!(
                                "[SMOKE-SELFTEST] committed={committed} views={views} \
                                 press_commit={press_committed} tracking_commit={} \
                                 mode_at_commit={} scroll_y={} \
                                 bp_before={bp_before:?} bp_after={bp_after:?} \
                                 bp_text_live={bp_text_live} \
                                 embed_mounted={embed_mounted} \
                                 embed_ticks={ticks_before}->{ticks_after} \
                                 occlusion={} verdict={}",
                                tracking.committed_during_tracking,
                                tracking.mode_at_commit,
                                tracking.scroll_y,
                                selftest::window_occlusion(),
                                if ok { "PASS" } else { "FAIL" }
                            );
                            // The static tree alone mounts well over 10
                            // views (column + texts + buttons + toggle +
                            // dyn hole + scroll view + keyed rows); a low
                            // count means realize/finish didn't attach.
                            std::process::exit(if ok { 0 } else { 1 });
                        });
                    });
                });
            });
        });
    }

    let tree = view()
        .style(padded_column())
        .child(text().content("New-core macOS smoke"))
        .child(text().content(move || format!("breakpoint = {:?}", bp.get())))
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
        );

    // Embedded wgpu simulator (the native mirror of the
    // website's in-page preview): a `graphics` surface whose `on_ready`
    // mounts `host_macos_desktop::mount_newcore`, realizing a tiny
    // scene into THIS app's world
    // (`backend_macos::newcore::mounted_world`). Self-test phase 5
    // asserts the mount landed and that the embed's raf-driven tick
    // signal advances — the staged writes commit through the APP's
    // flush driver (one thread, one world, one logical update stream).
    #[cfg(target_os = "macos")]
    let tree = tree.child(
        runtime_vocabulary::graphics(embed::mount).style(embed::embed_region()),
    );

    tree.build()
}

/// Embedded wgpu simulator — the smoke's live proof of the
/// `host_macos_desktop::mount_newcore` seam (self-test phase 5).
///
/// The `graphics` primitive's `on_ready` hands over a CAMetalLayer
/// surface; the async wgpu init rides `driver::spawn_async` (the
/// libdispatch executor `install_scheduler` provides under
/// `async-driver`), and the embedded tree realizes into THIS app's
/// world via `backend_macos::newcore::mounted_world()` — no second
/// flush driver, no viewport clobber (`start_in_world` installs
/// neither).
#[cfg(target_os = "macos")]
mod embed {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use runtime_shared::primitives::graphics::OnReadyEvent;
    use runtime_shared::{Length, StyleRules, Tokenized};
    use runtime_vocabulary::{text, view};
    use runtime_world::Signal;

    /// Embed viewport in logical px — small on purpose (a preview
    /// strip, not a second app).
    const EMBED_W: u32 = 220;
    const EMBED_H: u32 = 120;

    thread_local! {
        /// The live embed handle (drop = unmount). `!Send` wgpu state —
        /// a plain thread-local slot, same shape as the website
        /// simulator's handle slot.
        static HANDLE: RefCell<Option<host_macos_desktop::MacosHostHandle>> =
            const { RefCell::new(None) };
        /// The embedded scene's tick signal (`Copy` handle). Phase 5
        /// reads it from the outer app to prove the embed animates
        /// through the SHARED world's flush driver.
        static TICKS: Cell<Option<Signal<i32>>> = const { Cell::new(None) };
    }

    /// Fixed-size region for the `graphics` surface.
    pub fn embed_region() -> StyleRules {
        StyleRules {
            width: Some(Tokenized::Literal(Length::Px(EMBED_W as f32))),
            height: Some(Tokenized::Literal(Length::Px(EMBED_H as f32))),
            ..StyleRules::default()
        }
    }

    /// True once `mount_newcore` resolved Ok.
    pub fn is_mounted() -> bool {
        HANDLE.with(|h| h.borrow().is_some())
    }

    /// Current embedded tick count (`None` until the embed built).
    /// Bare `get()` is legal off-world: handles route to their own
    /// world for reads.
    pub fn ticks_now() -> Option<i32> {
        TICKS.with(|t| t.get()).map(|s| s.get())
    }

    /// `graphics` `on_ready` → async wgpu init → `mount_newcore` into
    /// the app's world.
    pub fn mount(event: OnReadyEvent) {
        // `OnReadyEvent` now carries a `GraphicsTarget` rather than a bare
        // surface field: `into_surface()` yields the raw-window handle by
        // value (what `mount` wants), or `None` on a GL-context backend.
        // Read `size` first — `into_surface` consumes the event.
        let size = event.size;
        let Some(surface) = event.into_surface() else {
            eprintln!("[SMOKE-EMBED] on_ready gave a GL context, not a raw-window surface; \
                       this embed needs wgpu — skipping mount");
            return;
        };
        runtime_shared::driver::spawn_async(async move {
            let profile = host_macos_desktop::DeviceProfile {
                logical_size: (EMBED_W, EMBED_H),
                position: None,
                title: String::new(),
                color_scheme: runtime_shared::ColorScheme::Light,
            };
            let skin: Rc<dyn host_macos_desktop::Painter> =
                Rc::new(render_wgpu::NativeSkin::new(runtime_shared::Platform::MacOs));
            let build = Rc::new(|| {
                let ticks = runtime_world::signal(0i32);
                TICKS.with(|t| t.set(Some(ticks)));
                // Scoped raf loop: the build position anchors it to the
                // embed's `collect_owned` harvest (collector-owned
                // keepalive), so it dies at embed unmount. Each tick's
                // staged write commits via the APP's flush driver — the
                // one-world-per-thread seam phase 5 proves.
                runtime_vocabulary::scoped_scheduling::raf_loop_scoped(move || {
                    ticks.update(|n| n + 1);
                });
                view()
                    .child(text().content(move || format!("embed ticks = {}", ticks.get())))
                    .build()
            });
            match host_macos_desktop::mount_newcore(surface, size, profile, skin, build).await
            {
                Ok(handle) => {
                    eprintln!("[SMOKE-EMBED] mounted (newcore, {}x{} px)", size.0, size.1);
                    HANDLE.with(|h| *h.borrow_mut() = Some(handle));
                }
                Err(e) => eprintln!("[SMOKE-EMBED] mount failed: {e}"),
            }
        });
    }
}

#[cfg(target_os = "macos")]
mod selftest {
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;

    use objc2_app_kit::{NSApplication, NSView};
    use objc2_foundation::{MainThreadMarker, NSString};
    use runtime_shared::primitives::scroll_view::ScrollViewHandle;

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
        runtime_shared::scheduling::after_ms_detached(600, || {});

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

    /// Resize the app's real NSWindow (phase 4's trigger).
    /// `setFrame:display:` drives the same content-view autoresize
    /// chain a user drag does — host root → `LayoutObserverView`'s
    /// `setFrameSize:` — short of posting window-server drag events
    /// (which need accessibility grants a headless self-test can't
    /// assume). Returns false if no window is up.
    /// Bring the smoke window on screen (activate the app + order the
    /// window front). The embedded-wgpu phase presents through
    /// `CAMetalLayer.nextDrawable`, which wgpu SKIPS
    /// (`SurfaceError::Occluded`) while the window's `occlusionState`
    /// lacks the visible bit — e.g. when the smoke launches behind a
    /// fullscreen editor. A self-test that must PRESENT is entitled to
    /// put its window where presentation is possible. (If the display
    /// itself is asleep/locked, nothing can help — phase 5's verdict
    /// therefore rides the tick signal, with presentation logged as
    /// separate evidence.)
    pub fn bring_window_front() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let _: () = unsafe { objc2::msg_send![&*app, activateIgnoringOtherApps: true] };
        if let Some(window) = app.windows().iter().next() {
            let _: () = unsafe { objc2::msg_send![&*window, orderFrontRegardless] };
        }
    }

    /// Raw `NSWindow.occlusionState` bits (diagnostic — bit 1 set =
    /// visible on screen, the gate wgpu's Metal acquire checks).
    pub fn window_occlusion() -> usize {
        let Some(mtm) = MainThreadMarker::new() else {
            return 0;
        };
        let app = NSApplication::sharedApplication(mtm);
        let windows = app.windows();
        let Some(window) = windows.iter().next() else {
            return 0;
        };
        unsafe { objc2::msg_send![&*window, occlusionState] }
    }

    pub fn resize_window(width: f64, height: f64) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let app = NSApplication::sharedApplication(mtm);
        let windows = app.windows();
        let Some(window) = windows.iter().next() else {
            return false;
        };
        let mut frame: objc2_foundation::CGRect = unsafe { objc2::msg_send![&*window, frame] };
        frame.size.width = width;
        frame.size.height = height;
        let _: () = unsafe { objc2::msg_send![&*window, setFrame: frame, display: true] };
        true
    }

    /// True if any live NSTextField under the key window currently
    /// shows exactly `want` — real-AppKit evidence that a reactive
    /// f-string re-rendered (phase 4 reads "breakpoint = Xl" after the
    /// resize).
    pub fn find_text(want: &str) -> bool {
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
        fn walk(view: &NSView, want: &str) -> bool {
            let is_field: bool =
                unsafe { objc2::msg_send![view, isKindOfClass: objc2::class!(NSTextField)] };
            if is_field {
                let value: Option<objc2::rc::Retained<NSString>> =
                    unsafe { objc2::msg_send_id![view, stringValue] };
                if value.map(|v| v.to_string()).as_deref() == Some(want) {
                    return true;
                }
            }
            let subviews = unsafe { view.subviews() };
            subviews.iter().any(|v| walk(&v, want))
        }
        walk(&content, want)
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
    use runtime_shared::primitives::scroll_view::ScrollViewHandle;

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
