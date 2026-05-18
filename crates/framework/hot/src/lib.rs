//! Hot-reload runtime — thin facade over [`subsecond`] under our own
//! `hot::*` naming so the rest of the framework doesn't grow a hard
//! dependency on the upstream API surface.
//!
//! # Design goals
//!
//! - **Zero production cost.** When the `hot` feature is off, every
//!   public function in this crate degrades to a direct call. The
//!   `#[component]` macro's hot-reload wrapper compiles out
//!   entirely. Production binaries pay nothing for the dev-only
//!   substrate.
//! - **Easy removal.** Every cross-crate consumer references this
//!   crate via the `hot-reload` cargo feature and gates integration
//!   on `#[cfg(feature = "hot-reload")]`. Toggling the feature
//!   workspace-wide removes every reference; this crate can then be
//!   deleted in one PR with no other code edits.
//! - **Platform-agnostic.** Built for the AAS dylib host today, but
//!   the same wrappers + jump-table protocol apply when the user's
//!   reactive runtime lives inside a native dev build (Android, iOS,
//!   eventually). The transport for delivering patches differs
//!   (in-process dlopen vs. WebSocket-shipped dylib that the device
//!   dlopens locally) — [`apply_patch`] is the same call regardless.
//!
//! # Two modes
//!
//! - **Off (`default`)**: [`call`] is `f(args)`. [`apply_patch`] is
//!   a no-op. [`HotFnPanic`] is a unit type that nothing ever
//!   constructs.
//! - **On (`hot` feature)**: [`call`] wraps the inner function in
//!   `subsecond::HotFn::current(...).call(args)`, going through the
//!   global jump table. [`apply_patch`] installs a new jump table.
//!   A [`HotFnPanic`] from a stale call site unwinds up to the
//!   nearest `catch_unwind` boundary in `framework_core::render`.
//!
//! # Macro emission shape
//!
//! `#[component]` (under `hot-reload`) rewrites
//!
//! ```ignore
//! fn Counter(props: &CounterProps) -> Primitive { /* body */ }
//! ```
//!
//! into
//!
//! ```ignore
//! fn Counter(props: &CounterProps) -> Primitive {
//!     ::framework_hot::call(__Counter_hot_impl, (props,))
//! }
//! #[doc(hidden)]
//! fn __Counter_hot_impl(props: &CounterProps) -> Primitive { /* body */ }
//! ```
//!
//! Without the feature, no wrapper is generated — `Counter` is
//! emitted unchanged.

#![cfg_attr(not(feature = "hot"), allow(unused_imports, dead_code))]

#[cfg(feature = "hot")]
pub use subsecond::{HotFn, HotFnPanic, JumpTable, PatchError};

/// Dispatch `f(args)` through the global hot-reload jump table when
/// the `hot` feature is on; otherwise call `f(args)` directly. The
/// generic constraint matches subsecond's `HotFunction` trait so the
/// signature is compatible with components of any arity.
///
/// Callers shouldn't invoke this directly — `#[component]` emits the
/// call site automatically. It's `pub` only so macro expansion can
/// reach it across crates.
#[cfg(feature = "hot")]
#[doc(hidden)]
#[inline]
pub fn call<Args, F, M>(f: F, args: Args) -> F::Return
where
    F: subsecond::HotFunction<Args, M>,
{
    debug_log_call::<F>(&f);
    subsecond::HotFn::current(f).call(args)
}

#[cfg(feature = "hot")]
fn debug_log_call<F>(f: &F) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let n = CALLS.fetch_add(1, Ordering::Relaxed);
    // Log only the first call of each session so we don't spam the
    // tick loop. Prints the runtime fn-ptr value and `cfg!
    // (debug_assertions)` so we can correlate against subsecond's
    // gating.
    if n < 3 {
        let size = std::mem::size_of::<F>();
        let ptr: usize = if size == std::mem::size_of::<fn()>() {
            unsafe { std::mem::transmute_copy::<F, usize>(f) }
        } else {
            0
        };
        eprintln!(
            "[framework-hot] call #{} F_size={} ptr=0x{:x} debug_assertions={}",
            n, size, ptr, cfg!(debug_assertions),
        );
    }
}

