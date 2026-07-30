//! Smoke app for the Android backend's boot path
//! (`backend_android::newcore::start`).
//!
//! Everything here is DIRECT vocabulary-builder calls — no `ui!`, no
//! `jsx!` — deliberately, so this crate proves the `runtime_scene`
//! registry-dispatch render path against real Android Views
//! independent of the macro-lowering work (the sanctioned deviation
//! from CLAUDE.md §9.2, same as `newcore-macos-smoke`: this crate
//! exists to gate the layer *under* the macro). The tree mirrors
//! `newcore-macos-smoke` — same coverage, third native backend.
//!
//! Coverage: static + reactive `text`, `button` (JNI click → wrapped
//! author callback → staged write → looper-turn driver flush), a
//! two-way `toggle`, a structural Dyn hole (closure child), a keyed
//! list with add/remove/reverse (keyed reconciliation against live
//! Android Views), and one literal `StyleRules` (the `StyleOps`
//! delegation on the native apply-style path).
//!
//! Hosted by the CLI-generated JNI wrapper: [`scene_app`] is the entry
//! its `attach` mounts, and [`register_scene_extensions`] is the
//! SDK-registration seam it passes alongside.

use runtime_shared::{Length, StyleRules, Tokenized};
use runtime_scene::{keyed, Element};
use runtime_vocabulary::builders::IntoSceneElement;
use runtime_vocabulary::{button, text, toggle, view};
use runtime_world::signal;

/// A minimal literal style — exercises the `StyleOps` delegation (on
/// Android: per-node `apply_style` through Taffy + GradientDrawable,
/// not class minting).
fn padded_column() -> StyleRules {
    StyleRules {
        padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
        padding_left: Some(Tokenized::Literal(Length::Px(16.0))),
        gap: Some(Tokenized::Literal(Length::Px(8.0))),
        ..StyleRules::default()
    }
}

/// The app tree. Runs inside `World::enter` (the boot path
/// wraps it), so the free `signal()` constructor works; these
/// top-level signals are world-root-owned and live for the app.
pub fn scene_app() -> Element {
    let count = signal(0i32);
    let on = signal(false);
    let rows = signal(vec![1u32, 2, 3]);
    let next_row = signal(4u32);

    // Always-on logcat self-test (the Android analogue of
    // newcore-macos-smoke's NEWCORE_SMOKE_SELFTEST): from a real
    // `Handler.postDelayed` timer on the real main looper, STAGE a
    // write and later assert it was COMMITTED by the flush driver — a
    // staged `set` is only observable through `get` after a
    // `world.flush()`, and the only thing that can flush here is the
    // scheduler's post-dispatch hook firing after the timer callback.
    // Also count the live Android Views under the host root over JNI
    // to prove realize/finish attached real widgets. The verdict line
    // is greppable in logcat:
    //   adb logcat -s idealyst | grep SMOKE-SELFTEST
    // The process does NOT exit (Android apps don't); the harness
    // greps for `verdict=PASS`. The static tree alone mounts well
    // over 10 views (column + texts + buttons + toggle + dyn hole +
    // 3 keyed rows, several of which are compound widgets); a low
    // count means realize/finish didn't attach.
    // The timers are SCOPE-OWNED, not detached: they're scheduled from
    // a (read-free, run-once) effect whose returned cleanup drops the
    // cancel-on-drop handles, so a teardown (detach → attach on
    // Activity recreation — Android recreates the Activity ~0.5 s
    // after a cold emulator launch) cancels the posted runnables. Two
    // live-caught bugs shaped this block:
    // - `after_ms_detached` let the mount-#1 timer outlive its dropped
    //   world; the `count.get()` read aborted with `dead-world-read`
    //   (the fresh mount #2 reschedules its own self-test, so nothing
    //   is lost by cancelling).
    // - a bare `runtime_world::on_cleanup` at the BUILD level panics
    //   ("on_cleanup called outside an effect") — cleanups belong to
    //   effects, hence the effect wrapper.
    #[cfg(target_os = "android")]
    {
        use std::cell::RefCell;
        use std::rc::Rc;
        runtime_world::effect(move || {
            let pending: Rc<RefCell<Vec<runtime_shared::scheduling::ScheduledTask>>> =
                Rc::new(RefCell::new(Vec::new()));
            let chain = pending.clone();
            let t = runtime_shared::scheduling::after_ms(2500, move || {
                count.set(41); // stages — the driver must commit it
                let t2 = runtime_shared::scheduling::after_ms(700, move || {
                    let committed = count.get() == 41;
                    let views = backend_android::newcore::live_view_count();
                    let verdict = if committed && views > 10 { "PASS" } else { "FAIL" };
                    log::info!(
                        "[SMOKE-SELFTEST] committed={committed} views={views} verdict={verdict}"
                    );
                });
                chain.borrow_mut().push(t2);
            });
            pending.borrow_mut().push(t);
            move || pending.borrow_mut().clear()
        });
    }

    view()
        .style(padded_column())
        .child(text().content("New-core Android smoke"))
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

/// SDK-handler registration seam the CLI-generated JNI wrapper invokes
/// after `runtime_vocabulary::register_builtins` (the wrapper's `attach`
/// passes it straight into `backend_android::newcore::start`). This
/// scaffold registers nothing, so it is an empty generic over the scene
/// registry's host.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}
