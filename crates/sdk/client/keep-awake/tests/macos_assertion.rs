//! The real IOKit assertion, read back with `pmset -g assertions`.

#![cfg(target_os = "macos")]

use keep_awake::{install_keep_awake_labels, is_supported, keep_screen_on, KeepAwakeLabels};

/// Whether this process holds a display-sleep assertion named `name`.
fn asserted(name: &str) -> bool {
    let out = std::process::Command::new("pmset").args(["-g", "assertions"]).output().expect("pmset");
    let mine = format!("pid {}(", std::process::id());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|l| l.contains(&mine) && l.contains("PreventUserIdleDisplaySleep") && l.contains(name))
}

/// One test: the assertion is the process's.
#[test]
fn a_hold_is_an_assertion_until_the_last_is_dropped() {
    assert!(is_supported());
    let reason = "keep-awake test reason";
    install_keep_awake_labels(KeepAwakeLabels { reason: reason.into() });
    assert!(!asserted(reason));
    let a = keep_screen_on();
    let b = keep_screen_on();
    assert!(asserted(reason));
    drop(a);
    assert!(asserted(reason), "one hold left");
    drop(b);
    assert!(!asserted(reason));
}
