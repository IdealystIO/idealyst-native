//! Random numbers, the same on every platform.
//!
//! Two layers:
//!
//! - **Entropy — the OS.** [`secure_bytes`] and [`secure_u64`] read the
//!   operating system's cryptographic source: `getentropy` on Apple,
//!   `getrandom(2)` on Linux and Android, `ProcessPrng` on Windows,
//!   `crypto.getRandomValues` on web. Use these for anything an attacker
//!   must not guess: tokens, ids, keys, nonces.
//! - **Generation — this crate.** [`Rng`] is xoshiro256++, implemented here
//!   in plain integer arithmetic, and every derived operation (ranges,
//!   floats, shuffle, choose) is ours too. So [`Rng::seeded`] produces the
//!   identical sequence on every backend, and code that draws from it
//!   behaves the same everywhere. Platform helpers (`Math.random`,
//!   `arc4random_uniform`, `java.util.Random`) differ in algorithm and in how
//!   they map bits to ranges, and none can be seeded across platforms —
//!   which is why the generator is not delegated.
//!
//! The free functions ([`range`], [`float`], [`shuffle`], …) draw from a
//! per-thread [`Rng`] seeded from OS entropy on first use. [`seed`] replaces
//! it with a seeded one — for tests, replays, or reproducible content.
//!
//! [`Rng`] is not for secrets: its output is predictable from earlier
//! output.
//!
//! ```
//! let roll = random::range(1..=6);
//! assert!((1..=6).contains(&roll));
//!
//! let mut deck: Vec<u8> = (0..52).collect();
//! random::shuffle(&mut deck);
//!
//! let mut token = [0u8; 16];
//! random::secure_bytes(&mut token); // OS entropy
//! ```

#![deny(missing_docs)]

use std::cell::RefCell;

mod rng;
pub use rng::{Rng, SampleRange};

// Compile-checked usage recipes (catalog feature only).
#[cfg(feature = "catalog")]
pub mod recipes;

#[cfg(target_arch = "wasm32")]
#[path = "entropy/web.rs"]
mod entropy;
#[cfg(not(target_arch = "wasm32"))]
#[path = "entropy/native.rs"]
mod entropy;

/// `getrandom` 0.2's custom backend on wasm32, fed by [`secure_bytes`] (the
/// `getrandom-0-2` feature). The macro defines one global `#[no_mangle]`
/// symbol, which is why exactly one crate — this one — may register it.
/// Native targets ignore a custom backend and use the OS directly.
///
/// A crate enabling the feature must also name this crate (`use random as
/// _;`): rustc links only dependencies something references, and an
/// unlinked registration leaves `__getrandom_custom` undefined at link time.
#[cfg(all(feature = "getrandom-0-2", target_arch = "wasm32", target_os = "unknown"))]
mod getrandom02_backend {
    fn fill(buf: &mut [u8]) -> Result<(), getrandom02::Error> {
        crate::secure_bytes(buf);
        Ok(())
    }
    getrandom02::register_custom_getrandom!(fill);
}

/// Fill `buf` from the OS cryptographic entropy source.
///
/// Panics if the source fails — it cannot on any supported platform (every
/// browser, window and worker alike, exposes `crypto.getRandomValues`), and a
/// silent fallback to weaker randomness would be worse than a crash.
pub fn secure_bytes(buf: &mut [u8]) {
    entropy::fill(buf);
}

/// A `u64` from the OS cryptographic entropy source. See [`secure_bytes`].
pub fn secure_u64() -> u64 {
    let mut b = [0u8; 8];
    secure_bytes(&mut b);
    u64::from_le_bytes(b)
}

thread_local! {
    static THREAD_RNG: RefCell<Option<Rng>> = const { RefCell::new(None) };
}

/// Run `f` with this thread's generator, seeding it from OS entropy on
/// first use. The borrow is held only for `f`, which never re-enters.
fn with_thread_rng<R>(f: impl FnOnce(&mut Rng) -> R) -> R {
    THREAD_RNG.with(|cell| f(cell.borrow_mut().get_or_insert_with(Rng::from_entropy)))
}

/// Replace this thread's generator with [`Rng::seeded`]`(seed)`, making the
/// free functions below deterministic — the same draws on every platform.
pub fn seed(seed: u64) {
    THREAD_RNG.with(|cell| *cell.borrow_mut() = Some(Rng::seeded(seed)));
}

/// The next 64 random bits from this thread's generator.
pub fn u64() -> u64 {
    with_thread_rng(Rng::next_u64)
}

/// A uniform `f64` in `[0, 1)`. See [`Rng::float`].
pub fn float() -> f64 {
    with_thread_rng(Rng::float)
}

/// A fair coin flip.
pub fn bool() -> bool {
    with_thread_rng(Rng::bool)
}

/// `true` with probability `p`. See [`Rng::chance`].
pub fn chance(p: f64) -> bool {
    with_thread_rng(|rng| rng.chance(p))
}

/// A uniform value in `range` (`a..b` or `a..=b`). See [`Rng::range`].
pub fn range<T, R: SampleRange<T>>(range: R) -> T {
    with_thread_rng(|rng| rng.range(range))
}

/// Shuffle `items` in place. See [`Rng::shuffle`].
pub fn shuffle<T>(items: &mut [T]) {
    with_thread_rng(|rng| rng.shuffle(items))
}

/// A uniformly chosen element, or `None` for an empty slice.
pub fn choose<T>(items: &[T]) -> Option<&T> {
    with_thread_rng(|rng| rng.choose(items))
}
