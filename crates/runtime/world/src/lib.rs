//! runtime-world — the reactive kernel for the idea-lite core migration (P0).
//!
//! Core model (idea-lite's three axioms, on arena storage):
//! - **Signals stage writes** (`set`/`update`) and commit them in batches at
//!   [`World::flush`]; reads (`get`/`with`) always see the committed snapshot.
//! - **Effects auto-track** the signals they read and re-run when those
//!   effectively change. Cleanups run before each re-run and at teardown.
//! - **Dropping is the entire teardown story**: slots created inside
//!   [`collect_owned`] are freed when the returned [`Owned`] drops; everything
//!   else is freed when its [`World`] drops.
//!
//! Storage model — per-world arenas, Copy handles:
//! - Each [`World`] owns an arena of generational signal/effect slots. Handles
//!   ([`Signal`], [`ReadSignal`], [`WriteSignal`], [`Effect`], [`Memo`]) are
//!   `Copy` triples `(world id, slot, generation)` — the authored tree passes
//!   them around with zero clone ceremony.
//! - ONE `thread_local!` total holds the registry `world id → arena` plus all
//!   transient stacks (ambient enter stack, running-effect stack, ownership
//!   collectors). A single TLS key matters on Android, where bionic caps
//!   pthread TLS keys at 128 (see the runtime-core stylesheet-registry
//!   incident).
//! - Handles route to their OWN world through the registry at use time, not
//!   the ambient world: writing an A-world signal from inside a B-world
//!   effect stages into A's queue; A and B flush independently.
//! - Slots are **generational**: freeing a slot bumps its generation, so a
//!   stale `Copy` handle (its `Owned` scope dropped) can never alias the
//!   slot's next occupant — it panics with a diagnostic instead. Generations
//!   are `u32` and wrap; ABA after 2^32 reuses of one slot is accepted.
//!
//! Flush model — derivation-class, glitch-free (the P0 centerpiece): see
//! [`World::flush`] for the algorithm and its glitch-freedom argument.
//!
//! Engine seam: this file is the TYPED layer (handles, `SignalData<T>`, memo,
//! `Value`, ownership's public surface). Everything untyped — slots, staging,
//! the flush, collectors, context stacks — sits behind the `Engine` trait in
//! `engine.rs`, implemented by the in-process arena in `native.rs`. The seam
//! exists so a second engine can run the same typed layer: remote components
//! (crates/streaming) bridge a wasm bundle's kernel onto the host app's arena.
//! A change to the kernel's contract is a change to `Engine`, which every
//! engine must then implement before the crate builds.
//!
//! The second engine is the bridge (`bridge/`): it keeps values and closures
//! on its side and forwards the graph to a native host. Its behavioural
//! parity is checked by running this crate's whole suite — and the suites
//! built on it — through it, in-process:
//!
//! ```sh
//! cargo test -p runtime-world --features loopback-engine
//! cargo test -p runtime-scene -p runtime-vocabulary --features runtime-world/loopback-engine
//! ```
//!
//! A kernel change is done when both the native and the loopback runs pass.
//!
//! Dev diagnostics — staged-read warning: staging makes `set(v); get()` in
//! one turn return the PRE-set value, the one 0.5 → 1.0 break that is
//! neither a compile error nor a panic. Debug builds warn once per call
//! site when a read lands on a signal with a pending staged write whose
//! value differs from the committed one; see
//! [`install_diagnostic_sink`] and the "staged-read diagnostic" section
//! below. Compiled out entirely in release.

use std::any::{Any, TypeId};
// Only the debug-build diagnostic table (and the tests) need it here; the
// engine's cells live in `native.rs`.
#[cfg(any(debug_assertions, test))]
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

#[cfg(test)]
mod tests;

// The state carrier reads NATIVE slots; under the bridged engine (the
// loopback test configuration, or a remote bundle) the active engine's
// slots are proxies, so the combination would carry garbage. Documented as
// unsupported in Cargo.toml; enforced here.
#[cfg(all(feature = "hot-reload", any(feature = "loopback-engine", idealyst_stream_guest)))]
compile_error!(
    "runtime-world's `hot-reload` can't be combined with the bridged engine (`loopback-engine`, or a remote \
     bundle build): hot reload's state carry reads native slots"
);

/// Dev-time hot reload: carrying signal VALUES across an in-process
/// re-run of a patched tree. Entirely behind the `hot-reload` feature;
/// see its module docs for why values move rather than handles.
#[cfg(feature = "hot-reload")]
pub mod hot_state;

/// Panic with an actionable diagnostic in dev builds and a terse stable code
/// in release builds (same convention as runtime-core: long prose ships in
/// wasm rodata, so it is compiled out of release; the slug keeps the failure
/// greppable and `#[should_panic(expected = "idealyst[<slug>]")]` stable in
/// both modes).
macro_rules! diag_panic {
    ($code:literal, $fmt:literal $(, $arg:expr)* $(,)?) => {{
        #[cfg(debug_assertions)]
        { panic!(concat!("idealyst[", $code, "]: ", $fmt) $(, $arg)*) }
        #[cfg(not(debug_assertions))]
        {
            $(let _ = &$arg;)* // args feed the dev-only prose; consume them here
            panic!(concat!("idealyst[", $code, "] (debug build has details)"))
        }
    }};
}


// The engine seam (see `engine.rs`): the typed layer below reaches the
// kernel's untyped half only through `Active`. Declared after `diag_panic!`
// so the engine can use it.
mod engine;
mod native;
// The bridged engine (remote components, `bridge/mod.rs`). Compiled wherever
// it is used — the `bridge` feature (remote-component hosts and bundles), the
// loopback test configuration — AND in this crate's own test build, so the
// compile-time parity check below runs on every `cargo test -p runtime-world`:
// a change to the `Engine` contract fails there until the bridge implements
// it too.
//
// NOT compiled into apps that don't use it. Measured: merely having the
// bridge's (unused) code in the crate shifted the size optimizer's outlining
// on hot paths in the web release profile (opt "z", no LTO) — fan-out +3.3%
// with it, -2.0% without.
#[cfg(any(test, all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)), feature = "loopback-engine", idealyst_stream_guest))]
#[cfg_attr(not(any(all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)), feature = "loopback-engine", idealyst_stream_guest)), allow(dead_code))]
mod bridge;

use engine::{Access, AnySignal, EffectClass, Engine, WorldId};

/// The engine this build runs. Every app runs the native arena; the
/// `loopback-engine` feature runs the whole crate through the bridge instead
/// (a test configuration — see `bridge/mod.rs`).
///
/// A remote bundle (`--cfg idealyst_stream_guest`, set by the bundle build)
/// runs the bridged engine over its wasm imports: the bundle's kernel lives
/// on the host app's graph.
#[cfg(not(any(feature = "loopback-engine", idealyst_stream_guest)))]
type Active = native::Native;
#[cfg(all(feature = "loopback-engine", not(idealyst_stream_guest)))]
type Active = bridge::Loopback;
#[cfg(idealyst_stream_guest)]
type Active = bridge::guest::Bridged<bridge::wasm::Imports>;

/// Typed halves of the bridge's host-owned-value machinery: the sync that
/// mirrors an imported value in a bundle, and the exporter that serves a
/// host signal to bundles. Here because both need `T` and `SignalData<T>`.
#[cfg(any(test, all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)), feature = "loopback-engine", idealyst_stream_guest))]
#[cfg_attr(not(any(all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)), feature = "loopback-engine", idealyst_stream_guest)), allow(dead_code))]
mod bridge_typed {
    use std::rc::Rc;

    use super::*;
    use crate::bridge::{ImportSync, StageMode};
    use crate::native::Native;

    /// How a value of type `T` crosses the boundary. Plain function
    /// pointers, so runtime-world stays codec-free: the remote-component
    /// crates supply them (the vocabulary's `RemoteValue`, or
    /// `remote_abi::Wire` in the hand-written kernel-bridge code).
    pub struct Codec<T> {
        pub encode: fn(&T, &mut Vec<u8>),
        pub decode: fn(&[u8]) -> Option<T>,
    }

    impl<T> Clone for Codec<T> {
        fn clone(&self) -> Self {
            *self
        }
    }
    impl<T> Copy for Codec<T> {}

    /// Mirrors an imported value through its codec (bundle side).
    #[cfg_attr(not(any(idealyst_stream_guest, feature = "loopback-engine")), allow(dead_code))]
    pub(crate) struct TypedSync<T>(pub(crate) Codec<T>);

