//! Pure style-decision helpers, factored out of the UIKit-only
//! `style.rs` / the mobile crate's `apply_style` so they build and
//! unit-test on the host target (the rest of those paths is UIKit-only
//! and compiles to nothing off-device).
//!
//! Two decisions live here, both regression-prone enough to deserve
//! host tests:
//!
//!   1. [`resolve_corner_radius`] — what `apply_style` should do with a
//!      requested corner radius given the px clamp it can (or can't)
//!      compute and the view's *current* laid-out bounds. The bug this
//!      guards: a reactive re-style on an already-laid-out percent-sized
//!      view used to blindly reset `cornerRadius` to 0 and defer the
//!      real value to the next layout pass — but a paint-only re-style
//!      produces no frame change, so `apply_frames`' frame-key cache
//!      skips the view and `sync_corner_radius` never re-fires. The
//!      rounded corner went square after any press. See
//!      `regression_ios_corner_radius_survives_restyle`.
//!
//!   2. [`is_layout_affecting`] — whether a style change touches any
//!      property that can change a node's size or position, and thus
//!      needs a Taffy `set_style` + a layout pass. A paint-only delta
//!      (background / opacity / color / shadow / corner radius) must
//!      NOT schedule a layout pass — doing so on every reactive re-style
//!      is the "layout runs on every press" churn. See
//!      `regression_ios_paint_only_skips_layout`.

use runtime_core::{Color, Length, StyleRules, Tokenized};

// The unstyled-`text()` color decision is shared by every native backend
// and now lives once in `runtime_core::text_defaults` (CLAUDE.md §7 — the
// resolved bytes must be byte-identical across iOS/Android/macOS/web).
// Re-exported here under the historical names so this crate's call sites
// (and `backend-ios-mobile`'s `apply_style`) are unchanged.
pub use runtime_core::text_defaults::{
    effective_text_color, THEME_TEXT_COLOR_FALLBACK, THEME_TEXT_COLOR_TOKEN,
};

/// The effective BACKGROUND for an editable text control (UITextField /
/// UITextView) — explicit author background wins, else the theme's
/// `color-surface` token instead of UIKit's dark-in-dark-mode
/// `systemBackground`. The canonical decision lives in
/// `backend_apple_core::text_control_style` so iOS + macOS share one source of
/// truth (host-tested there); this is a thin re-export for the iOS call sites.
/// See that module for the idea-ui `Textarea`-renders-black rationale.
pub fn effective_input_background(explicit: Option<&Tokenized<Color>>) -> Tokenized<Color> {
    backend_apple_core::text_control_style::effective_input_background(explicit)
}

/// The effective TEXT COLOR for an editable text control — explicit author
/// color wins, else the theme's `color-text` token (never the OS system label
/// color). Delegates to the shared
/// `backend_apple_core::text_control_style` decision; its fallback token is
/// identical to [`effective_text_color`]'s by design (§7).
pub fn effective_input_text_color(explicit: Option<&Tokenized<Color>>) -> Tokenized<Color> {
    backend_apple_core::text_control_style::effective_input_text_color(explicit)
}

/// What to do with a requested corner radius at `apply_style` time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CornerRadiusDecision {
    /// No rounding requested (or it resolved to zero). Leave the layer
    /// alone.
    None,
    /// Apply this clamped radius to the layer immediately. The clamp is
    /// known either from an explicit px width/height in the style, or
    /// from the view's already-known bounds.
    Apply(f64),
    /// We have a positive requested radius but no clamp value yet
    /// (percent-sized view, bounds still 0×0 pre-first-layout). Stash
    /// the requested radius for the layout pass's `sync_corner_radius`
    /// to clamp once bounds arrive, and set the live `cornerRadius` to
    /// 0 in the meantime so UIKit doesn't render the "999 on a tiny
    /// view → blank" state.
    Defer(f64),
}

