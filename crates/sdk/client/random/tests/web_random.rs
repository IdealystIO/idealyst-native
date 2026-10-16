//! Browser tests — `cargo test -p random --target wasm32-unknown-unknown`
//! (the workspace runner supplies web-glue's JS).
#![cfg(target_arch = "wasm32")]

mod golden;

use random::Rng;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

/// The same constants as the host tests: a seed yields the identical
/// sequence in the browser (32-bit wasm) as on 64-bit native.
#[wasm_bindgen_test]
fn seeded_sequences_match_the_host_goldens() {
    assert_eq!(golden::first(Rng::from_state([1, 2, 3, 4])), golden::STATE_1234);
    assert_eq!(golden::first(Rng::seeded(0)), golden::SEEDED_0);
    assert_eq!(golden::first(Rng::seeded(42)), golden::SEEDED_42);
    random::seed(42);
    assert_eq!(random::u64(), golden::SEEDED_42[0]);
}

#[wasm_bindgen_test]
fn secure_bytes_come_from_crypto_get_random_values() {
    let mut a = [0u8; 64];
    let mut b = [0u8; 64];
    random::secure_bytes(&mut a);
    random::secure_bytes(&mut b);
    assert_ne!(a, [0u8; 64]);
    assert_ne!(a, b);
}

/// `getRandomValues` refuses more than 65,536 bytes per call; the binding
/// chunks, so a larger buffer is filled end to end.
#[wasm_bindgen_test]
fn secure_bytes_fill_buffers_larger_than_one_get_random_values_call() {
    let mut big = vec![0u8; 200_000];
    random::secure_bytes(&mut big);
    for window in big.chunks(65_536) {
        assert!(window.iter().any(|&b| b != 0), "a chunk was left unfilled");
    }
}
