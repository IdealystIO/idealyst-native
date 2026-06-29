//! Cross-platform end-to-end tests, authored in Rust.
//!
//! Write a test as a normal function tagged [`#[robot_test]`](robot_test); it
//! drives a relay-connected [`App`] with a Playwright-flavoured
//! `locate → act → assert` vocabulary, and assertions panic on failure so the
//! body reads like an ordinary Rust test:
//!
//! ```ignore
//! use robot_test::{robot_test, App};
//!
//! #[robot_test]
//! fn increment_updates_count(app: &mut App) {
//!     app.test_id("counter").assert_text("Counter: 0");
//!     app.test_id("inc").click();
//!     app.test_id("inc").click();
//!     app.test_id("counter").assert_text("Counter: 2");
//!     app.signal("count").assert_eq(2);
//! }
//! ```
//!
//! Because `#[robot_test]` expands to a real `#[test]`, `cargo test` discovers
//! and runs these. But they need a live app + relay, which `cargo test` can't
//! set up — so when none is reachable each test **skips** (prints a note and
//! returns) rather than failing. `idealyst test` is what prepares the
//! environment: it launches the app on the chosen platform (`--web`, `--macos`,
//! `--ios`, `--android`), stands up the relay, points the tests at it via
//! `IDEALYST_ROBOT_BRIDGE`, and runs the very same `cargo test`. One suite,
//! every platform — the relay makes them all reachable as the same bridge.

mod app;
mod client;
mod harness;
pub mod parity;

pub use app::{App, Locator, SignalAssert};
pub use client::{default_apps_dir, discover, discover_all, RobotClient};
pub use parity::{capture_native, diff as diff_native, report as report_parity, Capture, Mismatch, Tolerance};

// The attribute macro that turns a function into a `#[test]`.
pub use robot_test_macros::robot_test;

// Called by the `#[robot_test]` expansion — public for the macro, hidden from
// the authoring surface.
#[doc(hidden)]
pub use harness::{__acquire, Acquire, AppGuard};
