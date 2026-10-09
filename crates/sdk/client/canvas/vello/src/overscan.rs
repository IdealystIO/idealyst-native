//! Overscan for cached layers, shared by the native and web renderers.

/// Overscan margin as a fraction of the viewport, per side, for cached layers —
/// read once from `OVERSCAN_FRAC` (default `0.0` = no overscan, the original
/// viewport-sized behavior). When `> 0`, cached layers bake into a texture that
/// extends `frac`·viewport beyond each edge so a pan up to that margin composites
/// (O(1)) with no black edge. Must exceed the app's `far` recenter threshold
/// (0.4) so the re-bake fires before the margin is exhausted. Clamped to [0, 1].
pub(crate) fn overscan_frac() -> f32 {
    thread_local! {
        static FRAC: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
    }
    FRAC.with(|c| match c.get() {
        Some(v) => v,
        None => {
            let v = std::env::var("OVERSCAN_FRAC")
                .ok()
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            c.set(Some(v));
            v
        }
    })
}

/// The overscanned device dimensions for a `(w, h)` viewport: `(1 + 2·frac)` on
/// each axis (rounded). `frac == 0` returns `(w, h)` unchanged.
pub(crate) fn overscan_dims(w: u32, h: u32, frac: f32) -> (u32, u32) {
    if frac <= 0.0 {
        return (w, h);
    }
    let scale = 1.0 + 2.0 * frac;
    (((w as f32) * scale).round() as u32, ((h as f32) * scale).round() as u32)
}
