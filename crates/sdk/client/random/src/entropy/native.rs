//! Native entropy through `getrandom`: `getentropy` on Apple, the
//! `getrandom(2)` syscall on Linux and Android, `ProcessPrng` /
//! `BCryptGenRandom` on Windows.

pub(crate) fn fill(buf: &mut [u8]) {
    if let Err(e) = getrandom::fill(buf) {
        panic!("random: the OS entropy source failed: {e}");
    }
}