    impl<T: PartialEq + 'static> ImportSync for TypedSync<T> {
        fn pull(&self, mirror: &mut dyn AnySignal, committed: &[u8], staged: Option<&[u8]>) {
            let d = typed::<T>(mirror);
            d.value = (self.0.decode)(committed).expect("kernel bridge: host value does not decode");
            d.next = staged.map(|b| (self.0.decode)(b).expect("kernel bridge: host staged value does not decode"));
        }
        fn take_next(&self, mirror: &mut dyn AnySignal, out: &mut Vec<u8>) -> bool {
            match typed::<T>(mirror).next.take() {
                Some(next) => {
                    (self.0.encode)(&next, out);
                    true
                }
                None => false,
            }
        }
        fn encode_value(&self, mirror: &mut dyn AnySignal, out: &mut Vec<u8>) {
            (self.0.encode)(&typed::<T>(mirror).value, out)
        }
        fn encode_next(&self, mirror: &mut dyn AnySignal, out: &mut Vec<u8>) -> bool {
            match &typed::<T>(mirror).next {
                Some(next) => {
                    (self.0.encode)(next, out);
                    true
                }
                None => false,
            }
        }
    }

    /// Serves a host-owned signal to bundles (host side). Goes through the
    /// NATIVE engine directly rather than `Active`: the slot is native, and
    /// in the loopback configuration `Active` is the bridged engine.
    pub(crate) struct Exporter<T> {
        pub(crate) handle: (WorldId, u32, u32),
        pub(crate) codec: Codec<T>,
        pub(crate) writable: bool,
    }

    impl<T: PartialEq + 'static> crate::bridge::host::Exported for Exporter<T> {
        fn fetch(&self, staged: bool, out: &mut Vec<u8>) -> bool {
            let (w, s, g) = self.handle;
            // An export does not keep its slot alive (the guard can outlive
            // it — a re-export of a promoted slot whose scope dropped): a
            // dead slot answers "not exported", never a stale-handle panic
            // in the app on a bundle's say-so.
            if !Native::signal_is_alive(w, s, g) {
                return false;
            }
            match Native::signal_access(w, s, g, |d| {
                let d = typed::<T>(d);
                match (staged, &d.next) {
                    (false, _) => {
                        (self.codec.encode)(&d.value, out);
                        true
                    }
                    (true, Some(next)) => {
                        (self.codec.encode)(next, out);
                        true
                    }
                    (true, None) => false,
                }
            }) {
                Access::Done(found) => found,
                Access::DeadWorld => false,
            }
        }
        fn writable(&self) -> bool {
            self.writable
        }
        fn stage(&self, bytes: &[u8], mode: StageMode) -> Result<(), String> {
            if !self.writable {
                return Err(format!(
                    "kernel bridge: a bundle wrote a host signal it received read-only (world {}, slot {})",
                    self.handle.0, self.handle.1
                ));
            }
            if !Native::signal_is_alive(self.handle.0, self.handle.1, self.handle.2) {
                return Err(format!(
                    "kernel bridge: a bundle wrote host signal (world {}, slot {}), which no longer exists",
                    self.handle.0, self.handle.1
                ));
            }
            let value = (self.codec.decode)(bytes).ok_or_else(|| {
                format!("kernel bridge: a bundle's value for host signal (world {}, slot {}) does not decode", self.handle.0, self.handle.1)
            })?;
            let (w, s, g) = self.handle;
            let _ = match mode {
                StageMode::Set | StageMode::SetAlways => {
                    Native::signal_write(w, s, g, mode == StageMode::SetAlways, |d| typed::<T>(d).next = Some(value))
                }
                StageMode::Untracked => Native::signal_access(w, s, g, |d| typed::<T>(d).value = value),
            };
            Ok(())
        }
    }

    /// A bundle-owned value PROMOTED into the native arena (host side; see
    /// `remote::receive_signal`). The slot's storage is a real
    /// `SignalData<T>`: `as_any_mut` hands out `data` itself, so every
    /// native read, write and commit of the slot is the code an app signal
    /// runs — the promotion costs native paths nothing. What the wrapper
    /// adds is teardown: when the slot is freed it withdraws the bundle's
    /// export and releases the bundle's (now mirroring) entry.
    #[cfg(all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)))]
    #[cfg_attr(idealyst_stream_guest, allow(dead_code))]
    pub(crate) struct Promoted<T> {
        pub(crate) data: SignalData<T>,
        pub(crate) handle: (WorldId, u32, u32),
        pub(crate) bundle_value: crate::bridge::Id,
        pub(crate) release: fn(crate::bridge::Id),
    }

    #[cfg(all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)))]
    impl<T: PartialEq + 'static> AnySignal for Promoted<T> {
        fn commit(&mut self, forced: bool) -> bool {
            self.data.commit(forced)
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            &mut self.data
        }
        // A promoted value belongs to a remote component, which the app's
        // hot reload does not re-run: handing back the wrapper makes the
        // harvest's `SignalData<T>` downcast miss, so nothing is seeded.
        #[cfg(feature = "hot-reload")]
        fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
            self
        }
        #[cfg(feature = "hot-reload")]
        fn value_type_id(&self) -> std::any::TypeId {
            std::any::TypeId::of::<T>()
        }
    }

    #[cfg(all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)))]
    impl<T> Drop for Promoted<T> {
        fn drop(&mut self) {
            crate::bridge::host::unregister_export(self.handle);
            (self.release)(self.bundle_value);
        }
    }

    // Host side: used by `remote::export_*` (feature `bridge`).
    #[cfg_attr(not(all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest))), allow(dead_code))]
    pub(crate) fn exporter<T: PartialEq + 'static>(
        handle: (WorldId, u32, u32),
        codec: Codec<T>,
        writable: bool,
    ) -> Rc<dyn crate::bridge::host::Exported> {
        Rc::new(Exporter { handle, codec, writable })
    }
}

/// The host side of the kernel bridge, for a remote-component host (the
/// app that loads bundles): implement [`remote::GuestHooks`] for the
/// transport that reaches a bundle, serve the bundle's kernel calls with
/// [`remote::Host`]'s [`remote::HostOps`], and hand host state to bundles
/// with [`remote::export_signal`] / [`remote::export_read_signal`] (props)
/// and [`remote::export_context`] (context). See `bridge/mod.rs`.
#[cfg(all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)))]
pub mod remote {
    use std::rc::Rc;

    pub use crate::bridge::host::{ExportGuard, Host};
    pub use crate::bridge::{GuestHooks, Handle, HostOps, Id, StageMode};
    pub use crate::bridge_typed::Codec;
    pub use crate::engine::{EffectClass, WorldId};
    use crate::{ReadSignal, Signal};

    /// Let bundles read AND write `sig` (a two-way prop). Returns its handle
    /// — what the bundle imports — and a guard that withdraws it.
    pub fn export_signal<T: PartialEq + 'static>(sig: Signal<T>, codec: Codec<T>) -> (Handle, ExportGuard) {
        let h = (sig.world, sig.slot, sig.gen);
        (h, crate::bridge::host::register_export(h, crate::bridge_typed::exporter(h, codec, true)))
    }

    /// Let bundles read `sig` (a read-only prop; a write is refused).
    pub fn export_read_signal<T: PartialEq + 'static>(sig: ReadSignal<T>, codec: Codec<T>) -> (Handle, ExportGuard) {
        let h = (sig.world, sig.slot, sig.gen);
        (h, crate::bridge::host::register_export(h, crate::bridge_typed::exporter(h, codec, false)))
    }

    /// Declare host context to bundles under `name`: when a bundle's
    /// `inject` for a type it registered under that name finds nothing of
    /// its own, `fetch` encodes the host's value (return `false` for none).
    /// The allowlist — undeclared host context is invisible to bundles.
    pub fn export_context(name: &str, fetch: impl Fn(&mut Vec<u8>) -> bool + 'static) -> ExportGuard {
        crate::bridge::host::register_context(name, Rc::new(fetch))
    }

    /// The app's side of a signal a bundle hands to native code (a prop of
    /// an app component the bundle uses): a handle the app can read and
    /// write natively.
    ///
    /// A signal the app already owns (one it passed into the bundle, or
    /// already promoted) is returned as is. A signal the BUNDLE created is
    /// PROMOTED: its value moves out of the bundle into this slot as a real
    /// `SignalData<T>` — the app now owns the value, and the bundle's reads
    /// and writes go to it from here on (as for any app signal the bundle
    /// imported). Same slot, so subscribers and the bundle's handle stay
    /// valid. Requires the bundle to have offered it
    /// ([`remote_guest::offer_signal`](crate::remote_guest::offer_signal)).
    ///
    /// `Err` when the value does not decode as `T` (the bundle and the app
    /// disagree about the type), or the bundle did not offer it.
    #[cfg(not(idealyst_stream_guest))]
    pub fn receive_signal<T: PartialEq + 'static>(h: Handle, codec: Codec<T>) -> Result<Signal<T>, String> {
        use crate::bridge::host::ValueProxy;
        use crate::native::Native;
        use crate::engine::Engine;
        use crate::{AnySignal, PhantomData, SignalData};
        let (w, s, g) = h;
        let signal = Signal { world: w, slot: s, gen: g, _marker: PhantomData };
        let proxy = match Native::signal_access(w, s, g, |d| {
            let any = d.as_any_mut();
            if any.is::<SignalData<T>>() {
                return Ok(None);
            }
            match any.downcast_ref::<ValueProxy>() {
                Some(p) => Ok(Some((p.value, p.promote, p.promote_finish, p.drop_value))),
                None => Err("the app holds this signal with a different value type".to_string()),
            }
        }) {
            crate::engine::Access::Done(r) => r?,
            crate::engine::Access::DeadWorld => return Err("the signal's world is gone".into()),
        };
        let Some((id, promote, finish, release)) = proxy else { return Ok(signal) };
        let (mut committed, mut staged) = (Vec::new(), Vec::new());
        let has_staged = promote(id, &mut committed, &mut staged)
            .ok_or_else(|| "the bundle did not offer this signal for promotion".to_string())?;
        let decode = |b: &[u8], what: &str| {
            (codec.decode)(b).ok_or_else(|| format!("the bundle's {what} value does not decode as the app's type"))
        };
        let value = decode(&committed, "committed")?;
        let next = if has_staged { Some(decode(&staged, "staged")?) } else { None };
        let promoted: Box<dyn AnySignal> = Box::new(crate::bridge_typed::Promoted {
            data: SignalData { value, next },
            handle: h,
            bundle_value: id,
            release,
        });
        let mut old = crate::native::swap_signal_data(w, s, g, promoted)
            .map_err(|_| "the signal's slot was freed mid-promotion".to_string())?;
        // The proxy no longer speaks for the bundle's entry (it is now an
        // import, released by the promoted value).
        if let Some(p) = old.as_any_mut().downcast_mut::<ValueProxy>() {
            p.defused = true;
        }
        drop(old);
        crate::bridge::host::register_export_owned(h, crate::bridge_typed::exporter(h, codec, true));
        finish(id);
        Ok(signal)
    }

    /// Bundle scopes held by id that nobody has claimed or dropped — `0`
    /// once every remote tree's scopes have been claimed (see
    /// [`claim_scope`]). A remote mount that leaves this above zero leaked
    /// the bundle's signals and effects into the host's graph.
    pub fn pending_scopes() -> usize {
        crate::bridge::host::pending_scopes()
    }

    /// Live export registrations, across every slot (tests: an export must
    /// not be registered again for each repeat of the same request).
    #[doc(hidden)]
    pub fn __export_registrations() -> usize {
        crate::bridge::host::export_registrations()
    }

    /// Stop panicking on a bundle's invalid kernel requests on this thread:
    /// record each as a fault for [`take_fault`] instead, so the host can
    /// trap the bundle that made it. A wasm host calls this before loading
    /// a bundle; see `bridge::host::fault`.
    pub fn trap_faults() {
        crate::bridge::host::trap_faults()
    }

    /// The fault the last bundle request raised (see [`trap_faults`]):
    /// checked after every kernel import, and turned into a trap.
    pub fn take_fault() -> Option<String> {
        crate::bridge::host::take_fault()
    }

    /// Where the frames bundles hold open on this thread's kernel stacks
    /// (an entered world, `untrack`, a collecting scope, …) stand: take it
    /// before calling into a bundle. If the call TRAPS, the bundle never
    /// makes its end calls (a wasm panic runs no destructors), and
    /// [`unwind_bundle_frames`] closes them for it.
    pub fn bundle_frames_mark() -> usize {
        crate::bridge::host::frames_mark()
    }

    /// Close every bundle frame opened since `mark`, innermost first; what
    /// an abandoned collecting scope gathered is freed.
    pub fn unwind_bundle_frames(mark: usize) {
        crate::bridge::host::unwind_frames(mark)
    }

    /// Claim scope `id` — a remote component's `Owned`, sent over by
    /// [`remote_guest::release_scope`](crate::remote_guest::release_scope) —
    /// as an `Owned` of this side. The slots it collected (the bundle's
    /// signals, effects and context entries, all living in this graph) now
    /// live and die with the returned value, so an `Element::Owned` decoded
    /// from a bundle is a component boundary exactly like a native one.
    /// `0` (collected nothing) gives an empty `Owned`.
    #[cfg(not(idealyst_stream_guest))]
    pub fn claim_scope(id: u32) -> crate::Owned {
        // Natively the scope's items move out of the bridge's table into a
        // native scope. On loopback the active engine IS the bridge, whose
        // scopes are table ids already: the id is the scope.
        #[cfg(not(feature = "loopback-engine"))]
        let scope = crate::bridge::host::take_scope(id);
        #[cfg(feature = "loopback-engine")]
        let scope = (id != 0).then_some(id);
        crate::Owned { scope, attachments: Vec::new(), _not_send: std::marker::PhantomData }
    }
}

/// The bundle side of host-owned values: what a remote bundle's component
/// glue uses to receive props and context the host owns.
#[cfg(any(idealyst_stream_guest, feature = "loopback-engine"))]
pub mod remote_guest {
    use std::any::{Any, TypeId};
    use std::rc::Rc;

