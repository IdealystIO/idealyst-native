#![allow(deprecated)]

use robot_test::robot_test;

#[robot_test(suites = smoke, platforms = web)]
fn t(app: &mut robot_test::App) {
    let _ = app;
}

fn main() {}
