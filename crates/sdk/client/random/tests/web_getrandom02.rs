//! `getrandom-0-2`: getrandom 0.2 on wasm32 draws from this crate's
//! web-glue entropy — registered from this library crate, not the app.
//!
//! `cargo test -p random --features getrandom-0-2 --target wasm32-unknown-unknown`
#![cfg(all(target_arch = "wasm32", feature = "getrandom-0-2"))]

// rustc links a dependency only when something names it; without this the
// registration in `random` never reaches the linker and getrandom's
// `__getrandom_custom` stays undefined. Consumers do the same.
extern crate random;

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn getrandom_0_2_is_backed_by_web_glue_entropy() {
    let mut a = [0u8; 64];
    let mut b = [0u8; 64];
    getrandom02::getrandom(&mut a).expect("getrandom 0.2 reaches the registered backend");
    getrandom02::getrandom(&mut b).unwrap();
    assert_ne!(a, [0u8; 64]);
    assert_ne!(a, b);
}