    use super::*;
    pub use crate::bridge_typed::Codec;
    use crate::bridge_typed::TypedSync;

    /// A handle to host-owned signal `h`, for this bundle. Reads subscribe
    /// on the host's graph and see the host's committed value; writes stage
    /// into the host's signal (`update` composes on its staged value). Lives
    /// until the importing scope ends.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn import_signal<T: PartialEq + 'static>(h: (u32, u32, u32), codec: Codec<T>) -> Signal<T> {
        let site = caller_site();
        let mut bytes = Vec::new();
        assert!(
            Active::fetch(h, &mut bytes),
            "kernel bridge: host signal (world {}, slot {}) was not exported to this bundle",
            h.0,
            h.1
        );
        let value = (codec.decode)(&bytes).expect("kernel bridge: host value does not decode");
        let mirror: Box<dyn AnySignal> = Box::new(SignalData { value, next: None });
        let id = Active::import(h, mirror, Rc::new(TypedSync(codec)), site);
        // Released with the importing scope (or effect run). Imported from
        // no scope, nothing would ever release it — and `on_scope_drop`
        // would anchor a keepalive effect at the world root on every call
        // (a context read in an async continuation, say). The slot's one
        // shared entry ([`Active::import`]) then simply lives on.
        if in_effect() || in_collector() {
            on_scope_drop(move || Active::release_import(id));
        }
        Signal { world: h.0, slot: h.1, gen: h.2, _marker: PhantomData }
    }

    /// [`import_signal`]'s read-only half.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn import_read_signal<T: PartialEq + 'static>(h: (u32, u32, u32), codec: Codec<T>) -> ReadSignal<T> {
        import_signal(h, codec).read_only()
    }

    /// Offer `signal` to the host for PROMOTION, before handing its handle
    /// to native code (see
    /// [`remote::receive_signal`](crate::remote::receive_signal)). A no-op
    /// for a signal the host already owns.
    pub fn offer_signal<T: PartialEq + 'static>(signal: Signal<T>, codec: Codec<T>) -> (u32, u32, u32) {
        let h = (signal.world, signal.slot, signal.gen);
        Active::offer(h, Rc::new(TypedSync(codec)));
        h
    }

    /// [`offer_signal`] for a read-only handle — a memo's output included:
    /// once promoted, the memo's own derivation writes the native value.
    pub fn offer_read_signal<T: PartialEq + 'static>(signal: ReadSignal<T>, codec: Codec<T>) -> (u32, u32, u32) {
        let h = (signal.world, signal.slot, signal.gen);
        Active::offer(h, Rc::new(TypedSync(codec)));
        h
    }

    /// Hand `owned` to the host as a scope id, for
    /// [`remote::claim_scope`](crate::remote::claim_scope) on the other
    /// side; `0` when it collected nothing. The slots stay alive — they are
    /// the host's now, and so is the job of dropping them. Attachments are
    /// this side's data and are dropped here.
    pub fn release_scope(mut owned: Owned) -> u32 {
        std::mem::take(&mut owned.scope).unwrap_or(0)
    }

    /// Let `inject::<T>()` fall back to the host context declared under
    /// `name` when this bundle provides no `T` itself.
    pub fn register_remote_context<T: Clone + 'static>(
        name: &'static str,
        decode: impl Fn(&[u8]) -> Option<T> + 'static,
    ) {
        let decode = Rc::new(move |b: &[u8]| decode(b).map(|v| Box::new(v) as Box<dyn Any>));
        Active::register_remote_context(TypeId::of::<T>(), name, decode);
    }
}

/// Compile-time parity: the bridged engine implements the full contract.
#[cfg(any(test, all(feature = "bridge", any(not(target_arch = "wasm32"), idealyst_stream_guest)), feature = "loopback-engine", idealyst_stream_guest))]
const _: fn() = || {
    fn implements_engine<E: Engine>() {}
    implements_engine::<bridge::Loopback>();
};

#[cfg(feature = "hot-reload")]
pub(crate) use native::{signal_type_id, steal_signal_data};
#[cfg(test)]
use native::arena_of;

// ============================================================================
// Staged-read diagnostic — dev-only source-site plumbing.
//
// Why `debug_assertions` and not a cargo feature: this is a correctness
// diagnostic for authors, not a profiling tool. It must be ON for every dev
// build of every app with zero opt-in (an upgrader who has to enable a
// feature to find a silent behaviour change will not enable it), and OFF in
// every release build. That is exactly what `debug_assertions` means, and it
// is the gate the sibling dev warning in runtime-shared's legacy arena
// (`maybe_warn_untracked_build_read`) already uses. A feature would also
// leak into the dependency graph of every consumer that has to forward it —
// see the `debug-stats` forwarding chain for how much ceremony that costs.
//
// `SiteLoc` is the caller-location currency. In debug it is a thin
// `&'static Location`; in release it is `()`, so every parameter, struct
// field and argument below is a ZST the optimizer erases — the diagnostic's
// only *persistent* footprint (`SignalSlot::created_at`) costs zero bytes in
// release. Pinned by `staged_read_diagnostic_is_debug_build_only`.
// ============================================================================

#[cfg(debug_assertions)]
pub(crate) type SiteLoc = &'static std::panic::Location<'static>;
#[cfg(not(debug_assertions))]
pub(crate) type SiteLoc = ();

/// The caller's source location in debug builds; nothing in release.
///
/// `#[track_caller]` is itself `cfg_attr`-gated at every call site so
/// release builds do not even pay the implicit location argument.
#[cfg(debug_assertions)]
#[track_caller]
#[inline]
fn caller_site() -> SiteLoc {
    std::panic::Location::caller()
}

#[cfg(not(debug_assertions))]
#[inline]
fn caller_site() -> SiteLoc {}

/// One recorded staged-read warning. Debug-only, `#[doc(hidden)]`: the
/// test/tooling view of what [`__take_staged_read_warnings`] drained.
#[cfg(debug_assertions)]
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct StagedReadWarning {
    /// Where the author read the signal (the `get`/`peek`/`with` call site).
    pub read_site: SiteLoc,
    /// Where the signal was created.
    pub created_at: SiteLoc,
    pub world: u32,
    pub slot: u32,
}

/// Host-installed diagnostic emit hook. See [`install_diagnostic_sink`].
#[cfg(debug_assertions)]
type DiagSink = Rc<dyn Fn(&str)>;

#[cfg(debug_assertions)]
struct Diag {
    /// Call sites already warned about, keyed by `(file, line, column)`.
    /// Process-lifetime (per thread), NOT per-turn: a raf loop or animation
    /// driver that reads-after-staging every frame must warn once, not 60
    /// times a second. A firehose is worse than silence.
    seen: rustc_hash::FxHashSet<(&'static str, u32, u32)>,
    /// Everything warned since the last drain — the test-visible sink.
    log: Vec<StagedReadWarning>,
    /// Host-installed emit hook (web `console.warn`, NSLog, …). `None`
    /// falls back to `eprintln!`, which is a real stderr write on native
    /// and a silent no-op sink on `wasm32-unknown-unknown` — the same
    /// platform-agnostic default runtime-shared's `StderrLogger` relies on.
    ///
    /// `Rc`, not `Box`, so the emit path can clone the handle OUT of the
    /// `RefCell` before calling it: a sink that itself touched a signal
    /// would otherwise re-enter this borrow and abort.
    sink: Option<DiagSink>,
}

#[cfg(debug_assertions)]
thread_local! {
    static DIAG: RefCell<Diag> = RefCell::new(Diag {
        seen: rustc_hash::FxHashSet::default(),
        log: Vec::new(),
        sink: None,
    });
}

/// Route this kernel's dev diagnostics through a host log channel.
///
/// runtime-world sits below runtime-shared's `Logger` (it is the bottom of
/// the new core's dependency chain and stays dependency-minimal), so it
/// cannot call `log_warn!` directly. Hosts bridge instead: runtime-vocabulary
/// installs a forwarder to `runtime_shared::logging` from
/// `register_builtins`, the one seam every backend's boot passes through.
/// Without a sink the messages go to `eprintln!` — visible under `cargo
/// test` and in terminal hosts, silently dropped on wasm (hence the bridge).
///
/// Thread-local and last-install-wins. Debug builds only: in release the
/// diagnostic does not exist, so neither does this function.
///
/// The sink runs while the *warned* signal's storage is moved out of the
/// arena, so it must not read that signal (it would hit the kernel's
/// reentrancy diagnostic). Log sinks do not read signals; this is a note,
/// not a trap anyone has sprung.
#[cfg(debug_assertions)]
pub fn install_diagnostic_sink(sink: Box<dyn Fn(&str)>) {
    let sink: DiagSink = Rc::from(sink);
    DIAG.with(|d| d.borrow_mut().sink = Some(sink));
}

/// Drain (and reset the dedupe table of) the staged-read warnings recorded
/// on this thread. Test/tooling hook — debug builds only.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn __take_staged_read_warnings() -> Vec<StagedReadWarning> {
    DIAG.with(|d| {
        let d = &mut *d.borrow_mut();
        d.seen.clear();
        std::mem::take(&mut d.log)
    })
}

/// Emit the staged-read warning for one read, at most once per call site.
///
/// Called from [`read_signal`] when the signal's storage still carries a
/// staged `next`. The engine holds no borrow of the slot's metadata at this
/// point (`signal_access` moves the storage box out and drops its borrows
/// before running its closure), so asking for `created_at` here is safe.
#[cfg(debug_assertions)]
fn warn_staged_read(world: WorldId, slot: u32, read_site: SiteLoc) {
    let Some(created_at) = Active::signal_created_at(world, slot) else { return };
    let fresh = DIAG.with(|d| {
        let d = &mut *d.borrow_mut();
        if d.seen.insert((read_site.file(), read_site.line(), read_site.column())) {
            d.log.push(StagedReadWarning { read_site, created_at, world, slot });
            true
        } else {
            false
        }
    });
    if !fresh {
        return;
    }
    let msg = format!(
        "idealyst[staged-read]: the read at {read_site} returns the COMMITTED value — a \
         write staged earlier in this turn (signal created at {created_at}; world {world}, \
         slot {slot}) has not been flushed yet. `set` only stages; the driver's flush is \
         what makes it visible, so `count.set(count.get() + 1)` twice nets +1, not +2. Use \
         `update(|v| ...)`, which composes on the STAGED value, or keep the intended value \
         in a local. Reading the committed value here may well be deliberate — this is a \
         warning, it fires once per call site, and only in debug builds."
    );
    // Clone the handle out before calling it — see `Diag::sink`.
    let sink = DIAG.with(|d| d.borrow().sink.clone());
    match sink {
        Some(sink) => sink(&msg),
        None => eprintln!("[WARN] {msg}"),
    }
}

// ============================================================================
// World — a discrete reactive runtime. A cloneable value; the LAST clone's
// drop tears the world down (cleanups run, registry entry removed, arena
// freed). Many worlds coexist on a thread and flush independently.
// ============================================================================

/// A reactive world. Create with [`World::new`], make it ambient with
/// [`World::enter`], commit staged writes with [`World::flush`]. Cheap to
/// clone (handle semantics); dropping the last clone tears the world down.
pub struct World {
    core: Rc<WorldCore>,
}

