//! ESP32-C3 firmware: drives the `backend-cpu` software rasterizer
//! against an ILI9341 240x320 SPI display.
//!
//! ## Hardware
//!
//! Wokwi diagram (default) wires the panel exactly like a typical
//! Adafruit ILI9341 breakout to an ESP32-C3 DevKit:
//!
//! | Display pin | ESP32-C3 GPIO | Role            |
//! |-------------|---------------|-----------------|
//! | VCC         | 3V3           | power           |
//! | GND         | GND           | ground          |
//! | CS          | GPIO10        | SPI chip select |
//! | RESET       | GPIO8         | reset (active low) |
//! | D/C         | GPIO3         | data/command select |
//! | SCK         | GPIO6         | SPI clock       |
//! | MOSI        | GPIO7         | SPI MOSI        |
//! | LED         | 3V3 (via 100Ω)| backlight       |
//!
//! These match the Wokwi `diagram.json` shipped alongside this
//! crate. Pin choices are constrained by the ESP32-C3's available
//! GPIOs (SPI2 is the only general-purpose SPI controller).
//!
//! ## Build
//!
//! See `README.md` in this directory. The short version:
//!
//! ```bash
//! cargo install espup ldproxy espflash
//! espup install
//! source ~/export-esp.sh
//! rustup target add riscv32imc-esp-espidf
//! cargo build --release      # or just `cargo build` for a debug bin
//! ```
//!
//! Then launch via the Wokwi for VS Code extension (uses
//! `wokwi.toml` in this dir) or flash to a real C3 with `espflash`.

use anyhow::{Context as _, Result};
use backend_cpu::{CpuBackend, Surface};
// Force a direct reference to `esp-idf-sys` so cargo wires its
// build-script-emitted `cargo:rustc-link-arg=` directives
// (the ESP-IDF link script, --ldproxy-linker, the wrapped IDF
// static libs) into our binary's link line. Without this `use`,
// cargo treats esp-idf-sys as a purely-transitive dep and the
// link args never propagate — ldproxy then panics with
// "Cannot locate argument '--ldproxy-linker <linker>'".
use esp_idf_sys as _;
use display_interface::WriteOnlyDataCommand;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_hal::digital::OutputPin;
use esp_idf_hal::delay::{Ets, FreeRtos};
use esp_idf_hal::gpio::{AnyIOPin, PinDriver};
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::prelude::*;
use esp_idf_hal::spi::{SpiConfig, SpiDeviceDriver, SpiDriver, SpiDriverConfig};
use esp_idf_svc::log::EspLogger;
use log::info;
use mipidsi::models::{ILI9341Rgb565, Model};
use mipidsi::options::{ColorOrder, Orientation, Rotation};
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{Backend, FlexDirection, Length, StyleRules, Tokenized};
use std::rc::Rc;

/// Panel dimensions for the ILI9341 in landscape orientation.
/// The chip is natively portrait (240x320); we rotate 90° so the
/// long edge is horizontal.
const PANEL_W: u32 = 320;
const PANEL_H: u32 = 240;

