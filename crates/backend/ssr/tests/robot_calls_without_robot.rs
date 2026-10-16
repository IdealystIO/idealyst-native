//! App code calls the robot (`watch_signal`, `is_test_driven`) with no
//! `cfg`, and a build without the robot — this crate's graph has no
//! `runtime-vocabulary/robot` — renders it.
//!
//! An app can't gate those calls itself: `idealyst dev` / `idealyst test`
//! turn on the framework's `robot` features, never the app crate's, so an
//! app-side `#[cfg(feature = "robot")]` is off in dev builds too and the
//! test would never see the signal. With the calls behind the robot's
//! feature only, the same component failed to compile here — SSR builds
//! without the robot.

use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::robot;
use runtime_world::{memo, signal};

#[test]
fn regression_robot_calls_compile_and_do_nothing_without_the_robot() {
    let page = backend_ssr::newcore::render_path("/", || {
        let count = signal(3i32);
        let doubled = memo(move || count.get() * 2);
        robot::watch_signal("count", count);
        robot::watch_signal("count_read", count.read_only());
        robot::watch_signal("doubled", doubled);
        robot::watch_signal_writable("count_writable", count);
        robot::set_test_driven(true);
        let label = if robot::is_test_driven() { "driven" } else { "not driven" };
        view().child(text().content(label)).build()
    });
    assert!(page.html.contains("not driven"), "nothing drives a robot-less build: {}", page.html);
}