impl Clone for World {
    fn clone(&self) -> Self {
        World { core: Rc::clone(&self.core) }
    }
}

struct WorldCore {
    id: WorldId,
}

impl Drop for WorldCore {
    fn drop(&mut self) {
        // Cleanups run while the world is still registered, then it is
        // unregistered — see `Engine::world_drop`.
        Active::world_drop(self.id);
    }
}

impl Default for World {
    fn default() -> Self {
        World::new()
    }
}

impl World {
    /// Create a new, empty world and register it in this thread's registry.
    pub fn new() -> World {
        World { core: Rc::new(WorldCore { id: Active::world_new() }) }
    }

    /// This world's registry id (diagnostic use).
    pub fn id(&self) -> u32 {
        self.core.id
    }

    /// Run `f` with this world as the ambient creation context, so plain
    /// functions can call the free [`signal()`]/[`effect()`] without ever
    /// seeing a world. This is what the framework wraps component calls in.
    /// A stack — worlds nest, and the previous ambient world is restored on
    /// exit (panic-safe).
    pub fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        Active::world_enter(self.core.id, f)
    }

    /// Create a signal belonging to this world (regardless of the ambient
    /// world). See the free [`signal()`] for the ambient form.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn signal<T: PartialEq + 'static>(&self, value: T) -> Signal<T> {
        signal_in(Some(self.core.id), value, caller_site())
    }

    /// Create an effect in this world and run it once immediately to collect
    /// dependencies. See the free [`effect()`] for the ambient form.
    pub fn effect<C: IntoCleanup>(&self, f: impl FnMut() -> C + 'static) -> Effect {
        effect_in(Some(self.core.id), EffectClass::Reaction, wrap_effect_body(f))
    }

    /// Create a memo in this world. See the free [`memo()`].
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn memo<T, F>(&self, f: F) -> Memo<T>
    where
        T: PartialEq + Clone + 'static,
        F: Fn() -> T + 'static,
    {
        // Site captured outside the closure — `#[track_caller]` does not
        // propagate through one, and a memo's cache signal should report
        // the AUTHOR's `memo(...)` site, not this file.
        let site = caller_site();
        self.enter(|| memo_at(f, site))
    }

    /// Store a value in this world's context, keyed by its type. Use newtype
    /// wrappers to distinguish values of the same underlying type.
    ///
    /// Always **world-lifetime**: this is the outside-in API, called on a
    /// `World` value rather than from inside a mount, so there is no
    /// ambient scope to own the provision. Use the free
    /// [`provide`] from inside components when the entry should die with
    /// the providing scope.
    pub fn provide<T: Clone + 'static>(&self, value: T) {
        Active::world_provide(self.core.id, TypeId::of::<T>(), Box::new(value));
    }

    /// Fetch a value from this world's context by type. Untracked — this is
    /// delivery, not reactivity; put a `Signal` inside the value for that.
    pub fn inject<T: Clone + 'static>(&self) -> Option<T> {
        Active::world_inject(self.core.id, TypeId::of::<T>(), |v| v.downcast_ref::<T>().cloned()).flatten()
    }

    /// Commit all staged writes and run affected effects, glitch-free.
    ///
    /// Effects carry a class: **Derivation** (memo recomputes) or
    /// **Reaction** (everything else). Per outer round:
    ///
    /// 1. drain + commit staged signals, collecting dirty effects (dedup);
    /// 2. run dirty Derivations — their writes stage; commit and run newly
    ///    dirty Derivations until none is dirty (Reactions collected along
    ///    the way are HELD);
    /// 3. then run each held Reaction exactly once;
    /// 4. Reactions may stage → next outer round.
    ///
    /// **Glitch-freedom argument**: a Reaction only runs after step 2 has
    /// settled, i.e. after every Derivation reachable from this round's
    /// commits has recomputed and committed. Whatever mix of raw signals and
    /// memos a Reaction reads, all of them reflect the same settled
    /// generation of the graph — the diamond `S → memo(S)` read together
    /// with `S` can never show (fresh S, stale memo), and the Reaction runs
    /// once per flush, not once per round. (idea-lite's naive round loop ran
    /// both classes together, letting a reaction observe the stale pair and
    /// run twice; the current sync core is depth-first-consistent, so
    /// authors implicitly rely on consistency — this preserves it.)
    ///
    /// A round limit (100) turns cyclic updates (reaction A stages what
    /// reaction B reads and vice versa) into a panic instead of a hang.
    ///
    /// Calling `flush` on world A from inside world B's effect is legal
    /// (worlds are independent); calling it re-entrantly on the SAME world
    /// from inside its own effect panics — staged writes made during a flush
    /// are committed by that same flush's next round.
    pub fn flush(&self) {
        Active::world_flush(self.core.id);
    }

    /// Is THIS world mid-flush? See the free [`is_flushing`] for the
    /// any-world form (the `is_reactive_busy` replacement).
    pub fn is_flushing(&self) -> bool {
        Active::world_is_flushing(self.core.id)
    }
}

/// True while any world on this thread is inside [`World::flush`] — i.e.
/// while effects are being run by the reactive system rather than by event
/// handlers. Replaces runtime-core's `is_reactive_busy`.
pub fn is_flushing() -> bool {
    Active::is_flushing()
}

/// True while a live world is ambient on this thread (inside
/// [`World::enter`]) — i.e. while creation-side APIs (`signal()`,
/// `effect()`, `provide()`/`inject()`) are legal.
///
/// This is the probe the vocabulary's handler-safe surfaces fork on:
/// platform event handlers run OUTSIDE `World::enter` (the flush driver
/// commits afterwards), so an ambient-convenience free fn that also
/// wants to work from a handler checks `is_entered()` and falls back to
/// a handle/ctx captured at build time when it returns `false`. Found
/// live: idea-theme's `set_theme` panicked "outside World::enter"
/// through the ambient `inject` when called from a button handler.
pub fn is_entered() -> bool {
    Active::is_entered()
}

/// True while an effect body (any world's) is running on this thread —
/// exactly the window where [`on_cleanup`] is legal.
///
/// This is the probe scope-anchored scheduling helpers fork on (the
/// vocabulary's `after_ms_scoped` / `raf_loop_scoped` shadows): inside
/// an effect run they anchor their cancellation via [`on_cleanup`]
/// (dies on the effect's re-run or its owner's drop); outside one they
/// must fall back to a collector-owned keepalive instead — calling
/// [`on_cleanup`] there would panic.
pub fn in_effect() -> bool {
    Active::in_effect()
}

/// How many effect bodies are running, nested, on this thread — `0`
/// outside every effect.
///
/// The discriminator a caller needs when it holds a lifetime captured at
/// some earlier point and must decide whether the effect running NOW is
/// *inside* that capture or *around* it. Comparing this against the depth
/// recorded at capture time answers exactly that, which
/// [`in_effect`] alone cannot: it says an effect is running, not whether
/// it is the innermost dynamic scope. `runtime_vocabulary`'s
/// `ScopeAlive::current` forks on the comparison.
pub fn effect_depth() -> usize {
    Active::effect_depth()
}

/// A handle to the effect whose body is running RIGHT NOW (innermost when
/// nested), or `None` outside every effect.
///
/// The handle is the effect's own liveness: [`Effect::is_alive`] goes
/// `false` exactly when the [`Owned`] that collected the effect drops and
/// its slot is freed — NOT on the effect's re-runs. That distinction is
/// the whole reason this exists rather than [`on_cleanup`]: work spawned
/// from an effect body wants the lifetime of the effect's OWNER (the
/// component subtree), while `on_cleanup` additionally fires before the
/// next re-run. See `ScopeAlive::current` in `runtime_vocabulary` for the
/// case that forced it — a `spawn_then` issued from an effect RE-RUN used
/// to anchor to nothing at all.
pub fn current_effect() -> Option<Effect> {
    let (world, slot, gen) = Active::current_effect()?;
    Some(Effect { world, slot, gen, _marker: PhantomData })
}

/// True while an ownership collector ([`collect_owned`]) is active on
/// this thread — i.e. a signal/effect created right now would be owned
/// by a component subtree's [`Owned`], not the world root.
///
/// A subtree being BUILT wins over the effect that happens to be building
/// it, so both the scope-anchored scheduling helpers and
/// `runtime_vocabulary`'s `ScopeAlive::current` resolve this rung FIRST.
/// (`scoped_scheduling::current_anchor` expresses that by applying the
/// collector keepalive IN ADDITION to `on_cleanup` on its `in_effect`
/// branch — whichever lifetime ends first kills the anchor — rather than
/// by testing this predicate earlier; the effect is the same.) A timer
/// registered while a subtree is being built —
/// even when that build happens inside a running effect, e.g. a
/// navigator's swap effect realizing a screen whose lazy fallback
/// schedules timers — must die with the SUBTREE. Anchoring it to the
/// running effect would let it outlive the subtree until the effect's
/// next re-run, firing into the subtree's already-freed signals (the
/// lazy-route-loader stale-signal-handle panic).
pub fn in_collector() -> bool {
    Active::in_collector()
}


// ============================================================================
// Typed value storage and creation. The engine stores `SignalData<T>` as a
// `Box<dyn AnySignal>` and only ever commits it; everything that knows `T`
// lives here.
// ============================================================================

struct SignalData<T> {
    /// Committed value — the only thing reads ever see.
    value: T,
    /// Staged value, last-write-wins within a batch.
    next: Option<T>,
}

