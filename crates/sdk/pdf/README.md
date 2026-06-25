# `pdf` — render PDF pages on the GPU

Renders a PDF page into the UI through the [`canvas`](../canvas/) primitive, so
it inherits the GPU pipeline ([`vello`](../canvas/vello/) on Metal / Vulkan /
WebGPU) where available and the CPU renderer ([`canvas-native`](../canvas/native/))
where not. **Pure Rust** — PDF parsing + interpretation is
[`hayro`](https://github.com/LaurenzV/hayro); there's no native pdfium/mupdf blob
and no JNI.

```rust
let bytes = std::fs::read("doc.pdf").unwrap();
let element = pdf::Pdf(pdf::PdfView { bytes, page: 0, width: 800.0 });
```

## How it works

```
hayro-syntax (parse) ─▶ hayro-interpret::interpret_page(page, &mut SceneDevice)
                                                            │
   SceneDevice : hayro_interpret::Device  ── records ──▶  canvas_core::Scene
                                                            │
                            canvas-vello (GPU) / canvas-native (CPU)
                                                            │
                                       vello → Metal / Vulkan / WebGPU
```

[`SceneDevice`] implements `hayro_interpret::Device` and records every drawing
instruction into a renderer-agnostic [`canvas_core::Scene`]:

| PDF instruction        | Scene op                                            |
| ---------------------- | --------------------------------------------------- |
| paths (fill/stroke)    | `Fill` / `Stroke` (wrapped in `Save·Transform`)     |
| text (embedded sfnt)   | `DrawOp::Glyphs` runs — vello's GPU glyph pipeline   |
| text (Type1 / no sfnt) | glyph outline → `Fill` (same pixels, no atlas)      |
| Type3 glyphs           | re-interpreted into path ops                        |
| images                 | `Image` blit                                        |
| clips / groups         | `Save`+`Clip`…`Restore` / `Layer`                   |

Text from **embedded** TrueType/OpenType/CID fonts (the common case) becomes
`DrawOp::Glyphs` runs that hit vello's cached-outline GPU glyph path. Standard-14
fonts (resolved from the bundled fallbacks) and Type1 fonts fall back to
outlining each glyph into a `Fill` — identical output, no glyph atlas.

The glyph-space invariant: an outline is normalized to **upem = 1000**, and every
renderer drives its glyph machinery at that nominal em (vello `font_size = 1000`,
skrifa `Size::new(1000.0)`), so the GPU and CPU paths converge to the same pixels
(CLAUDE.md §7).

## Fidelity

Rendered faithfully: vector fills/strokes, text (embedded + standard fonts),
images, clipping, transforms, and:

- **Shadings / gradients** — axial, radial, function-based, and mesh shading
  patterns are sampled (via `hayro`'s encoder) into a texture clipped to the fill.
- **All 16 blend modes** — the full PDF separable + non-separable set (Multiply,
  Screen, Overlay, Darken, ColorDodge, Difference, Hue, Luminosity, …) map 1:1
  onto vello/CoreGraphics/Canvas2D.
- **Soft masks / transparency groups** — `/Luminosity` soft masks (and masked
  groups) render exactly via vello's luminance-mask layer (a `DrawOp::MaskGroup`).
- **Dashed strokes** — dash arrays + phase flow through to every renderer.
- **Color** — ICC, Separation/spot, and CMYK are handled by `hayro`'s color
  management.

Remaining gaps (tracked in [`Warnings`], never silently dropped):

- **Tiling patterns** (`pattern_paints`) draw as nothing — only *shading*
  patterns are modeled; tiling cells aren't yet.
- **`/Alpha` soft masks** (`soft_masks`) render via the luminance path (vello has
  no alpha-mask primitive in 0.9), so they're approximate. `/Luminosity` is exact.
- On the **CPU** fallback (`canvas-native`, sim/emulator only) soft masks draw
  their content unmasked — the GPU path masks correctly.
- Encrypted PDFs are unsupported (a `hayro` limitation).

## Loading a file at runtime

`Pdf` interprets its bytes once, when built. For a document chosen at runtime
(e.g. a file picker), use `PdfReactive`, which reads a closure inside the canvas
`draw` scope and re-interprets only when the document identity (`Rc`) changes —
so updating the signal it reads swaps the page with no remount:

```rust
use std::rc::Rc;
let doc: Signal<Option<Rc<Vec<u8>>>> = signal!(None);
let viewer = pdf::PdfReactive(move || doc.get(), /*page*/ 0, /*w*/ 520.0, /*h*/ 680.0);
// from a file picker: doc.set(Some(Rc::new(file_bytes)));
```

The page is fit (aspect-preserved, centered) into the fixed `width × height` box.
[`examples/pdf-demo`](../../../examples/pdf-demo/) wires this to the
[`file-picker`](../file-picker/) SDK — **Open PDF…** loads a file from disk and
renders it on the GPU. Run it with **`idealyst dev --macos --local`**: the canvas
carries a `draw` closure in its external payload that can't cross the dev-server
wire, so canvas-based SDKs need single-process local-render mode (else the client
shows "Component not available: canvas_core::CanvasProps").

## Lower-level API

`Document::load(bytes)` → `Document::render_page(i)` returns a `RenderedPage`
{ `scene`, `width`, `height`, `warnings` } if you want the `canvas_core::Scene`
directly (to scale/compose it yourself) rather than a ready-made element.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet. PDF renders
through the `canvas` primitive, so behavior tracks the active canvas renderer:
`vello` (GPU) where available, `canvas-native` (CPU) otherwise. Run the
`pdf-demo` example with **`idealyst dev --macos --local`** (the canvas `draw`
closure can't cross the dev-server wire — single-process local-render only).
Tick each item as you exercise it.

**Rendering / behavior**

A multi-page PDF should render with correct text (embedded sfnt → GPU glyph runs;
standard-14/Type1 → outlined fills), vector fills/strokes, images, clips,
transforms, shadings/gradients, blend modes, and soft masks; the page fits
(aspect-preserved, centered) into the fixed `width × height` box; `PdfReactive`
swaps the document when its signal changes with no remount.

- [ ] **macOS** — verified on Metal (GPU/vello): open a real multi-page PDF via the
  `pdf-demo` + file-picker, confirm text/vectors/images render and pages swap.
- [ ] **iOS** — ⚠️ not yet device-confirmed (compile-checked). Real devices run vello
  on Metal; the **simulator** falls back to `canvas-native` (CPU) where soft masks
  draw content unmasked.
- [ ] **Android** — ⚠️ not yet device-confirmed (compile-checked). Vello on Vulkan on
  real devices; emulator uses the CPU fallback.
- [ ] **Web** — ⚠️ not yet confirmed (compile-checked). Renders via the WebGPU canvas
  renderer where available, Canvas2D CPU fallback otherwise.
- [ ] **Approximations** — confirm the documented gaps surface in `Warnings` and never
  panic: tiling patterns draw as nothing, `/Alpha` soft masks are approximate,
  encrypted PDFs are unsupported.
