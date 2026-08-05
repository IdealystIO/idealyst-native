//! Container queries — responsive overlays keyed on the resolved
//! inline-size of the **nearest ancestor containment context**, not the
//! global viewport.
//!
//! Where [`crate::breakpoint`] answers "how wide is the window?", a
//! container query answers "how wide is the box I was placed in?". A
//! card in a narrow sidebar and the same card in a wide main column can
//! lay themselves out differently *with the same stylesheet*, because
//! each keys off its own container rather than the shared viewport.
//!
//! # Authoring
//!
//! Mark a node as a containment context with the `.container()` builder
//! modifier, then key overlays off it in `stylesheet!`:
//!
//! ```ignore
//! stylesheet! {
//!     pub Card<Theme> {
//!         base(theme)                     { flex_direction: Column, padding: 12 }
//!         container (min_width: 400px)(theme) { flex_direction: Row, padding: 20 }
//!     }
//! }
//!
//! ui! {
//!     view(style = sidebar).container() {   // establishes the context
//!         Card()                            // Card's overlay keys off sidebar width
//!     }
//! }
//! ```
//!
//! Thresholds are arbitrary lengths (px), not named buckets — a 360px
//! sidebar can have its own `(min_width: 300px)` switch that the
//! viewport breakpoints (sm = 640) could never express. A semantic
//! named-bucket layer can be built on top of this primitive later.
//!
//! v1 supports `min_width` only (mobile-first cascade): every overlay
//! whose threshold is `<=` the container's inline-size applies, lowest
//! threshold first, so a wider threshold wins on conflicting properties
//! — the same ordering the viewport breakpoints and stacked
//! `@media (min-width)` rules use. `max_width` / range queries are a
//! planned extension.
//!
//! # The inline-size containment invariant
//!
//! Container queries are a *cycle*: a child's style depends on the
//! container's size, but a child's style can change the container's size
//! (taller content → taller container). Left unguarded this oscillates.
//!
//! CSS breaks the cycle with `container-type: inline-size`: the
//! container's **inline (width)** axis must be determinable *independent
//! of its contents*. Children may query only that axis, and their
//! restyling only affects the block (height) axis — so the queried value
//! is stable after one layout, and the system reaches a fixed point in
//! two layout passes.
//!
//! This framework adopts the same rule. A `.container()` node's width
//! must come from its parent (an explicit width, a percentage, or a flex
//! track) — **never** shrink-to-fit from the descendants that query it.
//! Querying inline-size only, and feeding the container's resolved width
//! into a change-guarded signal (so a restyle that doesn't change the
//! width produces no further work), is what guarantees convergence on
//! the native backends. On web the browser enforces it for us.
//!
//! # Where it works
//!
//! - **Web / SSR**: `container-type: inline-size` + `@container` rules.
//!   Full support, including the SSR first paint (no JS round trip).
//! - **Native local-render (iOS / Android / macOS)**: the `.container()`
//!   view's resolved inline-size is wired into a signal that descendant
//!   style effects subscribe to; a width change re-applies them. The
//!   backends invoke their `on_layout` subscribers from the frame-apply
//!   pass (the native analog of a `ResizeObserver`).
//! - **Runtime-server wire / AAS**: not yet. The wire is a command
//!   protocol (`CreateView` / `ApplyStyle` / …), not an `Element`
//!   serialization, so a per-node capability needs its own command —
//!   exactly as `safe_area` does (`node_safe_area` in `scene_model`). A
//!   `MarkContainer` command plus the container overlays on the styled-
//!   variants command would extend it; until then `.container()` is inert
//!   over the wire. Local-render mode (`--local`) exercises the full
//!   native path today.

/// Reserved variant-axis prefix for a `min_width` container-query
/// overlay. The threshold is appended as the lossless 8-char hex of the
/// `f32`'s bit pattern (see [`container_axis_name`]), so two distinct
/// thresholds get distinct axes and the px value round-trips exactly.
/// The `__cq_` namespace keeps these out of the author variant
/// namespace, exactly like `__bp_` does for breakpoints and `__state_`
/// for interaction states.
pub const CONTAINER_MIN_WIDTH_PREFIX: &str = "__cq_minw_";

/// The reserved variant-axis name for a `min_width` container overlay at
/// `threshold` px. Encodes the threshold as the 8-char hex of its
/// `f32` bit pattern so the value is recovered losslessly by
/// [`container_axis_threshold`] and identical thresholds map to the same
/// axis (idempotent registration), while distinct thresholds never
/// collide.
///
/// The `stylesheet!` macro emits the *same* encoding at compile time, so
/// a `container (min_width: 400px)` block becomes
/// `.variant("__cq_minw_43c80000", "on", …)`.
pub fn container_axis_name(threshold: f32) -> String {
    let mut out = String::with_capacity(CONTAINER_MIN_WIDTH_PREFIX.len() + 8);
    out.push_str(CONTAINER_MIN_WIDTH_PREFIX);
    push_u32_hex(&mut out, threshold.to_bits());
    out
}

/// Inverse of [`container_axis_name`]: map a `__cq_minw_*` axis name back
/// to its `min_width` threshold in px, or `None` if `axis` isn't a
/// container-query overlay. The style system uses this to recognize
/// which declared variant axes are container overlays (parallel to
/// [`crate::Breakpoint::from_axis_name`]).
pub fn container_axis_threshold(axis: &str) -> Option<f32> {
    let hex = axis.strip_prefix(CONTAINER_MIN_WIDTH_PREFIX)?;
    if hex.len() != 8 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().map(f32::from_bits)
}

/// Writes the 8-char lowercase hex of `n` to `out`. Mirrors the helper
/// in `style.rs` (kept local so this module has no cross-module dep for
/// a three-line routine).
fn push_u32_hex(out: &mut String, n: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for shift in (0..8).rev() {
        let nibble = ((n >> (shift * 4)) & 0xf) as usize;
        out.push(HEX[nibble] as char);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_name_round_trips_threshold() {
        for px in [0.0_f32, 1.0, 320.0, 400.0, 400.5, 768.0, 1024.75, 9999.0] {
            let axis = container_axis_name(px);
            assert!(axis.starts_with(CONTAINER_MIN_WIDTH_PREFIX));
            assert_eq!(
                container_axis_threshold(&axis),
                Some(px),
                "threshold {px} must round-trip losslessly through {axis}"
            );
        }
    }

    #[test]
    fn distinct_thresholds_get_distinct_axes() {
        assert_ne!(container_axis_name(400.0), container_axis_name(401.0));
        // Identical thresholds map to the identical axis (idempotent
        // registration relies on this).
        assert_eq!(container_axis_name(400.0), container_axis_name(400.0));
    }

    #[test]
    fn non_container_axes_are_rejected() {
        assert_eq!(container_axis_threshold("__bp_md"), None);
        assert_eq!(container_axis_threshold("__state_hovered"), None);
        assert_eq!(container_axis_threshold("size"), None);
        // Right prefix, wrong length payload.
        assert_eq!(container_axis_threshold("__cq_minw_abc"), None);
    }
}
