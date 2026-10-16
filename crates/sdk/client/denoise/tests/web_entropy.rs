//! tract (rand 0.8 → getrandom 0.2) draws entropy in the browser through
//! `random`'s web-glue backend, not getrandom's wasm-bindgen `js` backend.
//!
//! `cargo test -p denoise --target wasm32-unknown-unknown --test web_entropy`
#![cfg(target_arch = "wasm32")]

// Naming denoise links it, and through its `use random as _;` the getrandom
// registration — what an app using this SDK gets.
use denoise as _;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn getrandom_0_2_resolves_to_the_web_glue_backend() {
    let mut a = [0u8; 32];
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut a).expect("getrandom 0.2 has a backend in denoise's graph");
    getrandom::getrandom(&mut b).unwrap();
    assert_ne!(a, b);
}
