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
    };
    if let Err(e) = host_terminal::run(welcome::app, opts) {
        eprintln!("welcome (terminal) exited with error: {e}");
        std::process::exit(1);
    }
}
