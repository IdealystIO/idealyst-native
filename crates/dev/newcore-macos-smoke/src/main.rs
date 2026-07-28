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
//! runloop-turn driver flush), a two-way `toggle`, a structural Dyn hole
//! (closure child), a keyed list with add/remove/reverse (keyed
//! reconciliation against live NSViews), and one literal `StyleRules`
//! (the `StyleOps` delegation on the native apply-style path).

use runtime_core::{Length, StyleRules, Tokenized};
use runtime_scene::{keyed, Element};
use runtime_vocabulary::builders::IntoSceneElement;
use runtime_vocabulary::{button, text, toggle, view};
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

/// The app tree. Runs inside `World::enter` (the boot path wraps it), so
/// the free `signal()` constructor works; these top-level signals are
/// world-root-owned and live for the app.
pub fn app() -> Element {
    let count = signal(0i32);
    let on = signal(false);
    let rows = signal(vec![1u32, 2, 3]);
    let next_row = signal(4u32);

    // Headless live-verification hook (`NEWCORE_SMOKE_SELFTEST=1`): from
    // a real NSTimer on the real run loop, stage a write and later
    // assert it was COMMITTED by the flush driver (a staged `set` is
    // only observable through `get` after a `world.flush()` — on this
    // backend that means the frame-tick hook fired), and that the tree
    // actually realized into live NSViews under the key window. Exits 0
    // on success / 1 on failure so a CI-ish caller can gate on it.
    // Scheduled from inside the build (scheduler is installed by then;
    // `after_ms` uses an NSTimer, which is NOT mount-buffered, so it
    // fires ~1.5 s after the run loop starts).
    #[cfg(target_os = "macos")]
    if std::env::var("NEWCORE_SMOKE_SELFTEST").as_deref() == Ok("1") {
        runtime_core::scheduling::after_ms_detached(1500, move || {
            count.set(41); // stages — the driver must commit it
            runtime_core::scheduling::after_ms_detached(700, move || {
                let committed = count.get() == 41;
                let views = selftest::live_view_count();
                println!("[SMOKE-SELFTEST] committed={committed} views={views}");
                // The static tree alone mounts well over 10 views
                // (column + 3 texts + 3 buttons + toggle + dyn hole +
                // 3 keyed rows); a low count means realize/finish
                // didn't attach.
                std::process::exit(if committed && views > 10 { 0 } else { 1 });
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
        .build()
}

#[cfg(target_os = "macos")]
mod selftest {
    use objc2_app_kit::{NSApplication, NSView};
    use objc2_foundation::MainThreadMarker;

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
