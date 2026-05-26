# backend-cpu

A software-rasterizing `Backend` for `runtime_core`. Renders the
framework's primitive tree into a pixel framebuffer using a pure-Rust
rasterizer — no GPU, no native UI toolkit, no operating-system
window dependency.

The renderer outputs through the [`Surface`] trait, so the same
backend drives:

- An in-memory `Vec<u8>` framebuffer (default, for tests + the
  desktop preview).
- A desktop preview window (winit + softbuffer), gated behind the
  `preview` feature.
- An ESP32 SPI display (ST7789 / ILI9341 / etc.) via a `Surface`
  impl wrapping a `mipidsi` or `embedded-graphics` display driver.
- Any other "I have a framebuffer-shaped thing" target you can
  fit into the `Surface` trait.

## Status

MVP. View / Text / Button / Pressable / ScrollView render correctly
with Flexbox layout, axis-aligned rounded rectangles, per-side
borders, alpha blending, opacity inheritance, and the built-in 8×8
bitmap font. Click hit-testing routes through the same coordinate
system the renderer paints with.

## What's supported

| Primitive            | Status                | Notes                                  |
|----------------------|-----------------------|----------------------------------------|
| View                 | Full                  | Backgrounds, borders, opacity, rounded corners |
| Text                 | Full                  | 8×8 bitmap font, multi-line wrap       |
| Button               | Full                  | Label + on_click; same chrome as iOS   |
| Pressable            | Full                  | Hit-testing, no built-in chrome        |
| ScrollView           | Functional            | Offset + clip; no momentum, no scrollbars |
| Image                | Placeholder text      | No image decode pipeline on MCUs       |
| Icon                 | Placeholder text      | No vector path rasterization yet       |
| TextInput            | Placeholder text      | No input infra on most boards          |
| TextArea             | Placeholder text      | Same constraint as TextInput           |
| Toggle               | Placeholder text      | No bool-control affordance             |
| Slider               | Placeholder text      | No drag affordance                     |
| ActivityIndicator    | Placeholder text      | No tick-driven animation yet           |
| Virtualizer          | Placeholder text      | No cell pool / windowing               |
| Graphics             | Placeholder text      | No GPU                                 |
| Portal               | Placeholder text      | No overlay layer                       |
| External             | Placeholder text      | No SDK overlays                        |
| Navigator            | Placeholder text      | No screen-management infra             |

"Placeholder text" means the primitive renders the literal text
`"<PrimitiveName> not supported on CPU backend"` through the
existing 8×8 font path. Author code never panics, and the missing
support is **visible** on the device. This is deliberate per the
project's `feedback_cpu_unsupported_placeholders` posture — silent
no-ops mask the gap, visible placeholders surface it.

Deferred but in scope for later: Image decode (PNG behind a feature
flag), Icon path rasterization (extends the gradient-fill pipeline),
ActivityIndicator animation (per-frame dirty-rect raster).

Out of scope on this backend: TextInput / TextArea, Toggle, Slider,
Virtualizer, Graphics, External, Portal, Navigator. These don't fit
the MCU constraint and shouldn't be silently fabricated on the CPU
path.

## Desktop preview

```bash
# default 320×240 panel, 2× window pixel scale
cargo run -p backend-cpu --example preview --features preview

# custom resolution + 3× scale (320×240 framebuffer in a 960×720 window)
cargo run -p backend-cpu --example preview --features preview -- \
    --width 320 --height 240 --scale 3

# portrait panel
cargo run -p backend-cpu --example preview --features preview -- \
    --width 240 --height 320
```

`--width` / `--height` set the *framebuffer* size — what the
backend rasterizes into and what your ESP32 panel would receive.
`--scale` is an upscale-on-display factor so a 320×240 framebuffer
doesn't render as a postage stamp on a 4K monitor.

Click the bright blue button to bump the click counter; it shows up
in the title bar.

## ESP32 / embedded targets

The `Surface` trait is the entire integration surface for hardware.
Implement it once for your display driver and the rest of the
backend works unchanged.

### Recommended emulator: [Wokwi](https://wokwi.com/)

Wokwi is a free, web-based ESP32 simulator that includes virtual
ST7789 / ILI9341 displays. The fastest path:

1. Install the **Wokwi for VS Code** extension.
2. Use the [esp-rs/esp-idf-template](https://github.com/esp-rs/esp-idf-template)
   project template (`cargo generate esp-rs/esp-idf-template`) —
   pick the **std** variant so `HashMap`/`Vec` keep working without
   churn.
3. Add `backend-cpu = { path = "..." , default-features = false }`
   to your firmware's `Cargo.toml`. The `preview` feature is *off*
   by default, so winit/softbuffer stay out of the embedded dep
   tree.
4. Implement [`Surface`] for an `mipidsi` display (see sketch below).
5. Hit *Wokwi: Start Simulator* in VS Code. The simulator boots
   your firmware against a virtual ESP32-DevKitC + ST7789 and shows
   the framebuffer live.

### Why std-on-FreeRTOS (esp-idf) instead of no_std (esp-hal)?

`backend-cpu` currently uses `HashMap` and `Vec` from `alloc` /
`std`. On `esp-idf` those work out of the box (it ships a global
allocator and a `std`-compatible runtime). Porting to bare-metal
`esp-hal` (no_std) is doable — swap `HashMap` for `heapless::IndexMap`
or `slab` — but it's a follow-up, not a blocker. Start with esp-idf
to validate the render pipeline on hardware, then optimize.

### `Surface` impl sketch for a `mipidsi` ST7789

```rust,ignore
use backend_cpu::Surface;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use mipidsi::Display;

/// Wraps an `mipidsi` display and quantizes RGBA8 → RGB565 inside
/// `put_pixel`. The CPU backend stays in RGBA8 internally; the
/// quantization cost lives here, in surface code.
pub struct St7789Surface<DI, MODEL, RST> {
    display: Display<DI, MODEL, RST>,
    width: u32,
    height: u32,
}

impl<DI, MODEL, RST> Surface for St7789Surface<DI, MODEL, RST>
where
    Display<DI, MODEL, RST>: DrawTarget<Color = Rgb565>,
{
    fn width(&self) -> u32 { self.width }
    fn height(&self) -> u32 { self.height }

    fn put_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        // RGB888 → RGB565: 5 bits red, 6 bits green, 5 bits blue.
        // Drops alpha — we receive pre-blended opaque pixels from
        // the backend, so the alpha channel is informational only.
        let r = (rgba[0] >> 3) as u16;
        let g = (rgba[1] >> 2) as u16;
        let b = (rgba[2] >> 3) as u16;
        let color = Rgb565::new(r as u8, g as u8, b as u8);
        let _ = self.display.draw_iter(core::iter::once(
            Pixel(Point::new(x as i32, y as i32), color),
        ));
    }

    fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, rgba: [u8; 4]) {
        // ST7789 has a hardware fill primitive — drive it through
        // mipidsi's `fill_solid` instead of per-pixel writes. On a
        // 40 MHz SPI bus this is the difference between 200 fps and
        // 8 fps for a viewport-sized fill.
        let r = (rgba[0] >> 3) as u16;
        let g = (rgba[1] >> 2) as u16;
        let b = (rgba[2] >> 3) as u16;
        let color = Rgb565::new(r as u8, g as u8, b as u8);
        let rect = embedded_graphics::primitives::Rectangle::new(
            Point::new(x, y),
            Size::new(w, h),
        );
        let _ = self.display.fill_solid(&rect, color);
    }
}
```

### Memory budget on the ESP32

A 320×240 RGB888 back-buffer is 230 KB, which doesn't fit on a
plain ESP32 (520 KB total SRAM, much of which is reserved for the
Wi-Fi stack). Options, in order of preference:

1. **Render directly to the display** — `put_pixel` writes straight
   to SPI without a back-buffer. The Surface trait supports this;
   you give up `fill_rect`'s SPI batching benefit but use ~0 RAM.
2. **Line-buffer rendering** — `Surface` holds a single-row buffer
   in RGB565 (640 bytes for 320 px wide), `put_pixel` accumulates
   into it, and `present` flushes the row when Y changes. Reorder
   the paint walker to emit pixels in raster order to keep this
   cheap. Not implemented yet — would land alongside damage-rect
   tracking.
3. **Use PSRAM-equipped boards** — ESP32-WROVER, ESP32-S3, etc.
   have 4–8 MB PSRAM, which makes a full RGB565 back-buffer
   (153 KB at 320×240) trivial. `MemSurface` would work unchanged
   if you remap its allocator to PSRAM.

## Headless tests

```bash
cargo test -p backend-cpu
```

18 tests cover the rasterizer arithmetic (alpha blend rounding,
rounded-rect inclusion), the font table, and end-to-end rendering
through the `Backend` trait. Tests assert on pixel values from a
`MemSurface`, so they catch any regression that changes what gets
painted.