impl<T: PartialEq + 'static> AnySignal for SignalData<T> {
    #[cfg(feature = "hot-reload")]
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }

    #[cfg(feature = "hot-reload")]
    fn value_type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<T>()
    }

    fn commit(&mut self, forced: bool) -> bool {
        match self.next.take() {
            Some(next) => {
                // The equality cut: a staged value that nets to the committed
                // one notifies nobody (A→B→A within one window is a no-op —
                // window-initial comparison for free, matching the guarded
                // `set` semantics runtime-core just landed). `forced` (a
                // set_always/touch in the window) taints the window and
                // notifies regardless.
                if forced || next != self.value {
                    self.value = next;
                    true
                } else {
                    false
                }
            }
            // No staged value: notify iff a `touch` forced it.
            None => forced,
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Create a signal in `world` (`None` = ambient) — the typed half of
/// `signal()` / `World::signal`. The hot-reload seed is taken here, where
/// `T` is known, and the engine stores the erased box.
fn signal_in<T: PartialEq + 'static>(world: Option<WorldId>, value: T, site: SiteLoc) -> Signal<T> {
    // Dev-time hot reload: if a previous run of this same position held
    // a value of this type, start from it instead of the author's
    // initial value. Off by default and compiled out entirely — see
    // `hot_state`.
    //
    // A creation inside `unscoped` is a world-lifetime service, not a
    // component's state: it is often created once and cached, so the
    // second run may not create it at all. It must neither take a seed
    // nor consume an ordinal, or every later signal in the frame would
    // shift onto its neighbour's value.
    #[cfg(feature = "hot-reload")]
    let world_lifetime = hot_state::in_world_lifetime_region();
    #[cfg(feature = "hot-reload")]
    let value = if world_lifetime {
        value
    } else {
        hot_state::take_seed::<T>().unwrap_or(value)
    };
    let data: Box<dyn AnySignal> = Box::new(SignalData { value, next: None });
    let (world, slot, gen, _collected) = Active::signal_create(world, data, site);
    #[cfg(feature = "hot-reload")]
    if !world_lifetime {
        hot_state::record(world, slot, gen, _collected);
    }
    Signal { world, slot, gen, _marker: PhantomData }
}

/// Create an effect in `world` (`None` = ambient) and run it once.
fn effect_in(world: Option<WorldId>, class: EffectClass, body: Box<dyn FnMut()>) -> Effect {
    let (world, slot, gen) = Active::effect_create(world, class, body);
    Effect { world, slot, gen, _marker: PhantomData }
}

// ============================================================================
// Handles — Copy triples routing to their own world. `PhantomData<*const T>`
// keeps them !Send/!Sync on purpose: worlds are thread-local, so a handle on
// another thread could only ever observe "dead world"; better to reject the
// program shape at compile time.
// ============================================================================

/// The full read+write handle to a signal. `Copy`. Prefer handing components
/// a capability half ([`ReadSignal`] / [`WriteSignal`]) — the signature then
/// documents the data flow.
pub struct Signal<T> {
    world: WorldId,
    slot: u32,
    gen: u32,
    _marker: PhantomData<*const T>,
}

/// Read capability only: `get`/`peek`/`with`. What display-only components
/// take. `Copy`.
pub struct ReadSignal<T> {
    world: WorldId,
    slot: u32,
    gen: u32,
    _marker: PhantomData<*const T>,
}

/// Write capability only: `set`/`update`/`touch`/…. What controls and
/// emitters take. `Copy`.
pub struct WriteSignal<T> {
    world: WorldId,
    slot: u32,
    gen: u32,
    _marker: PhantomData<*const T>,
}

/// A non-owning `Copy` handle to an effect. The effect's LIFETIME is owned
/// by the [`Owned`] scope that collected it (or the world root) — dropping
/// this handle changes nothing; dropping the `Owned` retires the effect.
pub struct Effect {
    world: WorldId,
    slot: u32,
    gen: u32,
    _marker: PhantomData<*const ()>,
}

macro_rules! impl_handle_meta {
    ($name:ident $(<$t:ident>)?) => {
        impl$(<$t>)? Clone for $name$(<$t>)? {
            fn clone(&self) -> Self { *self }
        }
        impl$(<$t>)? Copy for $name$(<$t>)? {}
        impl$(<$t>)? PartialEq for $name$(<$t>)? {
            fn eq(&self, other: &Self) -> bool {
                self.world == other.world && self.slot == other.slot && self.gen == other.gen
            }
        }
        impl$(<$t>)? Eq for $name$(<$t>)? {}
        impl$(<$t>)? std::fmt::Debug for $name$(<$t>)? {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!(stringify!($name), "(w{}, s{}, g{})"), self.world, self.slot, self.gen)
            }
        }
    };
}

impl_handle_meta!(Signal<T>);
impl_handle_meta!(ReadSignal<T>);
impl_handle_meta!(WriteSignal<T>);
impl_handle_meta!(Effect);

#[inline]
fn signal_is_alive(world: WorldId, slot: u32, gen: u32) -> bool {
    Active::signal_is_alive(world, slot, gen)
}

#[inline]
fn signal_subscriber_count(world: WorldId, slot: u32, gen: u32) -> usize {
    Active::signal_subscriber_count(world, slot, gen)
}

#[inline]
fn effect_is_alive(world: WorldId, slot: u32, gen: u32) -> bool {
    Active::effect_is_alive(world, slot, gen)
}

macro_rules! impl_signal_liveness {
    ($name:ident) => {
        impl<T> $name<T> {
            /// Is this handle still usable — its world alive and its slot
            /// neither freed nor recycled?
            ///
            /// Handles are `Copy` and carry no ownership, so one can
            /// outlive the [`Owned`] scope that collected its signal; a
            /// *read* through such a handle aborts with
            /// `stale-signal-handle` rather than fabricating a value. This
            /// is the probe that lets a holder check first instead of
            /// crashing, for the cases where the handle legitimately
            /// arrives from somewhere with a shorter life than the reader
            /// — most notably a value pulled out of [`inject`].
            ///
            /// Treat it as a guard, not a license: `true` means the slot is
            /// live *now*, and the answer is only stable while nothing
            /// drops an `Owned` in between. Code whose correctness depends
            /// on the signal existing wants a real ownership relationship,
            /// not a probe.
            pub fn is_alive(&self) -> bool {
                signal_is_alive(self.world, self.slot, self.gen)
            }

            /// How many effects READ this signal on their latest run —
            /// its live readers. `0` for a stale handle.
            ///
            /// Never panics and never subscribes. A probe for tooling
            /// that has to know whether a write would be SEEN: the
            /// dev-time overlay swaps a component's literal prop for a
            /// signal and must tell "the body reads it in a binding"
            /// (a write updates the screen) from "the body read it once
            /// while building" (a write changes nothing, and the edit
            /// has to take another path). Subscriptions are reconciled
            /// on every effect run and unlinked when an effect is freed,
            /// so the count is exact, not an upper bound.
            pub fn subscriber_count(&self) -> usize {
                signal_subscriber_count(self.world, self.slot, self.gen)
            }
        }
    };
}

impl_signal_liveness!(Signal);
impl_signal_liveness!(ReadSignal);
impl_signal_liveness!(WriteSignal);

impl Effect {
    /// Is this handle still usable — its world alive and its effect slot
    /// neither freed nor recycled? See [`Signal::is_alive`].
    pub fn is_alive(&self) -> bool {
        effect_is_alive(self.world, self.slot, self.gen)
    }
}


// ============================================================================
// Signal operations — the typed half. Routing, liveness, re-entrancy and the
// borrow discipline are the engine's (`Engine::signal_access` runs its
// closure with no engine borrow held); what remains here is everything that
// needs `T`: the downcast, the staged-read check, the value writes.
// ============================================================================

/// The typed view of a signal's erased storage.
#[inline]
fn typed<T: PartialEq + 'static>(d: &mut dyn AnySignal) -> &mut SignalData<T> {
    d.as_any_mut()
        .downcast_mut::<SignalData<T>>()
        .expect("runtime-world: signal type mismatch despite matching generation — kernel bug")
}

/// Run `f` against a signal's typed storage. Panics on a stale handle or a
/// re-entrant access (the engine's diagnostics); `None` means the world is
/// dead, and the caller decides what that means.
#[inline]
fn with_data<T: PartialEq + 'static, R>(
    world: WorldId,
    slot: u32,
    gen: u32,
    f: impl FnOnce(&mut SignalData<T>) -> R,
) -> Option<R> {
    match Active::signal_access(world, slot, gen, |d| f(typed::<T>(d))) {
        Access::Done(r) => Some(r),
        Access::DeadWorld => None,
    }
}

fn dead_world_read(world: WorldId, slot: u32) -> ! {
    diag_panic!(
        "dead-world-read",
        "signal read after its World was dropped (world {}, slot {}): the world's \
         registry entry is gone — the World value was dropped (its signals died with \
         it), or this handle crossed to another thread. Writes to a dead world are \
         silent no-ops, but a read has no committed value to return; restructure so the \
         reader cannot outlive the world.",
        world,
        slot
    )
}

/// Tracked/untracked committed-value read. Reads PANIC on a dead world:
/// there is no committed value left to return, and silently fabricating one
/// would hide a real lifetime bug.
fn read_signal<T: PartialEq + 'static, R>(
    world: WorldId,
    slot: u32,
    gen: u32,
    track: bool,
    site: SiteLoc,
    f: impl FnOnce(&T) -> R,
) -> R {
    #[cfg(not(debug_assertions))]
    let _ = site;
    let read = Active::signal_read(world, slot, gen, track, |d, subscribed| {
        #[cfg(not(debug_assertions))]
        let _ = subscribed;
        let d = typed::<T>(d);
        // The staged-read diagnostic. `next.is_some()` IS the condition —
        // "a write was staged this turn and the flush that would commit it
        // has not run" — with no extra bookkeeping and an automatic
        // per-turn reset: `AnySignal::commit` does `self.next.take()`, so
        // the moment the flush commits, reads here are correct and silent.
        //
        // `!subscribed` is what makes it a *bug* detector rather than a
        // staging detector. A read that subscribed the running effect is
        // re-delivered when the staged value commits — the effect re-runs
        // with the fresh value, so the staleness is transient and
        // self-correcting. This is not a corner case: it is how the kernel
        // itself settles (a memo body reading a sibling memo's cache mid
        // derivation batch) and how ordinary drivers behave (the theme
        // driver's first run reads a version someone just bumped). An
        // UNSUBSCRIBED read — an event handler, a component build body
        // (`component_scope` runs untracked), `peek`/`with_untracked`, a
        // cross-world read — has no such second chance: the stale value is
        // the final answer, and that is exactly the 0.5 → 1.0 hazard.
        //
        // Deliberately placed on the READ path only: `stage_update` reaches
        // the same storage through `Engine::signal_write` directly and never
        // passes here, so `update` — the API the migration guide tells
        // people to switch to — is silent by construction.
        //
        // A staged value EQUAL to the committed one is not a stale read:
        // the read already returns what the flush will commit. The case
        // that matters is every `memo`: its derivation effect's first run
        // (at creation) stages `f()` on top of the `untrack(&f)` initial
        // value, so until the next flush the cache carries a `next` equal
        // to `value`, and any untracked read of a fresh memo (a component
        // build body reading a memo passed as a `Reactive` prop) warned
        // about a hazard that cannot exist. Pinned by
        // `a_fresh_memo_read_before_the_flush_does_not_warn`.
        #[cfg(debug_assertions)]
        if !subscribed && d.next.as_ref().is_some_and(|next| *next != d.value) {
            warn_staged_read(world, slot, site);
        }
        f(&d.value)
    });
    match read {
        Access::Done(r) => r,
        Access::DeadWorld => dead_world_read(world, slot),
    }
}

/// Stage a value (last-write-wins) and enqueue the signal for commit at the
/// owning world's next flush. `force` marks the window tainted (always
/// notify). Writes to a DEAD world are silent no-ops — the world (an app
/// teardown, a finished SSR request) is gone and nothing can ever commit;
/// in-flight async writes racing a teardown are expected and harmless.
/// Writes through a STALE handle (live world, freed slot) still panic: that
/// is a use-after-unmount logic error worth surfacing.
fn stage_set<T: PartialEq + 'static>(world: WorldId, slot: u32, gen: u32, value: T, force: bool) {
    let _ = Active::signal_write(world, slot, gen, force, |d| typed::<T>(d).next = Some(value));
}

/// Read-modify-write against the STAGED value (falling back to committed),
/// so multiple updates in one batch compose (two `+1`s = `+2`).
fn stage_update<T: PartialEq + 'static>(world: WorldId, slot: u32, gen: u32, f: impl FnOnce(&T) -> T) {
    let _ = Active::signal_write(world, slot, gen, false, |d| {
        let d = typed::<T>(d);
        let next = f(d.next.as_ref().unwrap_or(&d.value));
        d.next = Some(next);
    });
}

/// Force a notify with no value write (`touch`). Validates the generation
/// itself — with no storage access to piggyback on, a stale handle would
/// otherwise wake the recycled slot's NEW occupant's subscribers.
fn stage_touch(world: WorldId, slot: u32, gen: u32) {
    Active::signal_touch(world, slot, gen);
}

