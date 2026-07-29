//! `canvas-native` — the native-2D-engine renderer for the `canvas` SDK.
//!
//! Registers an [`Element::External`](runtime_core::Element) handler for
//! `canvas_core::CanvasProps` that replays the author's [`Scene`] with
//! the platform's native 2D engine. The app selects this renderer (over
//! `canvas-vello`) by calling [`register`] once at bootstrap.
//!
//! Per-target impls live in cfg-gated modules; only one compiles per
//! build. Targets with no native module fall back to a no-op `register`
//! (the framework draws its "not supported" placeholder) — use
//! `canvas-vello` for those.
//!
//! [`Scene`]: canvas_core::Scene
#![deny(missing_docs)]

// Shared glyph-outline expansion for `DrawOp::Glyphs`, used by every CPU
// backend (web / apple / android). Gated to those targets so the fallback
// build (no native 2D engine) doesn't carry an unused skrifa dependency.
// (The native halves are old-core-only for now — see the `new-core`
// feature notes below — so they drop out of new-core builds with their
// consumers.)
#[cfg(any(
    target_arch = "wasm32",
    all(
        any(target_os = "ios", target_os = "macos", target_os = "android"),
        not(target_arch = "wasm32"),
        not(feature = "new-core")
    )
))]
mod glyphs;

// The web module is compiled on BOTH cores: its rasterizer / layer /
// capture machinery is core-free and shared verbatim; only its
// old-core `register` + `build_canvas` (the `effect!` wrapper) are
// gated `not(new-core)` inside the module.
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(all(target_arch = "wasm32", not(feature = "new-core")))]
pub use web::register;
// Reusable Canvas2D rasterizer + capture helper — `canvas-vello`'s web renderer
// calls these as its WebGPU-unavailable fallback (renders into the graphics
// primitive's own `<canvas>`, same output as this crate's standalone handler)
// and for self-capture on its GPU path (captureStream works on any canvas).
#[cfg(target_arch = "wasm32")]
pub use web::{make_2d_rasterizer, publish_capture_stream};

// New-core web leg: the same handler over the scene registry (see
// web_newcore.rs — old `build_canvas` call-for-call, world effect
// instead of `effect!`).
#[cfg(all(target_arch = "wasm32", feature = "new-core"))]
mod web_newcore;
#[cfg(all(target_arch = "wasm32", feature = "new-core"))]
pub use web_newcore::register;

// Shared CoreGraphics painter for the Apple platforms (iOS + macOS).
// The Scene→CGContext op-replay is platform-identical; only context
// acquisition + the bezier/color vtable differ per backend.
#[cfg(all(
    any(target_os = "ios", target_os = "macos"),
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod apple;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32"), not(feature = "new-core")))]
mod ios;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32"), not(feature = "new-core")))]
pub use ios::register;

#[cfg(all(
    target_os = "macos",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod macos;
#[cfg(all(
    target_os = "macos",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
pub use macos::register;

#[cfg(all(
    target_os = "android",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod android;
#[cfg(all(
    target_os = "android",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
pub use android::register;

#[cfg(all(
    not(any(
        target_arch = "wasm32",
        target_os = "ios",
        target_os = "android",
        target_os = "macos"
    )),
    not(feature = "new-core")
))]
mod fallback {
    use runtime_core::Backend;

    /// No-op `register` for targets without a native canvas module
    /// (desktop uses `canvas-vello`). Still registers the wire serde so a
    /// canvas can round-trip over the runtime-server wire to a client that
    /// *does* have a renderer.
    pub fn register<B: Backend>(_backend: &mut B) {
        canvas_core::ensure_wire_serde();
    }
}
#[cfg(all(
    not(any(
        target_arch = "wasm32",
        target_os = "ios",
        target_os = "android",
        target_os = "macos"
    )),
    not(feature = "new-core")
))]
pub use fallback::register;

