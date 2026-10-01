//! Regression: `runtime_core::time::local_offset_minutes()` reported UTC
//! (`0`) on Android, Linux, Windows, terminal, CPU, Roku and the GPU hosts,
//! so the framework's own date UI (`CivilDate::today()`, the calendar's
//! "today" marker, `DatePicker`'s `min = today`) showed the UTC date there.
//! Only web, macOS and iOS installed a zone-aware wall clock; every other
//! backend got the default `SystemWallClockSource`, whose offset was a
//! hardcoded `0`.
//!
//! Every native backend's mount preamble boots the wall clock through the
//! same call — `install_default_time_source(platform)` with its own
//! `Platform` — so this drives exactly that call once per backend family's
//! `Platform` value and checks the installed clock's offset against a
//! forced zone.
//!
//! The wall clock is a process-wide first-install-wins `OnceLock` and `$TZ`
//! is process-wide, so each case re-runs THIS test binary filtered to
//! `child_installs_default_and_reports`, with `$TZ` and the platform set in
//! its environment; the child prints what core reports. Unix only, because
//! `$TZ` only steers libc's zone lookup — Windows' zone APIs read the
//! registry (that arm's arithmetic is unit-tested in `zone-offset`, its
//! Win32 calls compile-checked for `x86_64-pc-windows-gnu`). The installed
//! source and the selection logic are the same code on every target, so
//! this covers which clock a Windows/Android/Linux mount installs.

#![cfg(all(unix, not(target_arch = "wasm32")))]

use std::process::Command;

use runtime_shared::Platform;

const CHILD_PLATFORM_ENV: &str = "RUNTIME_SHARED_WALL_CLOCK_CHILD_PLATFORM";

/// Every non-web `Platform` a backend in this repo reports, by name. The
/// `Custom` names are the ones the backends return from
/// `AppEnvOps::platform` (linux, windows, terminal, cpu, the GPU sims and
/// the GPU engine's unnamed default).
const NATIVE_PLATFORMS: &[&str] = &[
    "Ios",
    "Android",
    "MacOs",
    "TvOs",
    "AndroidTv",
    "Roku",
    "Custom:linux",
    "Custom:windows",
    "Custom:Terminal",
    "Custom:cpu",
    "Custom:Sim",
    "Custom:",
];

fn platform_named(name: &str) -> Platform {
    match name {
        "Ios" => Platform::Ios,
        "Android" => Platform::Android,
        "MacOs" => Platform::MacOs,
        "TvOs" => Platform::TvOs,
        "AndroidTv" => Platform::AndroidTv,
        "Roku" => Platform::Roku,
        "Custom:linux" => Platform::Custom("linux"),
        "Custom:windows" => Platform::Custom("windows"),
        "Custom:Terminal" => Platform::Custom("Terminal"),
        "Custom:cpu" => Platform::Custom("cpu"),
        "Custom:Sim" => Platform::Custom("Sim"),
        "Custom:" => Platform::Custom(""),
        other => panic!("unknown platform {other}"),
    }
}

/// `(local_offset_minutes(), epoch_millis())` as core reports them in a
/// fresh process that booted `platform` under `TZ=tz`.
fn core_clock_under(tz: &str, platform: &str) -> (i32, i64) {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "child_installs_default_and_reports",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_PLATFORM_ENV, platform)
        .env("TZ", tz)
        .output()
        .expect("spawn child test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "child for TZ={tz} platform={platform} failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = stdout
        .lines()
        .find_map(|l| l.split_once("WALL-CLOCK ").map(|(_, r)| r))
        .unwrap_or_else(|| panic!("no report for TZ={tz} platform={platform}:\n{stdout}"));
    let (offset, epoch) = line.split_once(' ').unwrap();
    (offset.parse().unwrap(), epoch.trim().parse().unwrap())
}

/// Runs only as the child; a plain test run passes it trivially.
#[test]
fn child_installs_default_and_reports() {
    let Some(name) = std::env::var_os(CHILD_PLATFORM_ENV) else {
        return;
    };
    let platform = platform_named(name.to_str().unwrap());
    // Exactly what each backend's mount preamble does.
    runtime_shared::time::install_default_time_source(platform);
    println!(
        "WALL-CLOCK {} {}",
        runtime_shared::time::local_offset_minutes(),
        runtime_shared::time::epoch_millis()
    );
}

#[test]
fn regression_native_backends_report_the_local_offset_not_utc() {
    // Kolkata: +05:30 all year (no DST), so the expected value doesn't
    // depend on when the test runs, and the half hour catches an
    // hours-only offset.
    for platform in NATIVE_PLATFORMS {
        let (offset, epoch) = core_clock_under("Asia/Kolkata", platform);
        assert_eq!(offset, 330, "TZ=Asia/Kolkata, platform {platform}");
        assert!(epoch > 1_577_836_800_000, "{platform}: epoch {epoch}");
    }
}

#[test]
fn native_wall_clock_follows_dst_in_new_york() {
    // EST or EDT depending on today's date; never UTC.
    let (offset, _) = core_clock_under("America/New_York", "Android");
    assert!(offset == -300 || offset == -240, "got {offset}");
}

#[test]
fn native_wall_clock_is_zero_under_utc() {
    let (offset, _) = core_clock_under("UTC", "Custom:linux");
    assert_eq!(offset, 0);
}