fn main() -> Result<()> {
    // Patches that `esp_idf_svc::sys::link_patches` brings in resolve
    // a handful of `static` symbols the IDF C SDK expects to find
    // at link time. Calling it from `main` is the standard ritual
    // for any Rust-on-ESP-IDF binary.
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();
    info!("esp32-ili9341 boot");

    // -----------------------------------------------------------------
    // Bring up the SPI bus + ILI9341 panel.
    // -----------------------------------------------------------------
    let peripherals = Peripherals::take().context("Peripherals::take")?;
    let pins = peripherals.pins;

    let sclk = pins.gpio6;
    let mosi = pins.gpio7;
    let cs = pins.gpio10;
    let dc = PinDriver::output(pins.gpio3).context("dc pin")?;
    let mut rst = PinDriver::output(pins.gpio8).context("rst pin")?;

    let spi_driver = SpiDriver::new(
        peripherals.spi2,
        sclk,
        mosi,
        Option::<AnyIOPin>::None, // MISO unused — display is write-only
        &SpiDriverConfig::new(),
    )
    .context("SpiDriver::new")?;

    // 40 MHz is the IDF SPI driver's ceiling without exotic clock
    // dividers; ILI9341 panels are happy at that speed.
    let spi_config = SpiConfig::new().baudrate(40.MHz().into());
    let spi_device = SpiDeviceDriver::new(spi_driver, Some(cs), &spi_config)
        .context("SpiDeviceDriver::new")?;

    let di = display_interface_spi::SPIInterface::new(spi_device, dc);

    let mut display = mipidsi::Builder::new(ILI9341Rgb565, di)
        .reset_pin(&mut rst)
        .display_size(240, 320)
        .orientation(Orientation::new().rotate(Rotation::Deg90))
        .color_order(ColorOrder::Bgr)
        .init(&mut Ets)
        .map_err(|e| anyhow::anyhow!("display init failed: {e:?}"))?;

    // Clear once so any garbage left in the display's GRAM is
    // overwritten before our first paint.
    display
        .clear(Rgb565::BLACK)
        .map_err(|e| anyhow::anyhow!("display clear: {e:?}"))?;
    info!("display ready: {PANEL_W}x{PANEL_H}");

    // -----------------------------------------------------------------
    // Wire up the CPU backend + a Surface over the panel.
    // -----------------------------------------------------------------
    let mut surface = DisplaySurface {
        display,
        width: PANEL_W,
        height: PANEL_H,
    };
    let mut backend = CpuBackend::new(PANEL_W, PANEL_H);
    backend.set_clear_color([15, 23, 42, 255]); // slate-900

    let title_node = build_scene(&mut backend);
    info!("scene built, entering render loop");

    // -----------------------------------------------------------------
    // Render loop.
    // -----------------------------------------------------------------
    // The framework has no "is dirty" signal at this layer yet, so we
    // render every loop iteration. Cap at ~30 fps to leave the CPU
    // headroom and to keep the SPI bus from saturating — every full
    // 320×240 RGB565 push is ~150 KB at 40 MHz = ~30 ms over the wire.
    let mut tick: u32 = 0;
    loop {
        // Animate the title text counter so the panel shows it's
        // alive even without input. Once we wire animation into the
        // backend (animated_f32), this loop will fall away in favor
        // of per-frame animation interpolation.
        let label = format!("CPU BACKEND - {tick:>4}");
        backend.update_text(&title_node, &label);
        backend.render(&mut surface);
        tick = tick.wrapping_add(1);

        FreeRtos::delay_ms(33);
    }
}

// =============================================================================
// Surface impl: bridge backend-cpu's RGBA8 pixel writes to the panel's RGB565.
// =============================================================================

/// Wraps a `mipidsi::Display` and quantizes RGBA8 → RGB565 inside
/// each pixel write. The CPU backend keeps RGBA8 internally; this
/// adapter is where the storage-format conversion lives, isolating
/// the embedded path from the rest of the rasterizer.
///
/// The `where` clause mirrors `mipidsi::Display`'s own bounds —
/// when a struct holds a generic instance of another generic
/// struct, rustc requires the same bounds at the outer struct
/// definition site or it can't prove the inner field is
/// well-formed.
struct DisplaySurface<DI, MODEL, RST>
where
    DI: WriteOnlyDataCommand,
    MODEL: Model<ColorFormat = Rgb565>,
    RST: OutputPin,
{
    display: mipidsi::Display<DI, MODEL, RST>,
    width: u32,
    height: u32,
}

impl<DI, MODEL, RST> Surface for DisplaySurface<DI, MODEL, RST>
where
    // Mirror the bounds `mipidsi::Display<DI, MODEL, RST>` itself
    // requires — without these the compiler can't prove our
    // `self.display.fill_solid(...)` call is well-typed. The
    // additional `DrawTarget<Color = Rgb565>` clause pins us to
    // 16-bpp framebuffers (ILI9341 / ST7789 / GC9A01 etc. — any
    // mipidsi model that exposes a `Rgb565` `DrawTarget` impl).
    DI: WriteOnlyDataCommand,
    MODEL: Model<ColorFormat = Rgb565>,
    RST: OutputPin,
    mipidsi::Display<DI, MODEL, RST>: DrawTarget<Color = Rgb565>,
{
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn put_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        // RGB888 → RGB565: 5 bits red, 6 bits green, 5 bits blue.
        // Alpha is dropped — the backend has already blended each
        // pixel to opaque before calling us, so the alpha channel
        // carries no information at this layer.
        let color = rgb565_from_rgba(rgba);
        // `draw_iter` returns a `Result` keyed on the bus's error
        // type; failures here are SPI-level (line down, DMA full)
        // and unrecoverable from inside `put_pixel`. Log via the
        // ignored result and move on — the next frame will retry.
        let _ = self.display.draw_iter(core::iter::once(Pixel(
            Point::new(x as i32, y as i32),
            color,
        )));
    }

    fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, rgba: [u8; 4]) {
        // Use the panel's hardware-accelerated solid fill instead
        // of per-pixel SPI writes. The ILI9341's `RAMWR` window-set
        // + run-length push is roughly 100× faster than per-pixel
        // `draw_iter` for full-viewport fills — without it, the
        // `clear` step at the start of every `render` swallows ~3
        // seconds of SPI time per frame.
        let color = rgb565_from_rgba(rgba);
        let rect = Rectangle::new(Point::new(x, y), Size::new(w, h));
        let _ = self.display.fill_solid(&rect, color);
    }

    fn present(&mut self) {
        // ILI9341 is not double-buffered at the controller level —
        // each pixel write is visible immediately. There's nothing
        // to flush. (A future display driver with internal page-
        // flipping would override this.)
    }
}

