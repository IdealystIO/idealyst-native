//! A deliberately-heavy fake `Element::External` SDK, used to *measure*
//! the code-splitting win from lazy handler registration.
//!
//! The whole crate exists to answer one question with bytes: when a
//! third-party external handler is registered **eagerly** (at boot) vs
//! **lazily** (from inside a lazy component's chunk via
//! [`runtime_core::defer_external_registration`]), where does its code +
//! data end up?
//!
//! [`HEAVY`] is 512 KiB of static data reachable *only* through
//! [`build_heavy`]. wasm-split's reachability analysis places a symbol
//! in `main.wasm` iff main can reach it:
//!
//! - **Eager** ([`register`] from `register_extensions`): main reaches
//!   `build_heavy` → HEAVY stays in `main.wasm`.
//! - **Lazy** ([`register_lazy`] from the chunk body): only the chunk
//!   reaches `build_heavy` → the release data-prune drops HEAVY from
//!   `main.wasm`, and it rides in the chunk instead.
//!
//! The two sibling app crates (`eager/`, `lazy/`) differ *only* in which
//! path they use; the `prune-regression` runner builds both and asserts
//! the main-bundle byte delta.

use runtime_core::{Backend, RegisterExternal};
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};

/// 512 KiB — big enough that its presence/absence in `main.wasm` is
/// unmistakable against the framework's ~MB baseline, and it dwarfs any
/// build-to-build noise.
const HEAVY_LEN: usize = 512 * 1024;

/// The heavy payload. Reachable only through [`build_heavy`]; that
/// reachability is the entire measurement.
static HEAVY: [u8; HEAVY_LEN] = build_heavy_table();

/// Fill the table at compile time with a mix (multiply + variable shift
/// + xor) so the bytes aren't a trivial ramp — keeps the data segment
/// from compressing to nothing, so the delta shows in gzip too. The
/// runner asserts on the *raw* byte delta, which is entropy-independent.
const fn build_heavy_table() -> [u8; HEAVY_LEN] {
    let mut a = [0u8; HEAVY_LEN];
    let mut i = 0;
    while i < HEAVY_LEN {
        let x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        a[i] = ((x >> ((i % 57) as u64)) as u8) ^ (i as u8);
        i += 1;
    }
    a
}

/// Observable sink: storing the read byte into a global forces the
/// compiler to keep the `HEAVY[idx]` load (a store to a `static` is an
/// observable side effect it can't elide), so the table can't be
/// optimized away as dead.
static HEAVY_SINK: AtomicU8 = AtomicU8::new(0);

/// Marker props for the heavy external primitive — a leaf widget; the
/// point is the handler's reachable footprint, not its configuration.
pub struct HeavyProps;

/// Backend-neutral handler for the heavy external. Reads [`HEAVY`] at a
/// **runtime-opaque** index — `now_micros()` is an installed-time-source
/// call the optimizer can't fold through, so the 512 KiB table is
/// genuinely referenced at runtime and survives dead-code elimination in
/// whichever module reaches it. Returns a plain view; the bytes are the
/// payload.
pub fn build_heavy<B: Backend>(_props: &Rc<HeavyProps>, b: &mut B) -> B::Node {
    let idx = (runtime_core::time::now_micros() as usize) % HEAVY_LEN;
    HEAVY_SINK.store(HEAVY[idx], Ordering::Relaxed);
    b.create_view(&Default::default())
}

/// Construct the heavy external element. Called from both app variants —
/// inside the lazy component's body — so the *rendered* tree is identical and the
/// only variable is where the handler was registered.
pub fn widget() -> runtime_core::Element {
    use runtime_core::IntoElement;
    runtime_core::external(HeavyProps).into_element()
}

/// EAGER registration. Backend-neutral (`register_external` on a generic
/// `B: RegisterExternal` resolves to the trait method unambiguously).
/// Called from the eager app's `register_extensions` at boot, so
/// `main.wasm` statically reaches `build_heavy` and keeps HEAVY.
pub fn register<B: RegisterExternal>(backend: &mut B) {
    backend.register_external::<HeavyProps, _>(build_heavy::<B>);
}

/// LAZY registration. Queues the same registration from inside a lazy
/// chunk body via the deferral seam, keyed to the concrete web backend.
/// Because the closure — and the `build_heavy`/HEAVY it reaches — is
/// constructed only here (called only from the chunk), `main.wasm` never
/// statically reaches it, so the data-prune drops HEAVY from main.
#[cfg(target_arch = "wasm32")]
pub fn register_lazy() {
    runtime_core::defer_external_registration::<backend_web::WebBackend, _>(|b| {
        register::<backend_web::WebBackend>(b);
    });
}

/// Host stub so workspace-wide `cargo check`/`test` (which touch this
/// crate even though it only ships to web) compiles. Native targets
/// register eagerly and have no chunk, so a no-op is correct.
#[cfg(not(target_arch = "wasm32"))]
pub fn register_lazy() {}
