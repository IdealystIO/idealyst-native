//! The real platform zone database, driven through `$TZ` in a child process.
//!
//! `$TZ` is process-wide state and Rust runs tests on parallel threads, so
//! setting it in-process would race every other test. Instead the parent
//! test re-runs THIS test binary, filtered to `child_reports_zone`, with a
//! chosen `$TZ`; the child prints what `SystemClock` reports and the parent
//! checks it. Unix only: Windows' zone APIs ignore `$TZ` (they read the
//! registry), so there is no way to pick a zone per process there — the
//! Windows arm is compile-checked (`cargo check --target
//! x86_64-pc-windows-gnu -p datetime`).

#![cfg(all(unix, not(target_arch = "wasm32")))]

use std::process::Command;

use datetime::{local_offset_at, local_timezone, Timestamp};

const CHILD_ENV: &str = "DATETIME_TZ_CHILD";

/// 2024-01-15T12:00:00Z — northern winter.
const JAN_2024: i64 = 1_705_320_000;
/// 2024-07-15T12:00:00Z — northern summer.
const JUL_2024: i64 = 1_721_044_800;
/// 2024-03-10T07:00:00Z — the instant US Eastern springs forward
/// (02:00 EST → 03:00 EDT).
const US_SPRING_2024: i64 = 1_710_054_000;
/// 2024-03-31T01:00:00Z — the instant the EU springs forward.
const EU_SPRING_2024: i64 = 1_711_846_800;
/// 2006-03-20T12:00:00Z — under TODAY's US rule (second Sunday of March)
/// New York would be on EDT, but in 2006 DST started on April 2: the
/// historic rule must win.
const MAR_2006: i64 = 1_142_856_000;

const INSTANTS: [i64; 7] = [
    JAN_2024,
    JUL_2024,
    US_SPRING_2024 - 1,
    US_SPRING_2024,
    EU_SPRING_2024 - 1,
    EU_SPRING_2024,
    MAR_2006,
];

/// What the child reported: an offset (seconds) per `INSTANTS` entry, and
/// the zone name.
struct Report {
    offsets: Vec<i32>,
    name: Option<String>,
    chrono_local_matches: bool,
}

fn run_child(tz: &str) -> Report {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "child_reports_zone",
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
    let line = stdout
        .lines()
        // libtest prints `test child_reports_zone ... ` on the same line.
        .find_map(|l| l.split_once("ZONE-REPORT ").map(|(_, r)| r))
        .unwrap_or_else(|| panic!("no report from child for TZ={tz}:\n{stdout}"));
    let mut fields = line.split_whitespace();
    let offsets = fields
        .next()
        .unwrap()
        .split(',')
        .map(|s| s.parse().unwrap())
        .collect();
    let name = match fields.next().unwrap() {
        "-" => None,
        n => Some(n.to_owned()),
    };
    let chrono_local_matches = fields.next() == Some("chrono-ok");
    Report {
        offsets,
        name,
        chrono_local_matches,
    }
}

/// Runs only as the child (`DATETIME_TZ_CHILD=1`); a plain test run passes it
/// trivially.
#[test]
fn child_reports_zone() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let offsets: Vec<String> = INSTANTS
        .iter()
        .map(|&s| {
            local_offset_at(Timestamp::from_unix_seconds(s))
                .seconds()
                .to_string()
        })
        .collect();
    let name = local_timezone().unwrap_or_else(|| "-".to_owned());
    // `now_chrono_local()` must carry the offset in effect now.
    let chrono = if cfg!(feature = "chrono") {
        chrono_check()
    } else {
        "chrono-off"
    };
    println!("ZONE-REPORT {} {name} {chrono}", offsets.join(","));
}

#[cfg(feature = "chrono")]
fn chrono_check() -> &'static str {
    let local = datetime::now_chrono_local();
    let expected = local_offset_at(local.into()).seconds();
    if local.offset().local_minus_utc() == expected {
        "chrono-ok"
    } else {
        "chrono-mismatch"
    }
}

#[cfg(not(feature = "chrono"))]
fn chrono_check() -> &'static str {
    "chrono-off"
}

const H: i32 = 3_600;

#[test]
fn new_york_follows_us_dst_including_the_2006_rule() {
    let r = run_child("America/New_York");
    assert_eq!(
        r.offsets,
        vec![-5 * H, -4 * H, -5 * H, -4 * H, -4 * H, -4 * H, -5 * H],
        "JAN EST, JUL EDT, a second before / at the US spring-forward, EU instants in EDT, March 2006 still EST"
    );
    assert_eq!(r.name.as_deref(), Some("America/New_York"));
    if cfg!(feature = "chrono") {
        assert!(r.chrono_local_matches);
    }
}

#[test]
fn berlin_follows_eu_dst() {
    let r = run_child("Europe/Berlin");
    assert_eq!(r.offsets, vec![H, 2 * H, H, H, H, 2 * H, H]);
    assert_eq!(r.name.as_deref(), Some("Europe/Berlin"));
}

#[test]
fn sydney_is_southern_hemisphere_dst() {
    let r = run_child("Australia/Sydney");
    // AEDT (+11) in January, AEST (+10) in July.
    assert_eq!(r.offsets[0], 11 * H);
    assert_eq!(r.offsets[1], 10 * H);
    assert_eq!(r.name.as_deref(), Some("Australia/Sydney"));
}

#[test]
fn kolkata_has_a_half_hour_offset_and_no_dst() {
    let r = run_child("Asia/Kolkata");
    assert!(
        r.offsets.iter().all(|&o| o == 5 * H + 30 * 60),
        "{:?}",
        r.offsets
    );
    assert_eq!(r.name.as_deref(), Some("Asia/Kolkata"));
}

#[test]
fn posix_rule_tz_gives_offsets_but_no_name() {
    // A POSIX rule string is a valid `$TZ`, but names no IANA zone.
    let r = run_child("EST5EDT,M3.2.0,M11.1.0");
    assert_eq!(r.offsets[0], -5 * H);
    assert_eq!(r.offsets[1], -4 * H);
    assert_eq!(r.name, None);
}

#[test]
fn utc_tz() {
    let r = run_child("UTC");
    assert!(r.offsets.iter().all(|&o| o == 0), "{:?}", r.offsets);
    assert_eq!(r.name.as_deref(), Some("UTC"));
}