/// Decide how to apply `requested` corner radius.
///
/// - `px_cap`: half the smaller explicit px width/height, when the
///   style supplies explicit pixel dimensions. `None` for percent /
///   auto sizing.
/// - `bounds_min_half`: half the smaller of the view's *current*
///   laid-out width/height (`min(w, h) / 2`), or `None`/`<= 0.0` when
///   the view hasn't been laid out yet (bounds 0×0).
///
/// Precedence: an explicit px cap wins (it's the author's declared
/// size and is stable across relayout). Otherwise, if the view already
/// has real bounds, clamp against them *now* — this is the fix that
/// lets a reactive re-style keep its corner instead of zeroing it and
/// waiting for a layout pass that the frame-key cache may skip. Only
/// when neither a px cap nor real bounds exist do we `Defer`.
///
/// UIKit invariant (project_ios_cornerradius_unclamped): a
/// `cornerRadius > min(w, h) / 2` makes the layer render *nothing* —
/// the clamp is mandatory, not cosmetic.
pub fn resolve_corner_radius(
    requested: f64,
    px_cap: Option<f64>,
    bounds_min_half: Option<f64>,
) -> CornerRadiusDecision {
    if requested <= 0.0 {
        return CornerRadiusDecision::None;
    }
    if let Some(cap) = px_cap {
        return CornerRadiusDecision::Apply(requested.min(cap.max(0.0)));
    }
    match bounds_min_half {
        // View is already laid out — clamp against live bounds now so
        // the radius survives a paint-only re-style (no frame change →
        // `apply_frames` skips this view → `sync_corner_radius` won't
        // re-fire). This is the visible-bug fix.
        Some(half) if half > 0.0 => CornerRadiusDecision::Apply(requested.min(half)),
        // Pre-layout: bounds 0×0. Stash + zero, let the layout pass
        // clamp once Taffy assigns a real frame.
        _ => CornerRadiusDecision::Defer(requested),
    }
}

/// Compute the half-the-smaller-px-dimension clamp from explicit
/// `width`/`height` style fields. Only `Px` lengths yield a clamp;
/// `Percent`/`Auto` resolve at layout time and have no useful value
/// here. Returns `None` when neither axis is an explicit px length.
pub fn px_cap_from_style(style: &StyleRules) -> Option<f64> {
    fn px_half(t: &Tokenized<Length>) -> Option<f64> {
        match t.resolve() {
            Length::Px(v) => Some(v as f64 / 2.0),
            _ => None,
        }
    }
    let half_w = style.width.as_ref().and_then(px_half);
    let half_h = style.height.as_ref().and_then(px_half);
    match (half_w, half_h) {
        (Some(w), Some(h)) => Some(w.min(h)),
        (Some(w), None) => Some(w),
        (None, Some(h)) => Some(h),
        (None, None) => None,
    }
}

/// The maximum requested corner radius (in px) across the four corners,
/// or `0.0` when none is set / all resolve to non-px. Mirrors the fold
/// `apply_style_to_view` does so both paths agree on the value.
pub fn requested_corner_radius_px(style: &StyleRules) -> f64 {
    [
        style.border_top_left_radius.as_ref(),
        style.border_top_right_radius.as_ref(),
        style.border_bottom_left_radius.as_ref(),
        style.border_bottom_right_radius.as_ref(),
    ]
    .iter()
    .filter_map(|r| {
        r.map(|t| match t.resolve() {
            Length::Px(v) => v as f64,
            _ => 0.0,
        })
    })
    .fold(0.0_f64, f64::max)
}

