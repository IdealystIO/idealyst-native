//! Golden sequences shared by the host tests (`tests/determinism.rs`) and
//! the browser tests (`tests/web_random.rs`): the same constants passing on
//! both is what "the same sequence on every platform" means.
//!
//! Computed from the published reference algorithms (xoshiro256++ by
//! Blackman & Vigna, SplitMix64), independently of this crate's code.

/// `Rng::from_state([1, 2, 3, 4])` — the reference implementation's
/// first outputs.
pub const STATE_1234: [u64; 4] = [41943041, 58720359, 3588806011781223, 3591011842654386];

/// `Rng::seeded(0)` — state expanded by SplitMix64 from seed 0.
pub const SEEDED_0: [u64; 5] = [
    0x53175d61490b23df,
    0x61da6f3dc380d507,
    0x5c0fdf91ec9a7bfc,
    0x02eebf8c3bbe5e1a,
    0x7eca04ebaf4a5eea,
];

/// `Rng::seeded(42)`.
pub const SEEDED_42: [u64; 5] = [
    0xd0764d4f4476689f,
    0x519e4174576f3791,
    0xfbe07cfb0c24ed8c,
    0xb37d9f600cd835b8,
    0xcb231c3874846a73,
];

pub fn first<const N: usize>(mut rng: random::Rng) -> [u64; N] {
    std::array::from_fn(|_| rng.next_u64())
}
