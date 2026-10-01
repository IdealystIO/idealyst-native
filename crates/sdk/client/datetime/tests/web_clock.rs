//! Browser tests for the web clock (`Date` / `Intl` through web-glue).
//!
//! Run with
//! `cargo test -p datetime --target wasm32-unknown-unknown --features chrono`
//! (the workspace runner supplies web-glue's JS; `wasm-pack test` cannot).
//!
//! Each reading is checked against the same browser API called
//! independently through web-glue's reflect surface, in whatever zone the
//! browser runs. Chrome takes its zone from `$TZ`, so
//! `TZ=America/New_York cargo test …` additionally pins exact DST values
//! (`new_york_dst_when_the_browser_runs_there`).

#![cfg(target_arch = "wasm32")]

use datetime::{local_offset_at, local_timezone, now_utc, with_clock, FixedClock, Timestamp};
use wasm_bindgen_test::*;
use web_glue::JsValue;

wasm_bindgen_test_configure!(run_in_browser);

/// 2024-01-15T12:00:00Z, 2024-07-15T12:00:00Z, a second before / at the US
/// spring-forward (2024-03-10T07:00:00Z), and 2006-03-20T12:00:00Z (EST
/// under 2006's rule, EDT under today's).
const INSTANTS: [i64; 5] = [
    1_705_320_000,
    1_721_044_800,
    1_710_053_999,
    1_710_054_000,
    1_142_856_000,
];

fn js_date_now() -> f64 {
    JsValue::global()
        .get("Date")
        .unwrap()
        .call_method("now", &[])
        .unwrap()
        .as_f64()
        .unwrap()
}

/// `-(new Date(ms).getTimezoneOffset()) * 60`, straight from the browser.
fn js_offset_seconds(ms: f64) -> i32 {
    let date = JsValue::global()
        .get("Date")
        .unwrap()
        .construct(&[&JsValue::from_f64(ms)])
        .unwrap();
    let minutes = date
        .call_method("getTimezoneOffset", &[])
        .unwrap()
        .as_f64()
        .unwrap();
    (-minutes * 60.0).round() as i32
}

fn js_zone_name() -> String {
    let intl = JsValue::global().get("Intl").unwrap();
    let fmt = intl.get("DateTimeFormat").unwrap().construct(&[]).unwrap();
    fmt.call_method("resolvedOptions", &[])
        .unwrap()
        .get("timeZone")
        .unwrap()
        .as_string()
        .unwrap()
}

#[wasm_bindgen_test]
fn now_utc_is_date_now() {
    let before = js_date_now() as i64;
    let ours = now_utc().unix_millis();
    let after = js_date_now() as i64;
    assert!(
        before <= ours && ours <= after,
        "{before} <= {ours} <= {after}"
    );
}

#[wasm_bindgen_test]
fn offset_at_matches_the_browser_for_each_instant() {
    for s in INSTANTS {
        let ours = local_offset_at(Timestamp::from_unix_seconds(s)).seconds();
        assert_eq!(ours, js_offset_seconds(s as f64 * 1000.0), "instant {s}");
    }
}

#[wasm_bindgen_test]
fn timezone_is_the_intl_zone() {
    assert_eq!(local_timezone(), Some(js_zone_name()));
}

#[wasm_bindgen_test]
fn new_york_dst_when_the_browser_runs_there() {
    if js_zone_name() != "America/New_York" {
        return;
    }
    const H: i32 = 3_600;
    let got: Vec<i32> = INSTANTS
        .iter()
        .map(|&s| local_offset_at(Timestamp::from_unix_seconds(s)).seconds())
        .collect();
    assert_eq!(got, vec![-5 * H, -4 * H, -5 * H, -4 * H, -5 * H]);
}

#[wasm_bindgen_test]
fn a_fixed_clock_overrides_the_browser() {
    with_clock(
        FixedClock::new(Timestamp::from_unix_seconds(42)).with_timezone("Etc/Test"),
        || {
            assert_eq!(now_utc().unix_seconds(), 42);
            assert_eq!(local_timezone().as_deref(), Some("Etc/Test"));
        },
    );
}

/// The point of the feature: chrono values on web without chrono's `clock`
/// / `wasmbind` (which would panic here, or pull wasm-bindgen in).
#[cfg(feature = "chrono")]
#[wasm_bindgen_test]
fn chrono_now_works_on_web() {
    let before = js_date_now() as i64;
    let utc = datetime::now_chrono_utc();
    let local = datetime::now_chrono_local();
    let after = js_date_now() as i64;
    assert!(before <= utc.timestamp_millis() && utc.timestamp_millis() <= after);
    assert!(before <= local.timestamp_millis() && local.timestamp_millis() <= after);
    assert_eq!(
        local.offset().local_minus_utc(),
        js_offset_seconds(local.timestamp_millis() as f64)
    );
}