/// A stable key over ONLY the layout-affecting fields of a style —
/// anything that can change a node's size or position and therefore
/// requires a Taffy `set_style` + a layout pass. Paint-only properties
/// (background, color, opacity, shadow, corner radius, border *color*,
/// transitions) are deliberately excluded: changing them never moves a
/// box, so they must not trigger layout churn.
///
/// Conservative by construction: every field that *could* affect
/// intrinsic size or placement is included. Border *widths* are in
/// (a wider border insets content on web/Taffy's `border` rect and can
/// change a measured leaf's size), `font_*` is in (it changes a label's
/// intrinsic size). When in doubt a field belongs here — an extra
/// layout pass is correctness-safe; a *missing* one is a stale-frame
/// bug. Tokenized fields contribute their resolved value so a token
/// swap that changes a px dimension is seen as a layout change.
pub fn layout_affecting_key(style: &StyleRules) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(512);

    macro_rules! len {
        ($tag:literal, $field:expr) => {
            let _ = write!(s, concat!($tag, "="));
            match $field.as_ref().map(|t| t.resolve()) {
                Some(Length::Px(v)) => {
                    let _ = write!(s, "p{}", v);
                }
                Some(Length::Percent(v)) => {
                    let _ = write!(s, "%{}", v);
                }
                Some(Length::Auto) => {
                    let _ = write!(s, "a");
                }
                None => {
                    let _ = write!(s, "_");
                }
            }
            s.push(';');
        };
    }
    macro_rules! f32opt {
        ($tag:literal, $field:expr) => {
            let _ = write!(s, concat!($tag, "="));
            match $field.as_ref().map(|t| t.resolve()) {
                Some(v) => {
                    let _ = write!(s, "{}", v);
                }
                None => {
                    let _ = write!(s, "_");
                }
            }
            s.push(';');
        };
    }
    macro_rules! enom {
        ($tag:literal, $field:expr) => {
            let _ = write!(s, concat!($tag, "={:?};"), $field);
        };
    }

    // --- Sizing ---
    len!("w", style.width);
    len!("h", style.height);
    len!("minw", style.min_width);
    len!("minh", style.min_height);
    len!("maxw", style.max_width);
    len!("maxh", style.max_height);
    enom!("ar", style.aspect_ratio);

    // --- Flex container ---
    enom!("fd", style.flex_direction);
    enom!("fwrap", style.flex_wrap);
    enom!("jc", style.justify_content);
    enom!("ai", style.align_items);
    enom!("ac", style.align_content);
    len!("gap", style.gap);
    len!("rgap", style.row_gap);
    len!("cgap", style.column_gap);

    // --- Flex item ---
    f32opt!("fg", style.flex_grow);
    f32opt!("fsh", style.flex_shrink);
    len!("fb", style.flex_basis);
    enom!("as", style.align_self);

    // --- Padding / margin (inset the box / shift siblings) ---
    len!("pt", style.padding_top);
    len!("pr", style.padding_right);
    len!("pb", style.padding_bottom);
    len!("pl", style.padding_left);
    len!("mt", style.margin_top);
    len!("mr", style.margin_right);
    len!("mb", style.margin_bottom);
    len!("ml", style.margin_left);

    // --- Position + inset ---
    enom!("pos", style.position);
    len!("top", style.top);
    len!("right", style.right);
    len!("bottom", style.bottom);
    len!("left", style.left);

    // --- Border widths (a width change insets the content box) ---
    f32opt!("btw", style.border_top_width);
    f32opt!("brw", style.border_right_width);
    f32opt!("bbw", style.border_bottom_width);
    f32opt!("blw", style.border_left_width);

    // --- Typography (changes a label's intrinsic measured size) ---
    len!("fsz", style.font_size);
    enom!("ff", style.font_family);
    enom!("fwt", style.font_weight);
    enom!("fst", style.font_style);
    f32opt!("lh", style.line_height);
    f32opt!("ls", style.letter_spacing);
    enom!("ttf", style.text_transform);

    s
}

