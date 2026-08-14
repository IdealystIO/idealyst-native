//! Selectable global allocators for the wasm bundle.
//!
//! An app names the one it wants in its own manifest:
//!
//! ```toml
//! [package.metadata.idealyst.app]
//! allocator = "small"
//! ```
//!
//! [`entry!`](macro@crate::entry) reads that key and emits the matching
//! `#[global_allocator]` into the app's `main.rs`. The key is web-only:
//! native shells use the system allocator and ignore it.
//!
//! | value | allocator | shape |
//! | --- | --- | --- |
//! | absent / `"default"` | `std`'s wasm32 default, `dlmalloc` | size-binned, roughly O(1) for the common case |
//! | `"small"` | [`Small`] — `lol_alloc`'s `FreeListAllocator` | ~10KB less code, but every allocation walks a free list |
//!
//! # Picking one
//!
//! `"default"` is the default, and that is the important half.
//! `"small"` was unconditional up to 1.3.7 on the reasoning that it cost
//! "a few cycles per allocation in exchange for a few KB off the
//! bundle". The second half was true and the first was not: a free list
//! is a **linear scan**, so its cost is O(list length) and rises with
//! fragmentation — exactly what a UI runtime produces when it mounts a
//! subtree, tears it down, and mounts a differently-shaped one.
//!
//! Measured in CrewForge's schedule grid (~1400 cells re-sliced per
//! scroll step, debug wasm): **62% of a scroll frame** inside
//! `FreeListAllocator`, and still 44% after two rounds of cutting the
//! app's own hot path. Nothing in app code came within an order of
//! magnitude.
//!
//! So `"small"` is the right trade for a bundle that is genuinely
//! size-bound and mostly static, and the wrong one for anything that
//! allocates on a frame. Choose it against a profile, not against
//! intuition.
//!
//! # Why manifest metadata and not a cargo feature
//!
//! A `#[global_allocator]` is process-wide — one per binary, no per-app
//! override. Cargo features are additive and unify across a workspace,
//! so selecting the allocator with a feature would mean **one** crate
//! asking for the small allocator silently gives it to every app in that
//! workspace, with nothing to reject it at compile time. Manifest
//! metadata is read from the crate that owns `main`, which is exactly
//! the granularity a global allocator has.
//!
//! # Without `entry!`
//!
//! A hand-written `main` (the escape hatch documented on
//! [`entry!`](macro@crate::entry)) declares the static itself — the
//! macro has no privileged access here:
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOCATOR: idealyst::alloc::Small = idealyst::alloc::small();
//! ```

/// `lol_alloc`'s free-list allocator, with the single-threaded
/// assumption its `GlobalAlloc` impl needs.
///
/// Construct with [`small`] rather than by hand: the `AssumeSingleThreaded`
/// constructor is `unsafe`, and [`small`] is the one place that carries
/// the safety argument.
pub type Small = lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator>;

/// The [`Small`] allocator, ready to install as a `#[global_allocator]`.
///
/// `const` so it can initialize a `static`, which is the only position a
/// `#[global_allocator]` may appear in.
pub const fn small() -> Small {
    // SAFETY: `AssumeSingleThreaded` asserts that no two threads ever
    // reach this allocator concurrently. That holds on the wasm32
    // targets this crate builds for: linear memory is per-instance, and
    // a Web Worker instantiates its own module with its own memory
    // rather than sharing this one. It would stop holding under wasm
    // threads + `SharedArrayBuffer` memory, which the framework does not
    // build for — the `"default"` allocator carries no such assumption
    // and is what an app on that path must use.
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) }
}
