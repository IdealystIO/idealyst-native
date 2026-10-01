//! Regression: the terminal backend's mount installed a wall clock whose local
//! offset was always UTC, so `runtime_core::time::local_offset_minutes()`
//! read `0` and the framework's date UI ("today" in a calendar or date
//! picker) showed the UTC date. The fix is in the shared native default
//! (`runtime_shared::time::SystemWallClockSource`, through `zone-offset`);
//! this proves the backend's real boot path (`newcore::start`) ends up with
//! it — a backend installing its own UTC source first would fail here.
//!
//! The wall clock is a process-wide first-install-wins `OnceLock` and
//! `$TZ` is process-wide, so the parent re-runs THIS test binary filtered
//! to `child_mounts_and_reports_offset` with a forced `$TZ`. Unix only:
//! `$TZ` steers libc's zone lookup, not Windows' registry-backed one.

#![cfg(unix)]

use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;

const CHILD_ENV: &str = "WALL_CLOCK_ZONE_CHILD";

struct NoopHandle;
impl runtime_shared::scheduling::ScheduleHandle for NoopHandle {
    fn cancel(&mut self) {}
}

/// Drops every task: the child only needs the mount preamble to run, not
/// the flush it schedules.
struct DropScheduler;
impl runtime_shared::scheduling::Scheduler for DropScheduler {
    fn schedule_microtask(&self, _f: Box<dyn FnOnce() + 'static>) {}
    fn after_animation_frame(
        &self,
        _f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn runtime_shared::scheduling::ScheduleHandle> {
        Box::new(NoopHandle)
    }
    fn after_ms(
        &self,
        _delay_ms: i32,
        _f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn runtime_shared::scheduling::ScheduleHandle> {
        Box::new(NoopHandle)
    }
    fn raf_loop(
        &self,
        _f: Box<dyn FnMut() + 'static>,
    ) -> Box<dyn runtime_shared::scheduling::ScheduleHandle> {
        Box::new(NoopHandle)
    }
}

/// Runs only as the child; a plain test run passes it trivially.
#[test]
fn child_mounts_and_reports_offset() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    runtime_shared::scheduling::install_scheduler(Box::new(DropScheduler));
    // Nothing read the clock before the mount: the install is the boot's.
    let backend = Rc::new(RefCell::new(backend_terminal::TerminalBackend::new()));
    backend_terminal::install_global_self(Rc::downgrade(&backend));
    backend.borrow_mut().set_viewport(20, 10);
    let app = backend_terminal::newcore::start(
        backend.clone(),
        |_| {},
        || runtime_vocabulary::builders::view().build(),
    );
    println!("OFFSET {}", runtime_shared::time::local_offset_minutes());
    app.stop();
}

fn offset_under(tz: &str) -> i32 {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "child_mounts_and_reports_offset",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .env("TZ", tz)
        .output()
        .expect("spawn child test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "child for TZ={tz} failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
        .lines()
        .find_map(|l| {
            l.split_once("OFFSET ")
                .map(|(_, r)| r.trim().parse().unwrap())
        })
        .unwrap_or_else(|| panic!("no report for TZ={tz}:\n{stdout}"))
}

#[test]
fn regression_terminal_mount_reports_local_offset_not_utc() {
    // +05:30 all year: independent of when the test runs.
    assert_eq!(offset_under("Asia/Kolkata"), 330);
    let ny = offset_under("America/New_York");
    assert!(ny == -300 || ny == -240, "New York: {ny}");
}