#[inline]
fn rgb565_from_rgba(rgba: [u8; 4]) -> Rgb565 {
    let r = rgba[0] >> 3;
    let g = rgba[1] >> 2;
    let b = rgba[2] >> 3;
    Rgb565::new(r, g, b)
}

// =============================================================================
// Demo scene
// =============================================================================

/// Build the demo scene into `backend`. Returns the title-text node
/// so the render loop can mutate its content frame-by-frame without
/// rebuilding the tree.
fn build_scene(backend: &mut CpuBackend) -> backend_cpu::CpuNode {
    let a11y = AccessibilityProps::default();
    let (vw, vh) = backend.viewport();

    let mut root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style(|s| {
            s.background = Some(lit_color("rgb(15, 23, 42)"));
            s.width = Some(px(vw as f32));
            s.height = Some(px(vh as f32));
            s.padding_top = Some(px(12.0));
            s.padding_right = Some(px(12.0));
            s.padding_bottom = Some(px(12.0));
            s.padding_left = Some(px(12.0));
            s.gap = Some(px(10.0));
        }),
    );

    let title = backend.create_text("CPU BACKEND - 0", &a11y);
    backend.apply_style(
        &title,
        &style(|s| {
            s.color = Some(lit_color("rgb(226, 232, 240)"));
            s.background = Some(lit_color("rgb(30, 41, 59)"));
            s.padding_top = Some(px(6.0));
            s.padding_right = Some(px(10.0));
            s.padding_bottom = Some(px(6.0));
            s.padding_left = Some(px(10.0));
            s.border_top_left_radius = Some(px(4.0));
            s.border_top_right_radius = Some(px(4.0));
            s.border_bottom_left_radius = Some(px(4.0));
            s.border_bottom_right_radius = Some(px(4.0));
        }),
    );
    backend.insert(&mut root, title);

    // Swatch row — same demo as the desktop preview.
    let mut swatch_row = backend.create_view(&a11y);
    backend.apply_style(
        &swatch_row,
        &style(|s| {
            s.flex_direction = Some(FlexDirection::Row);
            s.gap = Some(px(8.0));
            s.height = Some(px(40.0));
        }),
    );
    for color in ["rgb(239, 68, 68)", "rgb(34, 197, 94)", "rgb(59, 130, 246)"] {
        let swatch = backend.create_view(&a11y);
        backend.apply_style(
            &swatch,
            &style(|s| {
                s.background = Some(lit_color(color));
                s.flex_grow = Some(Tokenized::Literal(1.0));
                s.border_top_left_radius = Some(px(6.0));
                s.border_top_right_radius = Some(px(6.0));
                s.border_bottom_left_radius = Some(px(6.0));
                s.border_bottom_right_radius = Some(px(6.0));
            }),
        );
        backend.insert(&mut swatch_row, swatch);
    }
    backend.insert(&mut root, swatch_row);

    let body = backend.create_text("rendered on esp32-c3 via spi.", &a11y);
    backend.apply_style(
        &body,
        &style(|s| {
            s.color = Some(lit_color("rgb(148, 163, 184)"));
        }),
    );
    backend.insert(&mut root, body);

    let overlay = backend.create_view(&a11y);
    backend.apply_style(
        &overlay,
        &style(|s| {
            s.background = Some(lit_color("rgba(250, 204, 21, 0.5)"));
            s.width = Some(px(120.0));
            s.height = Some(px(20.0));
            s.border_top_left_radius = Some(px(3.0));
            s.border_top_right_radius = Some(px(3.0));
            s.border_bottom_left_radius = Some(px(3.0));
            s.border_bottom_right_radius = Some(px(3.0));
        }),
    );
    backend.insert(&mut root, overlay);

    backend.finish(root);
    title
}

// ---------------------------------------------------------------------------
// Style helpers — same shape as the desktop preview's helpers.
// ---------------------------------------------------------------------------

fn style(mut f: impl FnMut(&mut StyleRules)) -> Rc<StyleRules> {
    let mut s = StyleRules::default();
    f(&mut s);
    Rc::new(s)
}

fn lit_color(s: &str) -> Tokenized<runtime_core::Color> {
    Tokenized::Literal(s.into())
}

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}
