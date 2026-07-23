//! `prune-repro` — reproduce the `--data-prune` instability with a shape
//! closer to a real app than `examples/lazy-canvas`:
//!
//! - TWO lazy components (two split points → the shared `chunk_0` module is
//!   actually populated, exercising the multi-chunk emit path).
//! - `#[lazy_component]` per the lazy-loading guide (not the raw `lazy!`
//!   block), with the vello registration deferred from inside each body.
//! - `canvas-vello` with DEFAULT features — the glyph stack (skrifa,
//!   read_fonts) stays in, like a text-capable canvas component.
//!
//! Build twice and compare behavior:
//!   idealyst build --web --release                  (prune on with the current installed CLI)
//!   idealyst build --web --release --no-data-prune  (control)

use runtime_core::{lazy_component, ui, Element};

pub fn app() -> Element {
    ui! {
        view {
            text { "prune repro: two lazy canvases" }
            LazyCanvasA(seed = 1u32)
            LazyCanvasB(seed = 2u32)
        }
    }
}

/// First lazy canvas — registers the vello renderer from inside its chunk.
#[lazy_component]
fn LazyCanvasA(#[prop(static)] seed: u32) -> Element {
    #[cfg(target_arch = "wasm32")]
    runtime_core::defer_external_registration::<backend_web::WebBackend, _>(|b| {
        canvas_vello::register(b);
    });
    canvas_scene(seed)
}

/// Chunk-only MUTABLE statics with non-zero initializers land in the wasm
/// `.data` segment (segment != 0). This is the shape that trips the prune
/// hole: `--data-prune` zeroes them in main (they're ≥ 24 bytes and
/// chunk-only), but the chunk emit only re-materializes symbols from
/// segment 0 (.rodata) — so NOTHING initializes these at runtime and the
/// chunk reads zeros. Real-world equivalents: once-cells, seeded counters,
/// fn-pointer tables, any `static` with interior mutability.
#[cfg(target_arch = "wasm32")]
static B_PALETTE: [core::sync::atomic::AtomicU32; 8] = {
    use core::sync::atomic::AtomicU32;
    [
        AtomicU32::new(0xDC3C3C), // B's red — reads 0 when the hole bites
        AtomicU32::new(0xF0C828),
        AtomicU32::new(0x101010),
        AtomicU32::new(0x202020),
        AtomicU32::new(0x303030),
        AtomicU32::new(0x404040),
        AtomicU32::new(0x505050),
        AtomicU32::new(0x606060),
    ]
};

/// Second lazy canvas — same registration (the deferred queue dedups by
/// handler type; registering twice is the documented-safe pattern since either
/// chunk can load first). Its colors come from `B_PALETTE`, and a zeroed
/// palette makes it draw nothing — mimicking "the chunk never loaded".
#[lazy_component]
fn LazyCanvasB(#[prop(static)] seed: u32) -> Element {
    #[cfg(target_arch = "wasm32")]
    runtime_core::defer_external_registration::<backend_web::WebBackend, _>(|b| {
        canvas_vello::register(b);
    });
    canvas_scene_b(seed)
}

/// B's scene, colored from the chunk-only mutable `B_PALETTE`. A zeroed
/// palette (the prune hole) draws nothing.
#[cfg(target_arch = "wasm32")]
fn canvas_scene_b(seed: u32) -> Element {
    use canvas::prelude::*;
    use runtime_core::IntoElement;
    // A runtime-dependent write keeps the palette a live mutable static —
    // without it LLVM const-folds the atomic loads and drops the static
    // entirely (which is why the first version of this repro didn't trip).
    // `seed` crosses the split boundary through an extern import, so the
    // optimizer can't prove this branch dead. Real components write to
    // their statics (once-cells, registries) and get this for free.
    if seed == 999 {
        B_PALETTE[0].store(0, core::sync::atomic::Ordering::Relaxed);
    }
    // Seed-indexed reads keep the WHOLE array live (≥ 24 bytes), so it can't
    // be shrunk below the prune threshold like a two-element read can.
    let outer = B_PALETTE[(seed as usize) & 7].load(core::sync::atomic::Ordering::Relaxed);
    let inner = B_PALETTE[(seed as usize + 1) & 7].load(core::sync::atomic::Ordering::Relaxed);
    canvas::Canvas(CanvasProps {
        draw: canvas::draw(move |s: &mut Scene| {
            if outer == 0 {
                // Zeroed palette → draw nothing (the visible failure).
                return;
            }
            s.path().add_path(Path::rect(0.0, 0.0, 300.0, 200.0));
            s.fill(Color::new((outer >> 16) as u8, (outer >> 8) as u8, outer as u8, 255));
            s.path().add_path(Path::rect(60.0, 50.0, 180.0, 100.0));
            s.fill(Color::new((inner >> 16) as u8, (inner >> 8) as u8, inner as u8, 255));
        }),
        ..Default::default()
    })
    .into_element()
}

#[cfg(not(target_arch = "wasm32"))]
fn canvas_scene_b(seed: u32) -> Element {
    canvas_scene(seed)
}

/// A fixed-size canvas with a seed-varied drawing, so A and B are visually
/// distinguishable (A: blue outer, B: red outer).
fn canvas_scene(seed: u32) -> Element {
    use canvas::prelude::*;
    use runtime_core::IntoElement;
    let outer = if seed == 1 {
        Color::new(40, 120, 240, 255)
    } else {
        Color::new(220, 60, 60, 255)
    };
    canvas::Canvas(CanvasProps {
        draw: canvas::draw(move |s: &mut Scene| {
            s.path().add_path(Path::rect(0.0, 0.0, 300.0, 200.0));
            s.fill(outer);
            s.path().add_path(Path::rect(60.0, 50.0, 180.0, 100.0));
            s.fill(Color::new(240, 200, 40, 255));
        }),
        ..Default::default()
    })
    .into_element()
}

/// No eager registration — both renderers register lazily, inside their chunks.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
