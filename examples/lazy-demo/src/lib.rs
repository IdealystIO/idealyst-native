//! `lazy-demo` — parent app for the lazy-primitive demo.
//!
//! Lays out a small UI that mounts a `Primitive::Lazy`, drives a
//! "status" Text from the `on_state` callback so you can watch the
//! lifecycle, and renders a placeholder while the chunk is loading.
//!
//! What you'll see per target:
//!
//! - **Terminal / iOS / macOS / Android**: the chunk is a normal
//!   cargo dep on these targets. `register_extensions` registers a
//!   thunk; the framework dispatches synchronously when it walks
//!   the `Primitive::Lazy`. Status flashes `Loaded → Rendered` and
//!   the chunk's content is mounted inline.
//!
//! - **Web**: the placeholder mounts and `Loading` fires. The chunk
//!   doesn't load — the dynamic-import web handler ships in PR 6
//!   of the lazy-primitive series. Status sticks at `Loading...`.
//!
//! Once PR 6 lands, the web target will produce both `pkg/` (parent)
//! and `pkg-demo/` (chunk) wasm bundles and the chunk will load on
//! first `Lazy` mount.

use std::rc::Rc;

use runtime_core::primitives::lazy::{lazy, ChunkId, LazyState};
use runtime_core::{signal, ui, IntoPrimitive, Primitive};

use lazy_demo_chunk::ChunkProps;

// Forward-compatible: once the `chunks!()` macro lands (PR 2),
// `crate::chunks::DEMO` replaces this constant and codegen'd
// `register()` replaces the manual wiring in `register_extensions`.
pub const DEMO: ChunkId = ChunkId::new("demo");

pub fn app() -> Primitive {
    // Drive the visible status string from the chunk's lifecycle
    // state. The walker fires the callback synchronously; on
    // native that means we see `Loaded → Rendered` immediately
    // during the first walk.
    let state = signal!(LazyState::Loading);
    let state_for_label = state.clone();
    let status = move || match state_for_label.get() {
        LazyState::Loading => "status: Loading chunk...".to_string(),
        LazyState::Loaded => "status: Loaded; mounting...".to_string(),
        LazyState::Rendered => "status: Rendered ✓".to_string(),
        LazyState::Error(e) => format!("status: Error — {e}"),
    };

    let chunk: Primitive = lazy::<ChunkProps>(
        DEMO,
        ChunkProps {
            greeting: "Hello from the lazy chunk!".to_string(),
            multiplier: 42,
        },
    )
    .on_state(move |s| state.set(s))
    .placeholder(|| {
        let placeholder = "(loading chunk...)";
        ui! {
            View {
                Text { placeholder }
            }
        }
        .into_primitive()
    })
    .into_primitive();

    let title = "Lazy Primitive Demo";
    let intro = "The status line below reflects the chunk's lifecycle:";

    ui! {
        View {
            Text { title }
            Text { intro }
            Text { status() }
            chunk
        }
    }
}

// Per-target SDK-handler registration hooks the CLI-generated
// wrappers invoke before mount. Each one registers the chunk's
// native dispatch thunk via `runtime_core::primitives::lazy::register`;
// the walker consults that registry when it builds a `Primitive::Lazy`.
//
// On wasm, the call is a no-op — chunks load dynamically (PR 6).

fn register_demo_chunk_thunk() {
    runtime_core::primitives::lazy::register(DEMO, |payload| {
        // Downcast the type-erased payload back to the concrete
        // props type the chunk crate expects. The framework
        // guarantees the payload type matches whatever the
        // matching `lazy::<T>(...)` call site supplied — a
        // mismatch is a bug and panicking is the right response
        // (better than silently rendering with default props).
        let props = payload
            .downcast::<ChunkProps>()
            .expect("lazy-demo: chunk payload must be ChunkProps");
        lazy_demo_chunk::app(Rc::unwrap_or_clone(props))
    });
}

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(_backend: &mut backend_web::WebBackend) {
    register_demo_chunk_thunk();
}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_ios_mobile::IosBackend) {
    register_demo_chunk_thunk();
}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_android_mobile::AndroidBackend) {
    register_demo_chunk_thunk();
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn register_extensions(_backend: &mut backend_terminal::TerminalBackend) {
    register_demo_chunk_thunk();
}
