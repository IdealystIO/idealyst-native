//! Per-position ids for [`use_id`]: ids for markup that needs them
//! (`<label for=…>`, `aria-describedby`), unique in a tree and the same on
//! every build of the same tree — so a server render and the client that
//! hydrates it agree.
//!
//! A *frame* is the identity of one build site plus a counter of the
//! seats taken in it. Every [`component_scope`](crate::component_scope)
//! takes the next seat of the frame it is built in and opens a frame of
//! its own for its body; [`use_id`] takes a seat too. A structural hole
//! ([`dyn_element`](crate::dyn_element), [`dyn_keyed`](crate::dyn_keyed),
//! [`keyed`](crate::keyed)) takes its seat when it is constructed and
//! re-opens a frame under it for each build, because a hole builds later,
//! from the driver, outside the frame it was written in: a keyed row's
//! frame comes from its key (a row keeps its ids when the list reorders),
//! a hole's from its build count (the first build is the same on every
//! run; a rebuild gets fresh ids, never a previous build's).
//!
//! Outside any frame, seats come from a counter that lives in the world
//! ([`runtime_world::world_local`]), so each world — each server request —
//! counts from zero, and the counter goes when the world does. With no
//! world entered (or in a remote bundle the host entered), one counter per
//! thread stands in.

use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

/// The seed a world's root frame starts from (the first 64 bits of the
/// fractional part of √2, a nothing-up-my-sleeve constant).
const ROOT_SEED: u64 = 0x6A09_E667_F3BC_C908;

#[derive(Clone, Copy)]
struct Frame {
    seed: u64,
    next: u64,
}

runtime_tls::thread_local! {
    /// The frame being built in, if any.
    static FRAME: Cell<Option<Frame>> = const { Cell::new(None) };
    /// Seats taken outside every frame with no world to count them in.
    static WORLDLESS_SEATS: Cell<u64> = const { Cell::new(0) };
}

/// A world's count of the seats taken outside every frame.
#[derive(Clone)]
struct RootSeats(Rc<Cell<u64>>);

/// SplitMix64's finalizer: a cheap, well-mixed 64-bit permutation.
fn scramble(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

pub(crate) fn mix(seed: u64, n: u64) -> u64 {
    scramble(seed ^ scramble(n.wrapping_add(0x9E37_79B9_7F4A_7C15)))
}

/// The frame seed of a keyed row under its list's `seat`.
pub(crate) fn key_seed(seat: u64, key: &crate::Key) -> u64 {
    let mut h = rustc_hash::FxHasher::default();
    key.hash(&mut h);
    mix(seat, h.finish())
}

/// Take the next seat of the current frame (or of this world's root).
pub(crate) fn take_seat() -> u64 {
    if let Some(mut frame) = FRAME.with(|f| f.get()) {
        let seat = mix(frame.seed, frame.next);
        frame.next += 1;
        FRAME.with(|f| f.set(Some(frame)));
        return seat;
    }
    let n = match runtime_world::world_local(|| RootSeats(Rc::new(Cell::new(0)))) {
        Some(RootSeats(count)) => count.replace(count.get() + 1),
        None => WORLDLESS_SEATS.with(|c| c.replace(c.get() + 1)),
    };
    mix(ROOT_SEED, n)
}

/// Run `f` in a fresh frame seeded `seed`, restoring the enclosing frame
/// afterwards (panic-safe).
pub(crate) fn with_frame<R>(seed: u64, f: impl FnOnce() -> R) -> R {
    let previous = FRAME.with(|c| c.replace(Some(Frame { seed, next: 0 })));
    struct Restore(Option<Frame>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = FRAME.try_with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(previous);
    f()
}

/// An id unique in this tree and stable across builds of the same tree:
/// `"ui-<16 hex digits>"`. Each call takes its own seat, so two calls in
/// one component answer two ids.
pub fn use_id() -> String {
    format!("ui-{:016x}", take_seat())
}

/// [`use_id`] mixed with `key`, without taking a seat: the same key in
/// the same component answers the same id however many other ids it
/// asked for first.
pub fn use_id_keyed<K: Hash + ?Sized>(key: &K) -> String {
    let seed = FRAME.with(|f| f.get()).map_or(ROOT_SEED, |f| f.seed);
    let mut h = rustc_hash::FxHasher::default();
    key.hash(&mut h);
    format!("ui-{:016x}", mix(seed, h.finish()))
}
