//! `#[idea_test(suites = …)]` end to end: real suite members, alone and
//! with platforms, listed by the test binary's own `--list` — the names
//! `idealyst test --suite` / `--list-suites` read the suites from.
//!
//! The members are ignored (the platform-bound one by the macro): what
//! they prove is that they compile, their bodies reaching the file's items
//! through the generated modules, under the names listed below.

use robot_test::{idea_test, json, App};

/// A file-level item the bodies reach through the modules' `use super::*`.
fn note() -> serde_json::Value {
    json!({})
}

#[idea_test(suites = [smoke, checkout])]
#[ignore = "listed, not run"]
fn pays(app: &mut App) {
    app.call_function("pays", note());
}

#[idea_test(suites = smoke, platforms = [ios, android])]
fn opens_the_number_pad(app: &mut App) {
    app.call_function("pad", note());
}

#[idea_test]
#[ignore = "listed, not run"]
fn in_no_suite(app: &mut App) {
    app.call_function("loose", note());
}

#[test]
fn the_binary_lists_members_under_their_suite_names() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--list", "--format", "terse"])
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&out.stdout);
    let mut names: Vec<&str> = listed.lines().filter_map(|l| l.strip_suffix(": test")).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "in_no_suite",
            "opens_the_number_pad::idea_suites_smoke::robot_only_ios_android",
            "pays::idea_suites_checkout__smoke",
            "the_binary_lists_members_under_their_suite_names",
        ]
    );
    assert_eq!(robot_test::suites_of_test(names[2]), ["checkout", "smoke"]);
    assert_eq!(robot_test::suites_of_test(names[1]), ["smoke"]);
    assert_eq!(robot_test::platforms_of_test(names[1]), Some(vec!["ios", "android"]));
    assert!(robot_test::suites_of_test(names[0]).is_empty());
}
