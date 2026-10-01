//! The `wasm-glue-test-runner` binary end to end, with a stand-in for
//! wasm-bindgen-test-runner that records the arguments it was given.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

/// `cargo test -q` over a glue crate's browser suite: cargo appends
/// `--quiet`, which wasm-bindgen-test-runner 0.2.128 rejects before any
/// test runs. The wrapper must hand it `--format terse` instead.
#[test]
fn regression_cargo_test_quiet_reaches_the_runner_as_terse_format() {
    let dir = std::env::temp_dir().join(format!("glue-runner-args-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A module with no glue: the wrapper passes it straight through.
    let wasm = dir.join("t.wasm");
    std::fs::write(&wasm, wasm_encoder::Module::new().finish()).unwrap();
    let seen = dir.join("args");
    let fake = dir.join("fake-runner");
    std::fs::write(&fake, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", seen.display())).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_wasm-glue-test-runner"))
        .arg(&wasm)
        .arg("--quiet")
        .env("WASM_BINDGEN_TEST_RUNNER", &fake)
        .status()
        .unwrap();
    assert!(status.success());
    let got = std::fs::read_to_string(&seen).unwrap();
    let got: Vec<&str> = got.lines().collect();
    assert_eq!(got, vec![wasm.to_str().unwrap(), "--format", "terse"]);
    let _ = std::fs::remove_dir_all(&dir);
}
