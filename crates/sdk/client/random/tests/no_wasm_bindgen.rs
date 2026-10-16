//! `random`'s web graph carries no wasm-bindgen.
//!
//! The usual entropy crate, `getrandom`, reaches the browser through
//! wasm-bindgen, and any wasm-bindgen in an app's graph forces the hybrid
//! build (docs/proposals/own-web-bindings.md). An app that wants a dice roll
//! must not pay for that, so web entropy goes through web-glue and
//! `getrandom` is native-only. This pins it.
#![cfg(not(target_arch = "wasm32"))]

use std::process::Command;

#[test]
fn regression_the_web_graph_links_no_wasm_bindgen() {
    let out = Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["tree", "-p", "random", "--target", "wasm32-unknown-unknown", "-e", "normal", "--offline"])
        .output()
        .expect("run cargo tree");
    assert!(out.status.success(), "cargo tree failed: {}", String::from_utf8_lossy(&out.stderr));
    let tree = String::from_utf8(out.stdout).unwrap();
    for banned in ["wasm-bindgen ", "web-sys ", "js-sys ", "getrandom "] {
        assert!(!tree.contains(banned), "random's wasm32 graph has {banned}:\n{tree}");
    }
}
