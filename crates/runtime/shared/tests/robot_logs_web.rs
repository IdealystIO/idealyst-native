//! Regression: `robot::logs::push` (and `robot_log!`) panicked on
//! `wasm32-unknown-unknown`, because the entry timestamp came from
//! `std::time::SystemTime::now()`, which wasm32 has no clock for. A web robot
//! build (backend-web's `robot` feature) that logged a single line aborted.
//! The timestamp now comes from the core wall clock, which the web backend
//! fills with `js Date`.
//!
//! Run with `cargo test -p runtime-shared --features robot --target
//! wasm32-unknown-unknown --test robot_logs_web`.

#![cfg(all(target_arch = "wasm32", feature = "robot"))]

use runtime_shared::robot::logs;
use runtime_shared::time::{install_wall_clock_source, WallClockSource};
use wasm_bindgen_test::*;

/// 2023-11-14T22:13:20Z.
const FIXED_MS: i64 = 1_700_000_000_000;

/// Stands in for the web backend's `js Date` source.
struct Fixed;
impl WallClockSource for Fixed {
    fn epoch_millis(&self) -> i64 {
        FIXED_MS
    }
    fn local_offset_minutes(&self) -> i32 {
        0
    }
}

#[wasm_bindgen_test]
fn regression_robot_log_push_does_not_panic_on_wasm() {
    install_wall_clock_source(Box::new(Fixed));
    logs::clear();
    runtime_shared::robot_log!("test", "hello {}", 1);
    let entries = logs::recent(10);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].text, "hello 1");
    assert_eq!(entries[0].timestamp_ms, FIXED_MS as u64);
    assert_eq!(logs::since(FIXED_MS as u64 - 1).len(), 1);
}
