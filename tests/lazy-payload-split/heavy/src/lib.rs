//! A deliberately-heavy fake third-party SDK, used to *measure* the
//! code-splitting win from registering its mount handler late.
//!
//! The whole crate exists to answer one question with bytes: when a
//! third-party payload's **handler** is registered at boot (**eager**) vs
//! from inside a `#[component(lazy)]` chunk (**lazy**), where does its
//! code + data end up?
//!
//! [`HEAVY`] is 512 KiB of static data reachable *only* through
//! [`mount_heavy`], the payload's [`Registry`] handler. wasm-split's
//! reachability analysis places a symbol in `main.wasm` iff main can
//! reach it:
//!
//! - **Eager** ([`register`] from the app's `register_scene_extensions`):
//!   main names `mount_heavy` → it reaches `HEAVY` → `HEAVY` stays in
//!   `main.wasm`.
//! - **Lazy** ([`register_from_chunk`] from the `#[component(lazy)]`
//!   body): main names only [`HeavyProps`] (a unit struct — the app
//!   declares it late-bound with `Registry::defer::<HeavyProps>()`, which
//!   costs main a compile-time `TypeId` and nothing else) and
//!   [`widget`]. Nothing in main reaches `mount_heavy`, so the release
//!   data-prune drops `HEAVY` from `main.wasm` and it rides in the chunk.
//!
//! **`widget()` is identical in both variants, and both apps render it
//! from `app()`.** In the lazy app the item has no handler when realize
//! first meets it, so the scene PARKS it behind a placeholder and
//! completes the mount in place when the chunk's registration lands —
//! the capability under test. The two sibling app crates (`eager/`,
//! `lazy/`) differ in exactly one line: their `register_scene_extensions`
//! body. The `prune-regression` runner builds both and asserts the
//! main-bundle byte delta.
//!
//! [`Registry`]: runtime_scene::Registry

use std::rc::Rc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use runtime_core::{ui, Element};
use runtime_scene::{item, MountCx, Registry};
use runtime_vocabulary::caps::ViewOps;

/// 512 KiB — big enough that its presence/absence in `main.wasm` is
/// unmistakable against the framework's ~MB baseline, and it dwarfs any
/// build-to-build noise.
const HEAVY_LEN: usize = 512 * 1024;

/// The heavy payload. Reachable only through [`mount_heavy`]; that
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

/// Runtime-opaque index source. A load from a mutable `static` cannot
/// be constant-folded, so the `HEAVY[idx]` below is a genuine runtime
/// indexed load — the optimizer cannot prove which byte is wanted and
/// therefore cannot drop the table. Bumped on every call so repeat
/// mounts don't converge to a foldable constant either.
static CURSOR: AtomicUsize = AtomicUsize::new(0);

/// Observable sink: storing the read byte into a global forces the
/// compiler to keep the `HEAVY[idx]` load (a store to a `static` is an
/// observable side effect it can't elide), so the table can't be
/// optimized away as dead.
static SINK: AtomicU8 = AtomicU8::new(0);

/// Read one byte of [`HEAVY`] at a runtime-opaque index and publish it.
/// The value is rendered too, so the payload is load-bearing all the way
/// to the DOM — nothing about this call is elidable.
fn touch_payload() -> u8 {
    let idx = CURSOR.fetch_add(0x9E37, Ordering::Relaxed) % HEAVY_LEN;
    let byte = HEAVY[idx];
    SINK.store(byte, Ordering::Relaxed);
    byte
}

/// The payload type. A unit struct on purpose: naming it from `main`
/// (which the lazy app does, to declare it late-bound) must cost main
/// nothing but a `TypeId`.
pub struct HeavyProps;

/// The payload's mount handler — the heavy half. Reaches [`HEAVY`], so
/// whichever module names this function is the module the 512 KiB rides
/// in. Caps-generic (`ViewOps` is all it needs), so one handler serves
/// every host.
pub fn mount_heavy<H>(
    cx: &mut MountCx<'_, H>,
    _props: &Rc<HeavyProps>,
    children: Vec<Element>,
) -> H::Node
where
    H: ViewOps,
{
    let byte = touch_payload();
    let mut node = cx.backend().borrow_mut().create_view(&Default::default());
    // The byte is rendered, so a browser smoke check proves the handler
    // actually ran — in the lazy variant that means the parked item was
    // drained after its chunk loaded.
    let mut kids: Vec<Element> = vec![ui! {
        text { format!("heavy payload byte: {byte:#04x}") }
    }];
    kids.extend(children);
    // One call per parent: the child-splice counter starts at 0.
    cx.realize_children_into(&mut node, kids);
    node
}

/// Construct the heavy payload's element. Tiny, and called from `app()`
/// in BOTH variants — the rendered tree is identical, so the only
/// variable is where the handler was registered.
pub fn widget() -> Element {
    item(HeavyProps, vec![])
}

/// EAGER registration, from the app's boot seam. Main names
/// [`mount_heavy`] here, so `main.wasm` keeps [`HEAVY`].
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ViewOps,
{
    registry.register::<HeavyProps, _>(mount_heavy::<H>);
}

/// LAZY registration, from inside a `#[component(lazy)]` body. Queues
/// the same handler through the scene's late-registration mailbox, keyed
/// to the concrete web backend (a chunk body has no registry in hand).
/// Because the closure — and the `mount_heavy`/`HEAVY` it reaches — is
/// constructed only here, and here is only called from the chunk,
/// `main.wasm` never statically reaches it and the data-prune drops
/// `HEAVY` from main.
///
/// Requires the app to have declared `Registry::defer::<HeavyProps>()`
/// at boot; otherwise realize would have panicked on the payload long
/// before this ran, and `register_deferred` says so. When a boot handler
/// already won (the eager sibling), `register_deferred` is inert — which
/// is why BOTH variants can call this from their chunk unconditionally
/// and hold the chunk-side work constant.
#[cfg(target_arch = "wasm32")]
pub fn register_from_chunk() {
    runtime_scene::defer_registration::<backend_web::WebBackend, _>(|registry| {
        registry.register_deferred::<HeavyProps, _>(mount_heavy::<backend_web::WebBackend>);
    });
}

/// Host stub so workspace-wide `cargo check`/`test` (which touch this
/// crate even though it only ships to web) compiles. Native targets
/// register eagerly and have no chunk, so a no-op is correct.
#[cfg(not(target_arch = "wasm32"))]
pub fn register_from_chunk() {}