/// Pass-through stub for the feature-off mode. Monomorphizes to a
/// direct call; no jump-table code is even linked in. Production
/// builds get exactly what they'd have without the macro touching
/// the function.
#[cfg(not(feature = "hot"))]
#[doc(hidden)]
#[inline(always)]
pub fn call<Args, F>(f: F, args: Args) -> <F as DirectCall<Args>>::Return
where
    F: DirectCall<Args>,
{
    f.direct_call(args)
}

/// Shim trait for the feature-off path so [`call`] accepts the same
/// `(fn_ptr, args_tuple)` shape regardless of arity. Mirrors
/// `subsecond::HotFunction` minus the jump-table dispatch.
#[cfg(not(feature = "hot"))]
#[doc(hidden)]
pub trait DirectCall<Args> {
    type Return;
    fn direct_call(self, args: Args) -> Self::Return;
}

#[cfg(not(feature = "hot"))]
macro_rules! impl_direct_call {
    ($($arg:ident),*) => {
        impl<Func, Ret, $($arg,)*> DirectCall<($($arg,)*)> for Func
        where
            Func: FnOnce($($arg,)*) -> Ret,
        {
            type Return = Ret;
            #[inline(always)]
            fn direct_call(self, args: ($($arg,)*)) -> Ret {
                #[allow(non_snake_case)]
                let ($($arg,)*) = args;
                self($($arg,)*)
            }
        }
    };
}

#[cfg(not(feature = "hot"))]
impl_direct_call!();
#[cfg(not(feature = "hot"))]
impl_direct_call!(A);
#[cfg(not(feature = "hot"))]
impl_direct_call!(A, B);
#[cfg(not(feature = "hot"))]
impl_direct_call!(A, B, C);
#[cfg(not(feature = "hot"))]
impl_direct_call!(A, B, C, D);
#[cfg(not(feature = "hot"))]
impl_direct_call!(A, B, C, D, E);
#[cfg(not(feature = "hot"))]
impl_direct_call!(A, B, C, D, E, F);
#[cfg(not(feature = "hot"))]
impl_direct_call!(A, B, C, D, E, F, G);
#[cfg(not(feature = "hot"))]
impl_direct_call!(A, B, C, D, E, F, G, H);

/// Install a new jump table built from a freshly-rebuilt user
/// dylib. Each entry maps an old function address (the one statically
/// linked into the running host) to a new address inside the loaded
/// patch dylib. After this call returns, every subsequent [`call`]
/// dispatching one of those addresses lands in the patch.
///
/// In feature-off mode this is a no-op.
///
/// # Safety
///
/// Documented `unsafe` upstream because the table is trusted to
/// contain valid function pointers with matching signatures. The
/// framework's symbol-diff generator [`diff`] only emits entries
/// for functions with identical mangled names, so signatures match
/// by construction.
#[cfg(feature = "hot")]
pub unsafe fn apply_patch(table: JumpTable) -> Result<(), PatchError> {
    subsecond::apply_patch(table)
}

#[cfg(not(feature = "hot"))]
pub unsafe fn apply_patch(_table: ()) -> Result<(), ()> {
    Ok(())
}

/// Register a callback that fires after every successful
/// [`apply_patch`]. The framework's render loop uses this to
/// schedule a re-render at the next idle tick — without it, the
/// patched components only take effect on the next signal change.
///
/// In feature-off mode the callback is silently dropped.
#[cfg(feature = "hot")]
pub fn register_handler(f: fn()) {
    use std::sync::Arc;
    subsecond::register_handler(Arc::new(f))
}

#[cfg(not(feature = "hot"))]
pub fn register_handler(_f: fn()) {}

/// Symbol-diff jump-table generator. After every successful
/// rebuild of the patch dylib, the AAS host calls
/// [`diff::apply_from_dylib`] which:
///
/// 1. Opens the running binary (via `std::env::current_exe`) and
///    parses its symbol table.
/// 2. Opens the freshly-built patch dylib and parses its symbol
///    table.
/// 3. For every `__*_hot_impl` symbol (and `main`, used as the
///    ASLR reference), records the (bin link-time offset, dylib
///    link-time offset) pair.
/// 4. Constructs a [`JumpTable`] and hands it to
///    [`apply_patch`].
#[cfg(feature = "diff")]
pub mod diff;
