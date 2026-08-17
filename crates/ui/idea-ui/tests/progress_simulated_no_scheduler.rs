//! Regression: `Progress(mode = Simulated)` must not take the process
//! down when no scheduler is installed.
//!
//! ## The bug this pins
//!
//! The simulated creep advances by re-arming itself:
//! `schedule_sim_step` → `after_ms_scoped(delay, || { advance;
//! schedule_sim_step(..) })`. With no [`Scheduler`] installed,
//! `runtime_shared::scheduling::after_ms` runs its closure
//! SYNCHRONOUSLY on every non-web platform (the delay is ignored), so
//! the re-arm called straight back into `schedule_sim_step` and recursed
//! until the stack was exhausted. The failure mode is a SIGSEGV with a
//! ~100k-frame trace, not a panic — nothing catches it and nothing is
//! logged.
//!
//! `runtime_shared::scheduling::is_scheduler_installed` exists for
//! exactly this shape (see its doc); the SSR / terminal / cpu / roku /
//! email hosts already guard their own loops with it. `schedule_sim_step`
//! now does too.
//!
//! Found by sweeping the `idea-ui-docs` catalog on the GTK backend:
//! opening the Progress page from a host that had not called
//! `host_gtk::install_scheduler()` killed the process.
//!
//! ## Why this test is shaped like this
//!
//! There is no assertion to make about a stack overflow — it is not
//! catchable, so a test can only survive it or not. This file is
//! therefore its own assertion: it mounts the affected subtree in a
//! process with NO scheduler installed (`SCHEDULER` is a process-global
//! `OnceLock`, so this must live in its own integration-test binary and
//! must not install one), flushes, and returns. Before the guard it
//! aborted here; after it, it completes.

use idea_ui::components::progress::{Progress, ProgressMode};
use idea_ui::{install_idea_theme, light_theme};
use runtime_core::ui;

#[test]
fn regression_simulated_progress_does_not_overflow_without_a_scheduler() {
    assert!(
        !runtime_core::scheduling::is_scheduler_installed(),
        "this test is only meaningful with NO scheduler installed — \
         something in this binary installed one, so the synchronous \
         `after_ms` fallback (the whole hazard) is no longer exercised"
    );

    let harness = host_mock::Harness::new();
    let tree = harness.world.enter(|| {
        // Progress reads its stylesheet from the installed theme.
        install_idea_theme(light_theme());
        ui! { Progress(mode = ProgressMode::Simulated) }
    });

    // The mount is what armed the creep chain. Reaching the line after
    // `flush` is the pass condition.
    let realized = harness.mount(tree);
    harness.flush();
    drop(realized);
}
