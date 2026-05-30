//! Prototype: **true in-place SSR hydration** (DOM adoption) + the
//! viewport-determinism solution.
//!
//! The SAME [`app`] tree renders on the server (`examples/ssr.rs` →
//! `backend-ssr`) and on the client (the CLI-generated `wasm-bindgen`
//! wrapper → `backend-web`). On the client we boot in **hydration mode**:
//! instead of clearing `#app` and rebuilding, `WebBackend::hydrate`
//! ADOPTS the server's existing DOM — reusing every node (and the
//! browser's already-computed layout) and just wiring handlers +
//! reactivity onto it.
//!
//! Build + serve via the CLI:
//! ```text
//! cd examples/hydration-demo
//! idealyst build --web                                   # → dist/web (bundle)
//! cargo run -p hydration-demo --example ssr -- dist/web  # SSR-render index.html
//! idealyst serve                                          # serves dist/web; hydrates
//! ```
//!
//! ## The viewport determinism problem (and the fix)
//!
//! SSR has no real viewport, so it renders at a fixed assumed size
//! (1280×800) and embeds it as `#app[data-ssr-viewport]`. A mobile
//! client's real viewport would render DIFFERENT nodes (see the `when`
//! below) — diverging from the server's DOM and breaking adoption. The
//! fix (in the CLI web wrapper's `start_local`): the client **seeds the
//! SSR-assumed viewport before its first render** so the hydration pass
//! matches the server exactly (clean adoption), THEN installs the real
//! viewport observer so reactivity reconciles to the actual size — after
//! adoption, not during it.

use idea_ui::{install_idea_theme, light_theme, Typography};
use runtime_core::{rx, signal, ui, viewport_size, when, Element, Signal};

/// Cross-platform app tree — byte-for-byte the same on server + client.
///
/// Exercises the three things hydration must get right:
/// 1. **a click handler** (Increment) wired onto an adopted `<button>`,
/// 2. **reactive text** (the count) retargeted on an adopted `<span>`,
/// 3. **viewport-conditional content** (the `when`) — the determinism
///    test: SSR renders the `>= 600px` branch at its seeded viewport, the
///    client adopts it, then the real viewport drives a reactive swap.
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let count: Signal<i32> = signal!(0);
    let inc = move || count.update(|c| *c += 1);

    // Viewport-conditional content — the determinism test. Bound to a
    // `let` and interpolated as a bare-identifier child (matching how the
    // website's responsive TOC is wired), so the `when` reactive region
    // mounts into the tree.
    let viewport_branch = when(
        move || viewport_size().get().width >= 600.0,
        || ui! { Typography(content = "WIDE viewport branch (>= 600px)".to_string()) },
        || ui! { Typography(content = "NARROW viewport branch (< 600px)".to_string()) },
    );

    ui! {
        view {
            Typography(content = "SSR in-place hydration prototype".to_string())
            Typography(content = rx!(format!("count = {}", count.get())), muted = true)
            button(label = "Increment".to_string(), on_click = inc)
            viewport_branch
        }
    }
}

/// SDK-registration hook the CLI-generated wrappers call before mount.
/// No third-party SDKs here, so it's an empty generic over `Backend` —
/// backend-agnostic, no per-target `cfg` and no `backend-*` dep. The
/// wasm wrapper boots + HYDRATES via `WebBackend::hydrate` (see the
/// crate-level docs); this crate stays platform-neutral.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