/// True iff applying `next` over the previously-applied `prev` changes
/// any layout-affecting property — i.e. a Taffy `set_style` + layout
/// pass is actually required. `prev = None` (first apply for the node)
/// always returns `true`. Paint-only deltas return `false`.
pub fn is_layout_affecting(prev: Option<&str>, next_key: &str) -> bool {
    match prev {
        None => true,
        Some(p) => p != next_key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::{Color, Length, StyleRules, Tokenized};

    fn px(v: f32) -> Tokenized<Length> {
        Tokenized::Literal(Length::Px(v))
    }

    // === Corner radius (the visible bug) ===============================

    // Explicit px dims → clamp against them, regardless of bounds.
    #[test]
    fn px_dims_clamp_immediately() {
        // requested 999 on a 200px-wide box → clamp to 100 (half).
        assert_eq!(
            resolve_corner_radius(999.0, Some(100.0), None),
            CornerRadiusDecision::Apply(100.0),
        );
        // px cap wins even if bounds are also known.
        assert_eq!(
            resolve_corner_radius(999.0, Some(100.0), Some(40.0)),
            CornerRadiusDecision::Apply(100.0),
        );
    }

    // The regression: a reactive re-style of an already-laid-out
    // percent-sized view. BEFORE the fix this path zeroed cornerRadius
    // and deferred to a layout pass that the frame-key cache skips
    // (no frame change on a paint-only re-style), so the corner went
    // square. AFTER the fix, real bounds are present so we clamp NOW
    // and the corner survives.
    #[test]
    fn regression_ios_corner_radius_survives_restyle() {
        // No px cap (percent-sized), but the view is already laid out
        // at 80×40 → min half = 20. A "make it a pill" 999 request must
        // clamp to 20 immediately, NOT defer/zero.
        let decision = resolve_corner_radius(999.0, None, Some(20.0));
        assert_eq!(decision, CornerRadiusDecision::Apply(20.0));
        assert_ne!(decision, CornerRadiusDecision::Apply(0.0));
        assert_ne!(decision, CornerRadiusDecision::Defer(999.0));
    }

    // First apply, pre-layout (bounds 0×0): still defer so we don't
    // paint the "999 on a 0×0 view → blank" state. The layout pass's
    // sync_corner_radius clamps once bounds arrive.
    #[test]
    fn pre_layout_percent_view_defers() {
        assert_eq!(
            resolve_corner_radius(999.0, None, None),
            CornerRadiusDecision::Defer(999.0),
        );
        assert_eq!(
            resolve_corner_radius(999.0, None, Some(0.0)),
            CornerRadiusDecision::Defer(999.0),
        );
    }

    #[test]
    fn zero_request_is_none() {
        assert_eq!(resolve_corner_radius(0.0, Some(100.0), Some(20.0)), CornerRadiusDecision::None);
    }

    #[test]
    fn px_cap_from_explicit_dims() {
        let mut s = StyleRules::default();
        s.width = Some(px(200.0));
        s.height = Some(px(80.0));
        // min(100, 40) = 40
        assert_eq!(px_cap_from_style(&s), Some(40.0));
    }

    #[test]
    fn px_cap_none_for_percent() {
        let mut s = StyleRules::default();
        s.width = Some(Tokenized::Literal(Length::Percent(100.0)));
        assert_eq!(px_cap_from_style(&s), None);
    }

    // === Paint-only vs layout-affecting (the perf bug) =================

    // The regression: a paint-only re-style (background/opacity/color)
    // used to schedule a layout pass on every press. The layout key
    // must be IDENTICAL before/after such a change, so the apply path
    // can skip both `set_style` and the layout pass.
    #[test]
    fn regression_ios_paint_only_skips_layout() {
        let mut base = StyleRules::default();
        base.width = Some(px(120.0));
        base.padding_top = Some(px(8.0));
        let key_before = layout_affecting_key(&base);

        // Flip ONLY paint properties: background, opacity, a corner
        // radius, color, shadow, a border color. None move the box.
        let mut after = base.clone();
        after.background = Some(Tokenized::Literal(Color("#123456".into())));
        after.opacity = Some(Tokenized::Literal(0.5));
        after.color = Some(Tokenized::Literal(Color("#fff".into())));
        after.border_top_left_radius = Some(px(8.0));
        after.border_top_color = Some(Tokenized::Literal(Color("#000".into())));
        let key_after = layout_affecting_key(&after);

        assert_eq!(
            key_before, key_after,
            "paint-only delta changed the layout key — would force a needless layout pass",
        );
        assert!(!is_layout_affecting(Some(&key_before), &key_after));
    }

    // A genuine size change MUST still flag layout.
    #[test]
    fn size_change_is_layout_affecting() {
        let mut base = StyleRules::default();
        base.width = Some(px(120.0));
        let key_before = layout_affecting_key(&base);

        let mut after = base.clone();
        after.width = Some(px(160.0));
        let key_after = layout_affecting_key(&after);

        assert_ne!(key_before, key_after);
        assert!(is_layout_affecting(Some(&key_before), &key_after));
    }

    // Padding, font-size, border-width, flex props all flag layout —
    // each can change a box's size or its children's placement.
    #[test]
    fn intrinsic_size_props_are_layout_affecting() {
        let base = StyleRules::default();
        let base_key = layout_affecting_key(&base);

        let mut pad = StyleRules::default();
        pad.padding_left = Some(px(4.0));
        assert!(is_layout_affecting(Some(&base_key), &layout_affecting_key(&pad)));

        let mut font = StyleRules::default();
        font.font_size = Some(px(20.0));
        assert!(is_layout_affecting(Some(&base_key), &layout_affecting_key(&font)));

        let mut bw = StyleRules::default();
        bw.border_top_width = Some(Tokenized::Literal(2.0));
        assert!(is_layout_affecting(Some(&base_key), &layout_affecting_key(&bw)));

        let mut fg = StyleRules::default();
        fg.flex_grow = Some(Tokenized::Literal(1.0));
        assert!(is_layout_affecting(Some(&base_key), &layout_affecting_key(&fg)));
    }

    // First apply (no prior key) always counts as layout-affecting —
    // the node has never been sized.
    #[test]
    fn first_apply_is_layout_affecting() {
        let key = layout_affecting_key(&StyleRules::default());
        assert!(is_layout_affecting(None, &key));
    }

    // === Effective text color (the invisible-dark-mode-text bug) =======
    //
    // A full UIKit test isn't reachable on the host (UILabel.textColor is
    // an objc property on a class that only links on-device), so we test
    // the pure decision that feeds the `setTextColor:` call. The bug:
    // when `style.color` is absent, `apply_text_style` used to leave the
    // label at UIKit's `labelColor` — white in dark mode → invisible over
    // a light surface. The fix routes the absent case to the theme's
    // `color-text` token instead.

    // Explicit author color passes straight through — author always wins.
    #[test]
    fn explicit_text_color_wins() {
        let explicit = Tokenized::Literal(Color("#ff0000".into()));
        assert_eq!(effective_text_color(Some(&explicit)), explicit);

        // A token color the author named also passes through unchanged.
        let tok: Tokenized<Color> = Tokenized::token("color-accent", Color("#0af".into()));
        assert_eq!(effective_text_color(Some(&tok)), tok);
    }

    // The regression: no explicit color must yield the THEME's text color
    // token — never an OS/system label color. We assert the result is the
    // `color-text` token (so `.resolve()` reads the installed theme), and
    // that its no-theme fallback is a visible dark color, not anything
    // system-appearance-derived.
    #[test]
    fn regression_absent_text_color_uses_theme_token_not_os_default() {
        let effective = effective_text_color(None);
        match &effective {
            Tokenized::Token { name, fallback } => {
                assert_eq!(
                    *name, THEME_TEXT_COLOR_TOKEN,
                    "absent text color must resolve through the theme's color-text token",
                );
                assert_eq!(*name, "color-text");
                assert_eq!(fallback.0, THEME_TEXT_COLOR_FALLBACK);
                // The fallback is a concrete dark color (visible on a
                // light surface), NOT the OS system label color.
                assert_eq!(fallback.0, "#1a1a1f");
            }
            Tokenized::Literal(_) => {
                panic!("absent text color must be a token, so a theme swap re-fires it");
            }
        }
    }

    // -- Editable text-control background (the Textarea-renders-black bug) --
    //
    // A full UIKit/AppKit test isn't reachable on the host (UITextView /
    // NSTextView only link on-device). We test the pure decision that feeds
    // the native `setBackgroundColor:` / `drawsBackground` call. The bug: an
    // editable control with no explicit background renders the OS
    // `systemBackground` — near-black in dark mode, so idea-ui's Textarea
    // (which DOES set an explicit `color-surface`) showed as a dark box,
    // meaning the explicit value wasn't reaching the native control.

    // The iOS re-exports delegate to the shared apple-core decision (where the
    // exhaustive cases are tested). These assert the delegation resolves to the
    // theme tokens — never an OS system fill (dark box) — and that absent input
    // text color lines up with the label decision (same `color-text` token).
    #[test]
    fn regression_absent_input_background_uses_theme_surface_not_os_default() {
        match effective_input_background(None) {
            Tokenized::Token { name, fallback } => {
                assert_eq!(name, "color-surface");
                assert_eq!(fallback.0, "#ffffff");
            }
            Tokenized::Literal(_) => {
                panic!("absent input background must be a token so a theme swap re-fires it");
            }
        }
        // Explicit author background (idea-ui's text_area path) wins unchanged.
        let surface: Tokenized<Color> =
            Tokenized::token("color-surface", Color("#ffffff".into()));
        assert_eq!(effective_input_background(Some(&surface)), surface);
    }

    #[test]
    fn input_text_color_matches_label_decision() {
        assert_eq!(effective_input_text_color(None), effective_text_color(None));
        let explicit = Tokenized::Literal(Color("#123456".into()));
        assert_eq!(
            effective_input_text_color(Some(&explicit)),
            effective_text_color(Some(&explicit)),
        );
    }
}