// New-core, non-web targets: the native CoreGraphics/android painters
// (and the GPU `canvas-vello` renderer) are old-core-only for now —
// their ports ride the same seam (register a `CanvasPrim` handler on
// the platform backend's registry; wrap any author callbacks with that
// backend's `newcore::schedule_flush`, the External residual named in
// each backend's newcore.rs module docs). Until then, `register`
// installs the frozen External-placeholder degradation path so a canvas
// in a new-core native tree renders the labeled "unsupported" box
// instead of panicking at realize (unregistered payloads panic on the
// scene registry).
#[cfg(all(not(target_arch = "wasm32"), feature = "new-core"))]
mod native_newcore {
    use std::any::Any;
    use std::rc::Rc;

    use canvas_core::CanvasPrim;
    use runtime_scene::{Element, MountCx, Registry};
    use runtime_vocabulary::caps::ExternalOps;
    use runtime_vocabulary::style_attach::{attach_style, on_teardown, StyleServices};

    /// Placeholder `register` for new-core native targets (no ported
    /// renderer yet). Mirrors the old walker's unregistered-External
    /// posture: `create_external` placeholder + author style.
    pub fn register<H>(registry: &mut Registry<H>)
    where
        H: ExternalOps + StyleServices + 'static,
    {
        canvas_core::ensure_wire_serde();
        registry.register::<CanvasPrim, _>(
            |cx: &mut MountCx<'_, H>, prim: &Rc<CanvasPrim>, _children: Vec<Element>| {
                let backend = cx.backend().clone();
                let payload: Rc<dyn Any> = prim.props.clone();
                let node = backend.borrow_mut().create_external(
                    std::any::TypeId::of::<canvas_core::CanvasProps>(),
                    std::any::type_name::<canvas_core::CanvasProps>(),
                    &payload,
                    &runtime_core::accessibility::AccessibilityProps::default(),
                );
                if let Some(style) = prim.take_style() {
                    attach_style(&backend, &node, style);
                }
                // Old walker parity: External mounts release at teardown.
                let backend_for_drop = backend.clone();
                let node_for_drop = node.clone();
                on_teardown(move || {
                    backend_for_drop.borrow_mut().release_external(&node_for_drop);
                });
                node
            },
        );
    }
}
#[cfg(all(not(target_arch = "wasm32"), feature = "new-core"))]
pub use native_newcore::register;

/// Regression tests for the `self-register` feature gate. The bug being
/// prevented: the inventory ctor is a LINK-TIME anchor — with it always on,
/// merely depending on this crate (as `canvas-vello` does for its Canvas2D
/// fallback delegate) made the rasterizer + glyph stack (skrifa/read_fonts,
/// ~670 KB) reachable from `main`'s ctors, pinning it in a lazy web bundle's
/// `main.wasm` even though the canvas itself lived in a wasm-split chunk.
///
/// Link-graph anchoring isn't observable from a unit test, so this asserts
/// the closest reachable state: what the crate submits into the inventory
/// registry under each feature setting, on the host target (the submit sites
/// are gated identically on web/iOS/Android). Run both:
///   cargo test -p canvas-native
///   cargo test -p canvas-native --no-default-features
// (`not(new-core)`: the new core has NO inventory self-registration by
// design — the registry is built explicitly at boot — so the old-core
// macos registrar ctor is gated out with its module and these link-graph
// assertions only apply to the old-core build.)
#[cfg(all(
    test,
    target_os = "macos",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod self_register_gate_tests {
    #[cfg(feature = "self-register")]
    #[test]
    fn regression_self_register_default_submits_the_registrar() {
        let count = inventory::iter::<backend_macos::MacosExternalRegistrar>
            .into_iter()
            .count();
        assert_eq!(count, 1, "default features must self-register the handler");
    }

    #[cfg(not(feature = "self-register"))]
    #[test]
    fn regression_delegate_only_build_links_no_registrar_ctor() {
        let count = inventory::iter::<backend_macos::MacosExternalRegistrar>
            .into_iter()
            .count();
        assert_eq!(
            count, 0,
            "without self-register the ctor must not exist — it would anchor \
             the rasterizer + glyph stack in a lazy bundle's main.wasm"
        );
    }
}
