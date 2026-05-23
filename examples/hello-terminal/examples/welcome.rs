//! Run the `welcome` example through the ASCII / terminal backend.
//!
//! `welcome::app()` is the canonical Idealyst intro — three-act
//! cinematic with springs, tweens, a radial-gradient sun, vignette,
//! orbiting planets, and animated text. Most of that machinery
//! degrades on the terminal:
//!
//! - Layout: ✓ Taffy runs unchanged
//! - Spring / tween timing: ✓
//! - Opacity tweens: ~ modulates cell alpha, but terminals don't
//!   actually composite, so it reads as on/off more than fading
//! - Translate animations: ~ floored to cell units (1 px = 1 cell)
//! - Color tweens on bg/fg: ✓
//! - Radial / linear gradients: ✗ no implementation — renders as the
//!   fallback solid background or nothing
//! - Scale / Rotate / ZIndex: ✗ documented no-op on this backend
//! - Stroke draw-in on text: ✗ no-op
//!
//! So you'll see the text appear and animate, but the visual
//! fireworks (sun glare, vignette) won't.

fn main() {
    let opts = host_terminal::RunOptions {
        target_fps: 30,
        on_key: None,
        // Welcome's stylesheet uses mobile-px values (planets are
        // `width: px(14..22)`, translates are 50+ px, etc.). At the
        // default cell_size of (1.0, 1.0), a 14-px-wide planet
        // would occupy 14 cells and translate animations would
        // launch text fully off-screen. We tell the backend
        // "1 cell ≈ 8 layout px horizontally, 16 px vertically" —
        // matches typical glyph aspect ratios in terminals and
        // gives welcome's layout a reasonable viewport.
        cell_size: Some((8.0, 16.0)),
    };
    if let Err(e) = host_terminal::run(welcome::app, opts) {
        eprintln!("welcome (terminal) exited with error: {e}");
        std::process::exit(1);
    }
}