/// Write the COMMITTED value directly, bypassing staging, waking nobody
/// (`set_untracked` — runtime-core semantics preserved). Dependents keep
/// their stale view until something else notifies. A later guarded `set`
/// compares its staged value against THIS write (so `set_untracked(9)` then
/// `set(9)` nets to no fan-out), and a later `touch` delivers it.
fn write_untracked<T: PartialEq + 'static>(world: WorldId, slot: u32, gen: u32, value: T) {
    let _ = with_data::<T, ()>(world, slot, gen, |d| d.value = value);
}

macro_rules! impl_read_ops {
    () => {
        /// Read the committed value. If called during an effect run — an
        /// effect of this signal's OWN world — subscribes that effect.
        ///
        /// In debug builds, reading a signal whose staged write has not
        /// been flushed yet warns once per call site — see the
        /// staged-read diagnostic in the module docs.
        #[cfg_attr(debug_assertions, track_caller)]
        pub fn get(&self) -> T
        where
            T: Clone,
        {
            read_signal(self.world, self.slot, self.gen, true, caller_site(), T::clone)
        }

        /// Read the committed value WITHOUT tracking — never subscribes,
        /// even inside an effect. For effects that update state derived from
        /// themselves: `set(peek() + 1)` inside an effect body would
        /// infinitely re-trigger if written with `get()`.
        ///
        /// `peek` drops the *subscription*, not the staging rule: it still
        /// returns the committed value, so it warns after a staged write
        /// exactly like [`get`](Self::get) does.
        #[cfg_attr(debug_assertions, track_caller)]
        pub fn peek(&self) -> T
        where
            T: Clone,
        {
            read_signal(self.world, self.slot, self.gen, false, caller_site(), T::clone)
        }

        /// Tracked borrow-read of the committed value: `f` gets `&T`, no
        /// clone. The idiom for `Vec` rows / large values. Note: reading the
        /// SAME signal again from inside `f` is a reentrancy error (the
        /// storage is moved out for `f`'s duration); other signals are fine.
        #[cfg_attr(debug_assertions, track_caller)]
        pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
            read_signal(self.world, self.slot, self.gen, true, caller_site(), f)
        }

        /// Untracked borrow-read — [`with`](Self::with) without the
        /// subscription.
        #[cfg_attr(debug_assertions, track_caller)]
        pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
            read_signal(self.world, self.slot, self.gen, false, caller_site(), f)
        }
    };
}

macro_rules! impl_write_ops {
    () => {
        /// Stage a value. Nothing observable changes until the owning world
        /// flushes; reads keep returning the committed value. Commit is
        /// equality-guarded (`PartialEq`): a staged value equal to the
        /// committed one notifies nobody. If the owning world is gone, this
        /// is a silent no-op.
        pub fn set(&self, value: T) {
            stage_set(self.world, self.slot, self.gen, value, false);
        }

        /// Stage a value that will notify subscribers at commit **even if
        /// equal** to the committed value (retrigger semantics). The
        /// escape hatch from the guarded [`set`](Self::set).
        pub fn set_always(&self, value: T) {
            stage_set(self.world, self.slot, self.gen, value, true);
        }

        /// Write the committed value directly **without staging or
        /// notifying**. Dependents keep their stale view until something
        /// else notifies (a later `set` that changes it, or a `touch`).
        /// Exists for the rare bookkeeping value effects read but must never
        /// react to; an antipattern for ordinary state.
        pub fn set_untracked(&self, value: T) {
            write_untracked(self.world, self.slot, self.gen, value);
        }

        /// Wake this signal's subscribers at the next flush **without
        /// writing** — the notification half of
        /// [`set_always`](Self::set_always). Use after mutating shared
        /// interior state the signal's value merely points to.
        pub fn touch(&self) {
            stage_touch(self.world, self.slot, self.gen);
        }

        /// Read-modify-write against the STAGED value (falling back to the
        /// committed one). What event handlers should use: two clicks in one
        /// batch compose (0→1→2), whereas `set(peek() + 1)` twice would read
        /// the committed 0 both times and lose an increment.
        pub fn update(&self, f: impl FnOnce(&T) -> T) {
            stage_update(self.world, self.slot, self.gen, f);
        }
    };
}

impl<T: PartialEq + 'static> Signal<T> {
    impl_read_ops!();
    impl_write_ops!();

    /// Split into capability halves. The `Signal` itself stays usable (it's
    /// `Copy`); this is about what you hand to OTHERS.
    pub fn split(&self) -> (ReadSignal<T>, WriteSignal<T>) {
        (self.read_only(), self.write_only())
    }

    pub fn read_only(&self) -> ReadSignal<T> {
        ReadSignal { world: self.world, slot: self.slot, gen: self.gen, _marker: PhantomData }
    }

    pub fn write_only(&self) -> WriteSignal<T> {
        WriteSignal { world: self.world, slot: self.slot, gen: self.gen, _marker: PhantomData }
    }

    /// A stable `u64` identity for EXTERNAL registries (JS-side binding
    /// maps, notifier dedup tables). Packs `world:8 | gen:24 | slot:32`
    /// — the low 32 bits are the slot, so consumers that truncate to
    /// `u32` (the web backend's JS signal-change dispatcher does) still
    /// get per-live-signal uniqueness within one world; the generation
    /// bits let a full-width consumer distinguish a freed-and-reused
    /// slot from its previous occupant. This is an identity KEY, not a
    /// capability — it cannot be turned back into a handle.
    pub fn raw_id(&self) -> u64 {
        ((self.world as u64 & 0xff) << 56)
            | ((self.gen as u64 & 0xff_ffff) << 32)
            | self.slot as u64
    }
}

impl<T: PartialEq + 'static> ReadSignal<T> {
    impl_read_ops!();

    /// Same identity KEY as [`Signal::raw_id`] (a read half aliases its
    /// signal's slot, so the ids agree) — external registries (JS text
    /// bindings, notifier dedup) key read-only slots by it too.
    pub fn raw_id(&self) -> u64 {
        ((self.world as u64 & 0xff) << 56)
            | ((self.gen as u64 & 0xff_ffff) << 32)
            | self.slot as u64
    }
}

impl<T: PartialEq + 'static> WriteSignal<T> {
    impl_write_ops!();
}

/// Create a signal in the ambient world. What components call.
#[cfg_attr(debug_assertions, track_caller)]
pub fn signal<T: PartialEq + 'static>(value: T) -> Signal<T> {
    // Captured HERE, outside the closure: `#[track_caller]` does not reach
    // through a closure body, so `caller_site()` must be called in the
    // tracked frame and the value carried in.
    let site = caller_site();
    signal_in(None, value, site)
}

// ============================================================================
// Effects — creation, running, cleanups, untrack.
// ============================================================================

fn wrap_effect_body<C: IntoCleanup>(mut f: impl FnMut() -> C + 'static) -> Box<dyn FnMut()> {
    Box::new(move || f().register())
}

/// Create an effect in the ambient world and run it once immediately to
/// collect dependencies. The body may return a cleanup closure
/// (React-style); it runs before the next re-run or at teardown, same as
/// [`on_cleanup`]. The returned handle is `Copy` and non-owning — the
/// effect's lifetime belongs to the enclosing [`collect_owned`] scope (or
/// the world root).
pub fn effect<C: IntoCleanup>(f: impl FnMut() -> C + 'static) -> Effect {
    effect_in(None, EffectClass::Reaction, wrap_effect_body(f))
}

/// Run `f` with dependency tracking suspended: signal reads inside subscribe
/// nothing, even when called from within an effect.
///
/// Suspension applies to the current code region only, not to nested effect
/// BODIES: an effect created inside `untrack` runs its first body with
/// tracking re-enabled (see `run_effect`) — it collects its own deps like
/// any other effect, and the enclosing region is untracked again once that
/// first run returns.
///
/// Suspension is GLOBAL (a depth counter in the single TLS struct), not
/// per-world: "this read is a snapshot" is a property of the code region,
/// not of whichever world is ambient or owns the signal. The alternative —
/// clearing only the ambient world's current effect — breaks under nesting:
/// inside `a`-effect { `b.enter(|| untrack(|| a_signal.get()))` } it would
/// clear B's (empty) slot while the running A-effect still tracks the read.
/// The global counter makes untrack mean the same thing in every
/// cross-world composition (tested: `untrack_suspends_tracking_globally_across_worlds`).
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    Active::untrack(f)
}

/// Run `f` with the ownership-collector stack SUSPENDED, so every signal /
/// effect / memo created inside — and every [`provide`]d context entry —
/// is **world-root-owned** (freed only when its world drops) instead of
/// being collected into whatever `collect_owned` scope happens to be
/// active.
///
/// The escape hatch for **world-lifetime services created lazily during a
/// mount**: a per-world theme-version signal or cohort-driver effect is
/// first needed inside some subtree's realization (a `collect_owned`
/// scope), but must outlive that subtree — without suspension it would be
/// collected into the subtree's `Owned` and freed on that subtree's
/// unmount, leaving the service's `Copy` handles dangling. A service that
/// publishes itself with `provide` must wrap the `provide` too, not just
/// the signal: a world-root signal announced through a scope-owned entry
/// stops being *findable* on that scope's unmount, and the service is
/// silently rebuilt. This is the
/// direct analogue of the old core's `reactive::unscope`, added for the
/// exact same regression class (runtime-core `style.rs`'s token-registry
/// "signal used after its scope was dropped" incident; the new-core style
/// engine hit the same shape with its per-world theme state).
///
/// Suspension is a swap of the whole collector stack (panic-safe restore),
/// mirroring [`untrack`]'s global posture: "this creation is
/// world-lifetime" is a property of the code region, not of one collector.
pub fn unscoped<R>(f: impl FnOnce() -> R) -> R {
    Active::unscoped(f)
}

/// Run `f` with signal creation EXEMPT from the hot-reload state carrier:
/// a signal created inside neither takes a carried-over value nor
/// consumes an ordinal of the component frame that is open. Ownership is
/// untouched — unlike [`unscoped`], the ambient collector still owns what
/// is created here, so it is freed with its subtree.
///
/// For tooling that creates signals inside an AUTHOR's frame that the
/// author did not write. The carrier matches state across a re-run by
/// the Nth `signal()` in a frame, so an extra creation that appears in
/// one run and not the next (the dev overlay's per-literal-prop cells
/// exist only while a prop is a literal) would shift every later signal
/// of that frame onto its neighbour's value — a `String` of state
/// receiving a button label, with nothing to say so. Without the
/// `hot-reload` feature this is just `f()`.
pub fn hot_state_exempt<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(feature = "hot-reload")]
    let _exempt = hot_state::WorldLifetimeRegion::enter();
    f()
}
/// Register a cleanup for the innermost RUNNING effect (whatever world it
/// belongs to). Runs before that effect's next re-run, or when its owning
/// scope drops — whichever comes first. May be called multiple times per
/// run; cleanups run in registration order.
pub fn on_cleanup(f: impl FnOnce() + 'static) {
    Active::on_cleanup(Box::new(f))
}

