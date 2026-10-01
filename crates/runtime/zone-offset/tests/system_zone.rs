//! The real platform zone database, driven through `$TZ` in a child process.
//!
//! `$TZ` is process-wide and tests run on parallel threads, so the parent
//! re-runs THIS test binary filtered to `child_reports_offsets` with a
//! chosen `$TZ`, and checks what it prints. Unix only: Windows' zone APIs
//! read the registry and ignore `$TZ`; that arm's arithmetic is unit-tested
//! in `src/lib.rs` (`filetime`) and its Win32 calls are compile-checked.

#![cfg(all(unix, not(target_arch = "wasm32")))]

use std::process::Command;

const CHILD_ENV: &str = "ZONE_OFFSET_TZ_CHILD";
const H: i32 = 3_600;

/// 2024-01-15T12:00:00Z, 2024-07-15T12:00:00Z, the US spring-forward instant
/// 2024-03-10T07:00:00Z and one second before it, and 2006-03-20T12:00:00Z
/// (EST under the pre-2007 rule, EDT under today's).
const INSTANTS: [i64; 5] = [
    1_705_320_000,
    1_721_044_800,
    1_710_054_000 - 1,
    1_710_054_000,
    1_142_856_000,
];

fn offsets_under(tz: &str) -> (Vec<i32>, i32) {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "child_reports_offsets",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .env("TZ", tz)
        .output()
        .expect("spawn child test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "child for TZ={tz} failed:\n{stdout}");
    let line = stdout
        .lines()
        .find_map(|l| l.split_once("OFFSETS ").map(|(_, r)| r))
        .unwrap_or_else(|| panic!("no report for TZ={tz}:\n{stdout}"));
    let (at, now) = line.split_once(' ').unwrap();
    (
        at.split(',').map(|s| s.parse().unwrap()).collect(),
        now.trim().parse().unwrap(),
    )
}

/// Runs only as the child; a plain test run passes it trivially.
#[test]
fn child_reports_offsets() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let at: Vec<String> = INSTANTS
        .iter()
        .map(|&s| zone_offset::local_offset_seconds_at(s).to_string())
        .collect();
    println!(
        "OFFSETS {} {}",
        at.join(","),
        zone_offset::local_offset_seconds_now()
    );
}

#[test]
fn new_york_follows_us_dst_including_the_2006_rule() {
    let (at, now) = offsets_under("America/New_York");
    assert_eq!(at, vec![-5 * H, -4 * H, -5 * H, -4 * H, -5 * H]);
    assert!(now == -5 * H || now == -4 * H, "now: {now}");
}

#[test]
fn kolkata_is_a_fixed_half_hour_offset() {
    let (at, now) = offsets_under("Asia/Kolkata");
    assert!(at.iter().all(|&o| o == 5 * H + 30 * 60), "{at:?}");
    assert_eq!(now, 5 * H + 30 * 60);
}

#[test]
fn utc_is_zero() {
    let (at, now) = offsets_under("UTC");
    assert!(at.iter().all(|&o| o == 0), "{at:?}");
    assert_eq!(now, 0);
}
