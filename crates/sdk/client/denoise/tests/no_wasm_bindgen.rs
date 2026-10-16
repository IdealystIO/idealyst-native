//! denoise's web graph carries no wasm-bindgen.
//!
//! tract pulls in getrandom 0.2, which has no default wasm backend; its `js`
//! backend links wasm-bindgen and would put every app using this SDK into
//! the hybrid build (docs/proposals/own-web-bindings.md). `random`'s
//! `getrandom-0-2` supplies the backend instead. This pins it.
#![cfg(not(target_arch = "wasm32"))]

use std::process::Command;

#[test]
fn regression_the_web_graph_links_no_wasm_bindgen() {
    let out = Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["tree", "-p", "denoise", "--target", "wasm32-unknown-unknown", "-e", "normal", "--offline"])
        .output()
        .expect("run cargo tree");
    assert!(out.status.success(), "cargo tree failed: {}", String::from_utf8_lossy(&out.stderr));
    let tree = String::from_utf8(out.stdout).unwrap();
    for banned in ["wasm-bindgen ", "web-sys ", "js-sys "] {
        assert!(!tree.contains(banned), "denoise's wasm32 graph has {banned}:\n{tree}");
    }
}