/// Register a teardown for the innermost OWNERSHIP SCOPE — a component
/// body, a registry mount handler, a `collect_owned` region — rather than
/// for a running effect. Runs when that scope's [`Owned`] drops.
///
/// This is the counterpart [`on_cleanup`] cannot be. `on_cleanup` reads the
/// effect stack and panics when it is empty, which is the normal state
/// everywhere a tree is BUILT: a `#[component]` body, a `Registry` mount
/// handler, the initial `realize`. Code that acquires a resource while
/// building and wants it released at unmount had no legal hook, and three
/// call sites reached for `on_cleanup` anyway — idea-ui's measured
/// `Collapsible` and the video SDK's iOS/macOS `build_video`. All three
/// aborted the moment their subtree was mounted directly instead of being
/// swapped in by a reactive re-render (the effect-run case that happens to
/// satisfy `on_cleanup`): deep-linking to the Collapsible page took the
/// whole app down.
///
/// Anchoring rules, matching `runtime_vocabulary::scoped_scheduling`'s
/// `current_anchor`:
///
/// - **Inside an effect run** — defers to [`on_cleanup`], so the teardown
///   also fires before the effect's next re-run. A resource acquired during
///   a run belongs to that run. "Inside an effect run" means inside an
///   effect BODY: building a subtree is not one, even when a driver effect
///   is what triggered the build, because component bodies and the mount
///   walk run [`unanchored`]. Before that was true, a screen mounted by a
///   navigator's driver anchored its teardown to that driver and fired it
///   on the next navigation.
/// - **Outside one, inside a world** — anchors to a dependency-free
///   keepalive effect. It reads nothing, so it never re-runs; it is owned by
///   the enclosing collector (component subtree → mount → world root), and
///   its teardown is the scope's teardown.
/// - **Outside any world** — inert. `f` is dropped without running, which
///   releases whatever it captured. Nothing owns the scope, so there is no
///   moment to fire at.
pub fn on_scope_drop(f: impl FnOnce() + 'static) {
    if in_effect() {
        on_cleanup(f);
        return;
    }
    if !is_entered() {
        return;
    }
    // Registered from INSIDE the keepalive's first run, where the effect
    // stack is non-empty and `on_cleanup` is legal — rather than relying on
    // the body closure's own drop, which only fires if nothing else is
    // holding the effect's `Rc<EffectData>` at free time. `cleanups` is
    // drained explicitly by `free_effect`, so the timing is exact.
    let mut once = Some(f);
    let _ = effect(move || {
        if let Some(f) = once.take() {
            on_cleanup(f);
        }
    });
}

/// Register a teardown that fires **only** when the innermost ownership
/// scope's [`Owned`] drops — never on an enclosing effect's re-run.
///
/// This is [`on_scope_drop`] minus its in-effect shortcut, and the
/// distinction is not academic. `on_scope_drop` defers to [`on_cleanup`]
/// while an effect is running, and `on_cleanup` fires before that effect's
/// next RE-RUN as well as at scope teardown. That is right for a resource
/// belonging to one run, and wrong for anything whose lifetime is the
/// mounted node's:
///
/// A keyed list's structural driver is one effect that re-runs on every
/// edit to the list, while keyed reconcile deliberately PRESERVES the
/// surviving rows' subtrees. Teardown registered from inside a row's mount
/// via `on_scope_drop` used to fire on the first unrelated edit — tearing
/// down state for a row that is still on screen. Anchoring to the ownership
/// scope instead ties the teardown to the row's own `Owned`, which keyed
/// reconcile drops if and only if that row really goes away.
///
/// That MOUNT-TIME case is no longer a live hazard: a row renders
/// [`unanchored`], so `on_scope_drop` there sees no running effect and lands
/// on the row's `Owned` by itself. What remains — and what this fn is still
/// the only way to express — is registering from inside a genuine effect
/// body while wanting the SUBTREE's lifetime rather than the run's.
///
/// Outside any world this is inert, exactly like [`on_scope_drop`]: nothing
/// owns the scope, so there is no moment at which it could fire.
///
/// Regression: `callbacks_survive_a_keyed_reconcile` in runtime-vocabulary.
pub fn on_owned_drop(f: impl FnOnce() + 'static) {
    if !is_entered() {
        return;
    }
    // Same keepalive-effect mechanism as `on_scope_drop`'s outside-an-effect
    // path, and registered from inside the keepalive's first run for the
    // same reason: `free_effect` drains `cleanups` explicitly, so the timing
    // is exact rather than depending on who else holds the `Rc<EffectData>`.
    // The keepalive reads nothing, so it never re-runs on its own — the only
    // thing that can fire it is its owner being dropped.
    let mut once = Some(f);
    let _ = effect(move || {
        if let Some(f) = once.take() {
            on_cleanup(f);
        }
    });
}

/// What an effect body is allowed to return. Sealed: exactly three forms —
/// nothing, a cleanup closure, or `Option` of one (conditional cleanup). A
/// returned cleanup is registered through the same [`on_cleanup`] mechanism,
/// so ordering and drop semantics are identical for both styles.
#[diagnostic::on_unimplemented(
    message = "an effect body must return `()`, a cleanup closure, or `Option<cleanup closure>`",
    label = "not a valid effect return value"
)]
pub trait IntoCleanup: sealed::Sealed {
    fn register(self);
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for () {}
    impl<F: FnOnce() + 'static> Sealed for F {}
    impl<F: FnOnce() + 'static> Sealed for Option<F> {}
}

impl IntoCleanup for () {
    fn register(self) {}
}

impl<F: FnOnce() + 'static> IntoCleanup for F {
    fn register(self) {
        on_cleanup(self)
    }
}

impl<F: FnOnce() + 'static> IntoCleanup for Option<F> {
    fn register(self) {
        if let Some(cleanup) = self {
            on_cleanup(cleanup);
        }
    }
}
/// Store a value in the ambient world's context. What components call.
///
/// **The provision is owned by the ambient ownership scope**, exactly like
/// a `signal()` or `effect()` created at the same point: when the
/// [`collect_owned`] scope that ran this `provide` drops, the entry is
/// retracted and whatever it shadowed becomes visible again. Called with
/// no collector active it is world-root-owned, living until the world
/// drops.
///
/// That ownership is the whole point. A `provide`d value routinely
/// *contains* scope-owned handles (a navigator publishing its
/// `active_route`), and a `Copy` handle carries no liveness of its own. An
/// unowned entry therefore outlives its contents: the providing scope
/// drops, its signal slots are freed, and the next `inject` hands out a
/// handle onto a freed slot — whose first read aborts with
/// `stale-signal-handle`. Tying the entry to the same scope as the handles
/// inside it makes that shape unrepresentable.
///
/// For a **world-lifetime service** created lazily during a mount (a
/// per-world theme context, a notifier registry) wrap the call in
/// [`unscoped`] — `unscoped(|| provide(ctx))` — which is the same escape
/// hatch, and for the same reason, as `unscoped(|| signal(0))`.
///
/// For a provision that should live only for a **bounded region** rather
/// than for the ambient scope — a navigator screen publishing its
/// `ScreenNav` from a driver effect, where there is no ambient collector
/// at all — wrap the call in its own [`collect_owned`] and hold the
/// returned `Owned` for exactly as long as the entry should be visible.
/// Prefer that to the `let prev = inject(); provide(x); …; provide(prev)`
/// idiom: restoring by re-providing publishes a *fresh, unowned* copy of
/// `prev`, so a value whose own scope died in the meantime is reinstated
/// as a live-looking entry full of freed handles.
pub fn provide<T: Clone + 'static>(value: T) {
    Active::ctx_provide(TypeId::of::<T>(), Box::new(value));
}

/// Fetch a value from the ambient world's context. What components call.
/// (Framework naming: `provide`/`inject`.)
///
/// Returns the most recent live provision of `T` — see [`provide`] for
/// what keeps one alive.
pub fn inject<T: Clone + 'static>() -> Option<T> {
    Active::ctx_inject(TypeId::of::<T>(), |v| v.downcast_ref::<T>().cloned()).flatten()
}
// ============================================================================
// Ownership — the Copy-handle price. Handles can't refcount, so slots need
// owners: collect_owned() gathers every signal/effect created during a
// closure into an Owned; dropping the Owned frees them (effect cleanups
// first). Creations outside any collector are world-root-owned and freed on
// world drop. Phase 1's Realized holds exactly this.
// ============================================================================

/// The owner of a batch of signal/effect slots (possibly spanning several
/// worlds). Dropping it is the entire teardown story: every collected
/// effect's cleanups run and its subscriptions unlink, then every collected
/// signal slot is freed; stale `Copy` handles left behind panic on use.
/// Items whose world is already dead are skipped (the world drop already
/// tore them down).
pub struct Owned {
    /// The collected items, as the active engine represents them.
    scope: <Active as Engine>::Scope,
    /// Values a higher layer rides on this scope, one per type (see
    /// [`Owned::attachment_mut`]). Dropped with the scope, after its items.
    attachments: Vec<Box<dyn Any>>,
    /// Worlds are thread-local; freeing from another thread could only
    /// silently leak. Reject the shape at compile time.
    _not_send: PhantomData<*const ()>,
}

impl Owned {
    /// Number of collected items (signals + effects + context provisions).
    pub fn len(&self) -> usize {
        Active::scope_len(&self.scope)
    }

    /// True when the scope collected nothing (dropping it is a no-op).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Absorb another scope's slots into this one: `other`'s signals and
    /// effects now live — and die — with `self`. The scene layer (P1) uses
    /// this to fold a component body's `Owned` (an `Element::Owned` wrapper,
    /// collected at build time) into the enclosing `Realized`'s scope, so
    /// one drop still tears down the whole subtree.
    ///
    /// Drop ordering after a merge: `Owned::drop` runs its effects-first
    /// pass over the combined list in item order (self's originals, then
    /// other's), then the signals pass likewise — relative creation order
    /// within each scope is preserved, and every merged cleanup still runs
    /// before any merged signal is freed.
    pub fn merge(&mut self, mut other: Owned) {
        Active::scope_merge(&mut self.scope, std::mem::take(&mut other.scope));
        // Attachments of a type `self` already holds are dropped with
        // `other`; the rest move over.
        for value in std::mem::take(&mut other.attachments) {
            let ty = (*value).type_id();
            if !self.attachments.iter().any(|a| (**a).type_id() == ty) {
                self.attachments.push(value);
            }
        }
        // `other` drops here with an empty item list — a no-op.
    }

    /// The value of type `T` attached to this scope, created with
    /// `T::default()` on first use.
    ///
    /// An attachment is out-of-band data a higher layer rides on a scope
    /// without the kernel knowing what it is — the scene carries a
    /// component's realize hooks this way, on the `Owned` its
    /// `Element::Owned` wrapper already holds, so tooling can bracket a
    /// subtree without changing the shape of the published `Element`
    /// enum. At most one value per type. Attachments are not collected
    /// items: they don't count toward [`len`](Self::len) /
    /// [`is_empty`](Self::is_empty), and they are dropped with the scope
    /// after its items are torn down.
    pub fn attachment_mut<T: Default + 'static>(&mut self) -> &mut T {
        let index = match self.attachments.iter().position(|a| a.is::<T>()) {
            Some(i) => i,
            None => {
                self.attachments.push(Box::new(T::default()));
                self.attachments.len() - 1
            }
        };
        self.attachments[index].downcast_mut::<T>().expect("attachment index matches its type")
    }

    /// Remove and return the attached value of type `T`, if any.
    pub fn take_attachment<T: 'static>(&mut self) -> Option<T> {
        let index = self.attachments.iter().position(|a| a.is::<T>())?;
        let value = self.attachments.swap_remove(index);
        Some(*value.downcast::<T>().expect("attachment index matches its type"))
    }
}

