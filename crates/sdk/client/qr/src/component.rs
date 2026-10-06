//! The `QrCode` component: a [`QrMatrix`] drawn as vector rectangles into a
//! `canvas`. The canvas renders identically on every renderer (web Canvas2D,
//! CoreGraphics, `android.graphics`, Cairo, vello on the GPU), so the code
//! looks — and scans — the same everywhere.

use std::rc::Rc;

use canvas::{Canvas, CanvasProps, Color, Path, Scene};
use runtime_core::{component, memo, Element, IntoElement, Length, StyleRules, StyleSheet};

use crate::generate::{ErrorCorrection, QrMatrix, DEFAULT_QUIET_ZONE};

/// A QR code for `data`, drawn as a crisp vector square.
///
/// ```ignore
/// ui! {
///     QrCode(data = "https://example.com".to_string())
///     QrCode(data = invite_url, size = Some(160.0), error_correction = ErrorCorrection::High)
/// }
/// ```
///
/// **Size.** By default the code is a square as wide as its container
/// (`width: 100%`, `aspect-ratio: 1`); pass `size` for a fixed side in logical
/// px. The modules (quiet zone included) are fitted into the largest centered
/// square, snapped to whole logical pixels when they're at least one pixel
/// wide, so module edges land on pixel boundaries.
///
/// **Reactive.** `data`, `error_correction`, `quiet_zone` and the colors may
/// be signals; a change re-encodes (only when the data or error correction
/// changed) and redraws.
///
/// **Colors.** Defaults are black on white. Keep strong contrast and a light
/// background: most scanners can't read light-on-dark codes.
///
/// Data that doesn't fit in a QR code (see [`GenerateError::DataTooLong`](crate::GenerateError))
/// draws just the background — check with [`QrMatrix::encode`] first if the
/// data's size isn't under your control.
#[component]
pub fn QrCode(
    #[doc = " The text (or URL) to encode."] data: String,
    #[doc = " How much damage the code survives; higher is denser."]
    #[prop(default = ErrorCorrection::Medium)]
    error_correction: ErrorCorrection,
    #[doc = " Light border around the code, in modules (the specification's minimum is 4)."]
    #[prop(default = DEFAULT_QUIET_ZONE)]
    quiet_zone: usize,
    #[doc = " Color of the dark modules."]
    #[prop(default = Color::new(0, 0, 0, 255))]
    dark: Color,
    #[doc = " Background color, filling the whole square including the quiet zone."]
    #[prop(default = Color::new(255, 255, 255, 255))]
    light: Color,
    #[doc = " Fixed side length in logical px; `None` fills the container's width as a square."]
    #[prop(static, default = None)]
    size: Option<f32>,
) -> Element {
    // Re-encode only when the content changes, not on every paint (a resize or
    // a color change redraws from the same matrix).
    let matrix = memo(move || QrMatrix::encode(data.get(), error_correction.get()).ok());

    let draw = canvas::draw(move |s: &mut Scene| {
        let (w, h) = s.size();
        let side = w.min(h);
        if side <= 0.0 {
            return;
        }
        let (ox, oy) = ((w - side) / 2.0, (h - side) / 2.0);
        s.fill_path(Path::rect(ox, oy, side, side), light.get());
        let quiet = quiet_zone.get();
        let dark = dark.get();
        matrix.with(|m| {
            if let Some(m) = m {
                m.draw(s, (ox, oy, side), quiet, dark);
            }
        });
    });

    let mut rules = StyleRules::default();
    match size {
        Some(px) => {
            rules.width = Some(Length::Px(px).into());
            rules.height = Some(Length::Px(px).into());
        }
        None => {
            rules.width = Some(Length::pct(100.0).into());
            rules.aspect_ratio = Some(1.0);
        }
    }
    // Built with canvas's documented builder form rather than `ui!`: `Canvas`
    // returns a `CanvasBound` that takes its style through `.with_style`
    // (canvas-core `prim.rs`), and `.into_element()` is the real conversion
    // here, not the no-op one the macros already do.
    Canvas(CanvasProps { draw, ..Default::default() })
        .with_style(Rc::new(StyleSheet::r#static(rules)))
        .into_element()
}

impl QrMatrix {
    /// Draw this code into a canvas scene: the dark modules, with
    /// `quiet_zone` modules of margin, fitted into the square
    /// `(x, y, side)`. The background is not painted — fill the square first
    /// if the canvas behind it isn't already light. This is what the
    /// [`QrCode`] component draws; call it directly to put a code into a
    /// canvas of your own (a printable card, a composited overlay).
    ///
    /// The modules are filled as ONE path of horizontal runs — a single fill,
    /// so abutting rows don't show anti-aliasing seams — and the module size
    /// is snapped to whole logical pixels when it is at least one pixel, with
    /// the leftover split evenly around the code.
    pub fn draw(&self, scene: &mut Scene, square: (f32, f32, f32), quiet_zone: usize, dark: Color) {
        let (x0, y0, side) = square;
        let n = (self.width() + 2 * quiet_zone) as f32;
        let raw = side / n;
        let module = if raw >= 1.0 { raw.floor() } else { raw };
        let inset = (side - module * n) / 2.0;
        let margin = quiet_zone as f32 * module;
        let (ox, oy) = (x0 + inset + margin, y0 + inset + margin);
        scene.path();
        for (x, y, len) in self.dark_runs() {
            scene.add_path(Path::rect(
                ox + x as f32 * module,
                oy + y as f32 * module,
                len as f32 * module,
                module,
            ));
        }
        scene.fill(dark);
    }
}
