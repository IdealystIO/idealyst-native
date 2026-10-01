//! canvas-native's default web graph carries no wasm-bindgen.
//!
//! A module that links wasm-bindgen builds in hybrid mode — the wasm-bindgen
//! CLI runs over it (docs/proposals/own-web-bindings.md). This crate used to
//! depend on wasm-bindgen + web-sys unconditionally for two web-sys-typed
//! entry points only canvas-vello calls, and that alone made every canvas app
//! (examples/whiteboard-demo, charts-demo) a hybrid build. Those entry points
//! are behind `web-sys-canvas` now; this pins that the default graph stays
//! free of them.
#![cfg(not(target_arch = "wasm32"))]

use std::process::Command;

fn wasm32_tree(features: &[&str]) -> String {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["tree", "-p", "canvas-native", "--target", "wasm32-unknown-unknown", "-e", "normal", "--offline"]);
    if !features.is_empty() {
        cmd.args(["--features", &features.join(",")]);
    }
    let out = cmd.output().expect("run cargo tree");
    assert!(out.status.success(), "cargo tree failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn regression_the_default_web_graph_links_no_wasm_bindgen() {
    let tree = wasm32_tree(&[]);
    for banned in ["wasm-bindgen ", "web-sys ", "js-sys "] {
        assert!(!tree.contains(banned), "canvas-native's default wasm32 graph has {banned}:\n{tree}");
    }
    // The feature canvas-vello turns on still brings the seam in.
    let bridged = wasm32_tree(&["web-sys-canvas"]);
    assert!(bridged.contains("web-sys "), "{bridged}");
}