/// An empty scope: dropping it frees nothing. The scene uses one to carry
/// a realize hook (as an attachment) on a subtree whose component body
/// collected nothing.
impl Default for Owned {
    fn default() -> Self {
        Owned { scope: Default::default(), attachments: Vec::new(), _not_send: PhantomData }
    }
}

impl std::fmt::Debug for Owned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Owned({} slots)", self.len())
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // Context retracted first, then effects (cleanups), then signals —
        // see `Engine::scope_drop`. Attachments drop after, with the struct.
        Active::scope_drop(&mut self.scope);
    }
}

/// Run `f`, collecting every signal and effect it creates (in any world)
/// into the returned [`Owned`]. Collectors stack: an inner `collect_owned`'s
/// creations belong to the INNER scope only. If `f` panics, everything
/// collected so far is freed before the panic propagates.
pub fn collect_owned<R>(f: impl FnOnce() -> R) -> (R, Owned) {
    let (result, scope) = Active::collect(f);
    (result, Owned { scope, attachments: Vec::new(), _not_send: PhantomData })
}

/// Run `f` with the running-effect stack SUSPENDED, so [`in_effect`]
/// reports false and every teardown anchored on it — [`on_cleanup`],
/// [`on_scope_drop`], `runtime_vocabulary`'s `*_scoped` timers, a
/// resource's private scope — falls to the ambient ownership collector
/// instead of to whatever effect happens to be running.
///
/// The third member of the [`untrack`] / [`unscoped`] family, and the one
/// that makes "this code is BUILDING a subtree" a real state rather than an
/// accident of who called it. A structural driver (a `Dyn` hole, a keyed
/// list, a navigator's command effect) realizes subtrees from inside its own
/// effect body. Without suspension the code it builds sees that effect as
/// "the current scope", and its teardowns fire on that effect's next re-run
/// — which is neither where the author registered them nor when the thing
/// they belong to actually goes away:
///
/// - a swap navigator mounted inside an auth gate's `when` seated its
///   landing screen inside the gate's driver, which never re-runs, so the
///   screen's registrations outlived every navigation (the leak this was
///   added for — `regression_swap_seated_screen_under_a_reactive_region_fires_its_teardown`);
/// - a screen mounted by the navigator's own driver had its teardown fire
///   on the NEXT navigation instead of at its eviction — right by accident
///   under `LazyDisposing`, and plainly wrong under `LazyPersistent`, where
///   it retracted a still-mounted screen's registration.
///
/// Suspending is only ever safe where tracking is already suspended: a
/// tracked read records against `effect_stack.last()`, so hiding the stack
/// from a region that is *meant* to collect dependencies (a plain `Dyn`
/// closure, an effect body) would silently drop every subscription. Both
/// call sites honor that — [`component_scope`] untracks, and
/// `runtime_scene`'s mount walk is untracked by contract.
///
/// The stack is swapped whole (with its parallel pending-dependency frames,
/// which the tracked-read path expects to stay in lockstep) and restored on
/// the way out, panic included. `enter_stack` is deliberately NOT touched:
/// the world must stay ambient, or the body could not create signals at all.
pub fn unanchored<R>(f: impl FnOnce() -> R) -> R {
    Active::unanchored(f)
}

/// Run a component body: untracked (a run-once body's reads are snapshots by
/// definition — they must not subscribe whatever structural effect happens
/// to be mounting the component), [`unanchored`] (for the same reason, its
/// teardowns belong to the body's own scope and not to that effect's next
/// re-run), with every signal/effect it creates collected into the returned
/// [`Owned`]. This is what `#[component]` will wrap function bodies in
/// (P1/P6); the Owned rides in the `Realized`.
pub fn component_scope<R>(f: impl FnOnce() -> R) -> (R, Owned) {
    collect_owned(|| untrack(|| unanchored(f)))
}
// ============================================================================
// Memo — a derived signal, composed from existing parts: a cache signal fed
// by a Derivation-class effect. Because the result commits like any signal,
// a memo gets the PartialEq cut for free (recompute lands on an equal value
// → downstream effects don't run), and because the recompute is a
// Derivation, every memo settles before any Reaction runs (glitch-free).
// ============================================================================

/// A cached derivation. `Copy`, like every handle: the cache signal and the
/// recomputation effect are OWNED by the collector active at `memo()` time
/// (or the world root), so the handle co-owns nothing — unlike idea-lite's
/// Rc-based `Memo`, whose handle kept the effect alive. Dropping the
/// enclosing `Owned` retires the memo; stale handles panic like any other.
pub struct Memo<T> {
    value: ReadSignal<T>,
    _effect: Effect,
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Memo<T> {}

impl<T> std::fmt::Debug for Memo<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Memo({:?})", self.value)
    }
}

/// Create a derived signal in the ambient world.
///
/// For a single consumer a plain closure (`move || src.get() * 2`) is
/// lighter and needs no machinery — every `IntoValue` prop already accepts
/// one. `memo()` earns its signal when the derivation is shared (one
/// computation instead of N), expensive, or should cut propagation: source
/// changed but the derived value didn't → consumers don't re-run.
#[cfg_attr(debug_assertions, track_caller)]
pub fn memo<T, F>(f: F) -> Memo<T>
where
    T: PartialEq + Clone + 'static,
    F: Fn() -> T + 'static,
{
    memo_at(f, caller_site())
}

/// [`memo`] with the author's creation site passed in explicitly, so
/// `World::memo` (which must go through a closure to enter its world) can
/// forward the same site. See `SignalSlot::created_at`.
fn memo_at<T, F>(f: F, site: SiteLoc) -> Memo<T>
where
    T: PartialEq + Clone + 'static,
    F: Fn() -> T + 'static,
{
    // Initial value computed untracked — the memo's own effect is the one
    // and only subscriber to f's dependencies, wherever memo() was called.
    let initial = untrack(&f);
    let cache = signal_in(None, initial, site);
    let write = cache.write_only();
    let _effect = effect_in(None, EffectClass::Derivation, Box::new(move || write.set(f())));
    Memo { value: cache.read_only(), _effect }
}

impl<T: PartialEq + 'static> Memo<T> {
    /// Tracked read of the cached value — same semantics as [`Signal::get`].
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.value.get()
    }

    /// Untracked read — same semantics as [`Signal::peek`].
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn peek(&self) -> T
    where
        T: Clone,
    {
        self.value.peek()
    }

    /// Tracked borrow-read — same semantics as [`Signal::with`].
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.value.with(f)
    }

    /// The identity KEY of the memo's cache signal (see
    /// [`Signal::raw_id`]) — lets external registries (JS text-binding
    /// tables) subscribe a memo slot exactly like a plain signal.
    pub fn raw_id(&self) -> u64 {
        self.value.raw_id()
    }

    /// The memo's cached value as a plain [`ReadSignal`] — the observe
    /// half, mirroring [`Signal::read_only`]. Lets a derived value flow
    /// into `ReadSignal<T>`-typed surfaces (a component prop, an
    /// old-core-signature mirror like the glue's `current_breakpoint`)
    /// without exposing the recompute effect. The effect handle stays
    /// with whoever owns the memo; the returned handle only reads.
    pub fn read_only(&self) -> ReadSignal<T> {
        self.value
    }
}

// ============================================================================
// Value<T> — a property that may or may not be dynamic. This is the props
// model: component parameters are `impl IntoValue<T>`, the CALLER picks
// reactivity per argument (literal → Const, signal/closure → Dyn), and
// whoever consumes the prop binds it — zero effects for Const, exactly one
// for Dyn.
// ============================================================================

pub enum Value<T> {
    /// Statically known to never change. Bound once; no reactive machinery.
    Const(T),
    /// Re-evaluated reactively; reads inside are tracked by the binding effect.
    Dyn(Box<dyn Fn() -> T>),
}

impl<T: 'static> Value<T> {
    /// Drive a setter with this value: once for Const (returns `None`), or
    /// via a tracking effect for Dyn (re-applied whenever its dependencies
    /// effectively change). The binding effect is created in the ambient
    /// ownership scope — collect it (via [`collect_owned`]) into whatever
    /// owns the bound thing; dropping that `Owned` stops the updates.
    #[must_use = "for Dyn values the updates stop when the collected binding Effect's Owned scope is dropped — collect it where the bound node lives"]
    pub fn bind(self, mut apply: impl FnMut(&T) + 'static) -> Option<Effect> {
        match self {
            Value::Const(v) => {
                apply(&v);
                None
            }
            Value::Dyn(f) => Some(effect(move || apply(&f()))),
        }
    }

    /// Read the current value. For consumers that want a one-off look rather
    /// than a live binding. (A Dyn read inside a running effect still tracks
    /// through the closure's own `get()` calls.)
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        match self {
            Value::Const(v) => v.clone(),
            Value::Dyn(f) => f(),
        }
    }
}

/// Conversions into `Value<T>`. Components take `impl IntoValue<T>` and
/// callers pass literals, signals, or closures with one call shape.
pub trait IntoValue<T> {
    fn into_value(self) -> Value<T>;
}

/// Pass-through, so a prop can be forwarded to a child component untouched.
impl<T> IntoValue<T> for Value<T> {
    fn into_value(self) -> Value<T> {
        self
    }
}

/// A signal IS a dynamic value: the binding effect subscribes through get().
impl<T: PartialEq + Clone + 'static> IntoValue<T> for Signal<T> {
    fn into_value(self) -> Value<T> {
        Value::Dyn(Box::new(move || self.get()))
    }
}

/// The read half works as a prop source exactly like a full signal.
impl<T: PartialEq + Clone + 'static> IntoValue<T> for ReadSignal<T> {
    fn into_value(self) -> Value<T> {
        Value::Dyn(Box::new(move || self.get()))
    }
}

/// A memo is a dynamic value like any signal. (The handle is Copy; the
/// memo's lifetime is its owning scope's, not the closure's.)
impl<T: PartialEq + Clone + 'static> IntoValue<T> for Memo<T> {
    fn into_value(self) -> Value<T> {
        Value::Dyn(Box::new(move || self.get()))
    }
}

/// Any zero-arg closure is a dynamic (usually derived) value.
impl<T, F> IntoValue<T> for F
where
    F: Fn() -> T + 'static,
{
    fn into_value(self) -> Value<T> {
        Value::Dyn(Box::new(self))
    }
}

/// Plain values of common types are Const. (A blanket `for T` impl would
/// collide with the closure impl, so these are enumerated.)
macro_rules! impl_const_into_value {
    ($($t:ty),+ $(,)?) => {$(
        impl IntoValue<$t> for $t {
            fn into_value(self) -> Value<$t> {
                Value::Const(self)
            }
        }
    )+};
}

impl_const_into_value! {
    String, bool, char,
    i8, i16, i32, i64, i128, isize,
    u8, u16, u32, u64, u128, usize,
    f32, f64,
}

impl IntoValue<String> for &'static str {
    fn into_value(self) -> Value<String> {
        Value::Const(self.into())
    }
}
