//! Text-style application for `NSTextField` (label mode) and
//! `NSTextView` (text-area). Mirrors `backend_ios_core::style::
//! apply_text_style` — same shape, AppKit setters.

use backend_apple_core::font::FontRegistry;
use runtime_shared::{FontFamily, FontStyle, FontWeight, StyleRules};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_app_kit::NSView;
use objc2_foundation::{CGFloat, CGSize, NSObject, NSString};

/// AppKit's NSTextAlignment enum values. The `objc2-app-kit`
/// generated bindings define them; we mirror raw values here so
/// `msg_send!` can hand the right integer without pulling in a
/// feature that drags more code into the build.
const NS_TEXT_ALIGNMENT_LEFT: isize = 0;
const NS_TEXT_ALIGNMENT_RIGHT: isize = 1;
const NS_TEXT_ALIGNMENT_CENTER: isize = 2;
const NS_TEXT_ALIGNMENT_JUSTIFIED: isize = 3;

/// Apply text-related style props to an NSTextField (label) or
/// NSTextView. Reads `style.color`, `style.font_*`, `style.text_align`.
///
/// `is_label`: true for NSTextField in label mode (different
/// `setStringValue:` path); false for NSTextView (uses `setString:`
/// and behaves like a UITextView).
pub(crate) fn apply_text_style(
    view: &NSView,
    style: &StyleRules,
    is_label: bool,
    font_registry: &FontRegistry,
) {
    // Text color — via the transition system so `color: …` animates over
    // `color_transition` (e.g. the theme toggle's text fade) instead of snapping.
    if let Some(color) = &style.color {
        let rgba = crate::imp::style_color_rgba(&color.resolve());
        crate::imp::transitions::apply_color(
            view,
            crate::imp::transitions::ColorProp::TextColor,
            false,
            rgba,
            style.color_transition.as_ref(),
        );
    }

    // Font: route through the registry first (custom typefaces),
    // fall back to system font.
    let has_typography = style.font_family.is_some()
        || style.font_size.is_some()
        || style.font_weight.is_some()
        || style.font_style.is_some();
    if has_typography {
        let weight = style
            .font_weight
            .as_ref()
            .copied()
            .unwrap_or(FontWeight::Normal);
        let fstyle = style
            .font_style
            .as_ref()
            .copied()
            .unwrap_or(FontStyle::Normal);
        let size = match style.font_size.as_ref().map(|t| t.resolve()) {
            Some(len) => {
                let px = length_to_px(&len);
                if px > 0.0 { px } else { 13.0 as CGFloat }
            }
            None => 13.0 as CGFloat,
        };
        // No explicit `font_family` still means the author-set weight/size/style
        // must land — web applies `font-weight`/`font-size` to any text
        // regardless of family, and iOS falls back to the system font here
        // (`style.rs`'s `!applied` branch). macOS previously dropped the whole
        // font when `resolve_nsfont` returned `None` (no family), so a
        // `font_weight: SemiBold` with no family left the label at AppKit's
        // default regular 13px — the idea-ui Button "not bold" bug. Fall back to
        // the weighted system font so weight/size apply on every backend (Rule
        // #7: converge output).
        let font = resolve_nsfont(font_registry, style.font_family.as_ref(), weight, fstyle, size)
            .unwrap_or_else(|| system_font(weight, size));
        let _: () = unsafe { msg_send![view, setFont: &*font] };
    }

    // Text alignment
    if let Some(ta) = &style.text_align {
        let align: isize = match ta {
            runtime_shared::TextAlign::Left => NS_TEXT_ALIGNMENT_LEFT,
            runtime_shared::TextAlign::Right => NS_TEXT_ALIGNMENT_RIGHT,
            runtime_shared::TextAlign::Center => NS_TEXT_ALIGNMENT_CENTER,
            runtime_shared::TextAlign::Justify => NS_TEXT_ALIGNMENT_JUSTIFIED,
        };
        let _: () = unsafe { msg_send![view, setAlignment: align] };
    }

    apply_text_shadow(view, style);

    // `max_lines` is a `text()` property: only the display label truncates.
    // The NSTextView path (`text_area`) is an editor, sized by its own
    // `min_rows`/`max_rows`, and must never hide typed text behind an ellipsis.
    if is_label {
        apply_label_line_limit(view, style.max_lines);
    }
}

/// How a display label's cell is configured for a `max_lines` value.
/// `max_lines` is the `NSTextField.maximumNumberOfLines` to write (0 = no
/// limit, AppKit's own convention); `truncates_last_visible_line` is the
/// cell's `truncatesLastVisibleLine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LabelLineLimit {
    pub(crate) max_lines: isize,
    pub(crate) truncates_last_visible_line: bool,
}

/// The default a label gets at create time (`create_text_impl`): wrap freely,
/// never truncate. `None`/`Some(0)` restore exactly this.
pub(crate) const LABEL_NO_LINE_LIMIT: LabelLineLimit =
    LabelLineLimit { max_lines: 0, truncates_last_visible_line: false };

/// Map `StyleRules::max_lines` to the label configuration. `None` and
/// `Some(0)` both mean "no limit" (the style field's contract).
pub(crate) fn label_line_limit(max_lines: Option<u32>) -> LabelLineLimit {
    match max_lines {
        Some(n) if n > 0 => LabelLineLimit {
            max_lines: n.min(isize::MAX as u32) as isize,
            truncates_last_visible_line: true,
        },
        _ => LABEL_NO_LINE_LIMIT,
    }
}

/// Apply (or clear) the `max_lines` limit on a display label.
///
/// ONE recipe for every `n`, including 1: keep the cell WORD-WRAPPING
/// (`wraps = true`, the create-time default), cap it with
/// `maximumNumberOfLines = n`, and set `truncatesLastVisibleLine` so the last
/// kept line ends in "…". Verified on macOS 26 (`cellSizeForBounds:` and a
/// rendered bitmap): this measures exactly `n` lines and draws a tail
/// ellipsis, for plain AND attributed (styled-run) strings.
///
/// Why NOT `lineBreakMode = NSLineBreakByTruncatingTail` (the obvious
/// mapping): on an `NSCell`, `lineBreakMode` and `wraps` are one property —
/// a truncating mode sets `wraps = false`. The cell then lays out a single
/// line (so `n > 1` collapses to one), ignores `maximumNumberOfLines` for an
/// embedded `\n` (two lines measured for `max_lines: 1`), and an attributed
/// string with no paragraph style measures fully wrapped anyway (7 lines for
/// `max_lines: 1`) — the Taffy measure would then disagree with the drawing.
///
/// The measure needs no change of its own: `create_text_impl`'s measure_fn
/// asks the same cell for `cellSizeForBounds:`, which honours
/// `maximumNumberOfLines` in this configuration (height ≤ `n` lines; with
/// `n = 1` the text never wraps). `text_measure_signature` includes
/// `max_lines`, so a limit change re-measures.
///
/// Writes only on change: `apply_style` re-runs on every hover restyle, and
/// the setters invalidate the cell's display.
pub(crate) fn apply_label_line_limit(label: &NSView, max_lines: Option<u32>) {
    let want = label_line_limit(max_lines);
    let responds: bool =
        unsafe { msg_send![label, respondsToSelector: objc2::sel!(setMaximumNumberOfLines:)] };
    if !responds {
        return;
    }
    let cell: *mut NSObject = unsafe { msg_send![label, cell] };
    if cell.is_null() {
        return;
    }
    let current: isize = unsafe { msg_send![label, maximumNumberOfLines] };
    if current != want.max_lines {
        let _: () = unsafe { msg_send![label, setMaximumNumberOfLines: want.max_lines] };
    }
    let truncates: bool = unsafe { msg_send![cell, truncatesLastVisibleLine] };
    if truncates != want.truncates_last_visible_line {
        let _: () = unsafe {
            msg_send![cell, setTruncatesLastVisibleLine: want.truncates_last_visible_line]
        };
    }
    // The recipe depends on word-wrapping; nothing else in the backend clears
    // it, but assert the invariant rather than trust it.
    let wraps: bool = unsafe { msg_send![cell, wraps] };
    if !wraps {
        let _: () = unsafe { msg_send![cell, setWraps: true] };
    }
}

/// Apply (or clear) the text primitive's GLYPH shadow — the
/// `text_shadow` field (web lowers it to `text-shadow`; the framework
/// converges the output across backends, Rule #7). Here it's a CALayer
/// shadow on the label's backing layer: an `NSTextField`'s layer
/// content is the drawn glyphs over a transparent background, so the
/// layer shadow takes the glyph silhouette rather than the box.
/// Mirrors `backend_ios_core`'s text-shadow path.
fn apply_text_shadow(view: &NSView, style: &StyleRules) {
    // Label views are layer-backed (`apply_style_to_view` sets
    // `wantsLayer` before this runs); fetch that same layer.
    let _: () = unsafe { msg_send![view, setWantsLayer: true] };
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
    match &style.text_shadow {
        Some(sh) => {
            // `shadowColor` carries the alpha; `shadowOpacity` is a plain
            // enable multiplier at 1.0 (CALayer multiplies the two, so the
            // effective strength is exactly the author's color alpha).
            let ns_color = crate::imp::color_to_nscolor(&sh.color);
            let cg: crate::imp::CGColorRef = unsafe { msg_send![&*ns_color, CGColor] };
            if !cg.0.is_null() {
                let _: () = unsafe { msg_send![&layer, setShadowColor: cg] };
            }
            let _: () = unsafe { msg_send![&layer, setShadowOpacity: 1.0f32] };
            let _: () = unsafe { msg_send![&layer, setShadowRadius: sh.blur as CGFloat] };
            let (w, h) = text_shadow_offset(sh.x, sh.y);
            let offset = CGSize { width: w, height: h };
            let _: () = unsafe { msg_send![&layer, setShadowOffset: offset] };
        }
        None => {
            // No shadow in THIS restyle → clear any a prior style left, so a
            // reactively-toggled shadow actually turns off (the same
            // set-then-never-unset hazard the background path guards).
            let _: () = unsafe { msg_send![&layer, setShadowOpacity: 0.0f32] };
        }
    }
}

/// Build an `NSFont` for the given style. `family` is the optional
/// `font_family` from `StyleRules`; `weight`/`style` are the
/// resolved typography knobs.
///
/// Routes through the cross-Apple font registry first (custom
/// typefaces registered via `register_asset`); falls through to
/// `+[NSFont fontWithName:size:]` for `FontFamily::System(name)`;
/// falls through finally to `+[NSFont systemFontOfSize:weight:]`.
pub(crate) fn resolve_nsfont(
    registry: &FontRegistry,
    family: Option<&FontFamily>,
    weight: FontWeight,
    style: FontStyle,
    size: CGFloat,
) -> Option<Retained<NSObject>> {
    let family = family?;
    match family {
        FontFamily::Typeface(t) => {
            let resolved = registry.resolve_typeface(t, weight, style);
            if let Some(face) = resolved {
                ns_font_with_name(face.postscript_name, size)
                    .or_else(|| ns_font_with_name(face.family_name, size))
                    .or_else(|| resolve_system_fallback(t.fallback, weight, size))
            } else {
                resolve_system_fallback(t.fallback, weight, size)
            }
        }
        FontFamily::System(name) => ns_font_with_name(name, size)
            .or_else(|| Some(system_font(weight, size))),
    }
}

/// `+[NSFont fontWithName:size:]` — returns `None` if AppKit
/// doesn't recognize the name.
pub(crate) fn ns_font_with_name(name: &str, size: CGFloat) -> Option<Retained<NSObject>> {
    let ns_name = NSString::from_str(name);
    let font: Option<Retained<NSObject>> = unsafe {
        msg_send_id![
            objc2::class!(NSFont),
            fontWithName: &*ns_name,
            size: size
        ]
    };
    font
}

/// `+[NSFont systemFontOfSize:weight:]`. The weight axis is the
/// same -1.0..1.0 NSFontWeight as `UIFont.Weight` (both bridge to
/// `CGFloat`), so the iOS weight mapping is reusable here.
pub(crate) fn system_font(weight: FontWeight, size: CGFloat) -> Retained<NSObject> {
    let w = font_weight_to_nsfont(weight);
    let font: Retained<NSObject> = unsafe {
        msg_send_id![
            objc2::class!(NSFont),
            systemFontOfSize: size,
            weight: w
        ]
    };
    font
}

/// Generic-role fallback for a typeface that couldn't be resolved.
/// Same mapping iOS uses (serif → Times New Roman, monospace →
/// Menlo, sans → system).
fn resolve_system_fallback(
    fallback: runtime_shared::assets::SystemFallback,
    weight: FontWeight,
    size: CGFloat,
) -> Option<Retained<NSObject>> {
    use runtime_shared::assets::SystemFallback;
    match fallback {
        SystemFallback::Serif => ns_font_with_name("Times New Roman", size)
            .or_else(|| Some(system_font(weight, size))),
        SystemFallback::Monospace => ns_font_with_name("Menlo", size)
            .or_else(|| Some(system_font(weight, size))),
        SystemFallback::SansSerif | SystemFallback::None => Some(system_font(weight, size)),
    }
}

/// Map framework `FontWeight` to NSFontWeight (same -1.0..1.0 axis
/// UIFont uses). Mirrors `backend_ios_core::style::font_weight_to_uikit`.
pub(crate) fn font_weight_to_nsfont(weight: FontWeight) -> CGFloat {
    match weight {
        FontWeight::Thin => -0.6,
        FontWeight::ExtraLight => -0.5,
        FontWeight::Light => -0.4,
        FontWeight::Normal => 0.0,
        FontWeight::Medium => 0.23,
        FontWeight::SemiBold => 0.3,
        FontWeight::Bold => 0.4,
        FontWeight::ExtraBold => 0.56,
        FontWeight::Black => 0.62,
    }
}

/// Translate a `Shadow`'s `(x, y)` offset (web semantics: +x right, +y
/// DOWN) into a CALayer `shadowOffset` `(width, height)`. An `NSTextField`
/// is a plain (non-flipped) `NSView`, so its layer geometry is y-up — a
/// positive `height` casts the shadow *upward*. Negating `y` makes the
/// macOS shadow land in the same place as web's `text-shadow` and iOS's
/// layer shadow (UIKit is y-down, so iOS passes `+y` directly). Pure so
/// the sign convention — the one non-obvious bit — is unit-testable off
/// the main thread.
fn text_shadow_offset(x: f32, y: f32) -> (CGFloat, CGFloat) {
    (x as CGFloat, -(y as CGFloat))
}

pub(crate) fn length_to_px(len: &runtime_shared::Length) -> CGFloat {
    match len {
        runtime_shared::Length::Px(v) => *v as CGFloat,
        // This one serves TEXT lengths (font size, styled-run metrics), not
        // corner radii. `Full` is the pill radius and has no meaning on a
        // font size, so it lands with Percent/Auto on "no defined value" and
        // the caller falls back to its default. The radius-context
        // `length_to_px` in `imp/mod.rs` is the one that maps `Full` to
        // `FULL_RADIUS_FALLBACK_PX`.
        runtime_shared::Length::Full
        | runtime_shared::Length::Percent(_)
        | runtime_shared::Length::Auto => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression: macOS dropped the ENTIRE font whenever a text node set
    // `font_weight`/`font_size` without an explicit `font_family` — `resolve_nsfont`
    // early-returns `None` on no family, and the old `apply_text_style` only sent
    // `setFont:` inside `if let Some(f)`. So the idea-ui Button's label (weight
    // `SemiBold`, no family) rendered at AppKit's default regular 13px: the
    // "not bold" bug. iOS never had this (its `!applied` branch backfills the
    // weighted system font). `apply_text_style` now mirrors iOS via
    // `.unwrap_or_else(|| system_font(weight, size))`.
    //
    // The live `setFont:` needs a main-thread NSView (the `cargo test` harness
    // runs off the main thread), but `NSFont` is not `MainThreadOnly`, so we
    // exercise the fallback builder + the family-None gap it covers directly —
    // the same "test the reachable deterministic pieces" pattern the cell tests
    // in `view.rs` use.
    #[test]
    fn no_family_leaves_resolve_nsfont_none_so_the_fallback_must_run() {
        let reg = FontRegistry::new();
        // The gap: with no family there is nothing to resolve, so the fallback
        // in `apply_text_style` is the ONLY thing that applies the weight/size.
        let resolved = resolve_nsfont(&reg, None, FontWeight::SemiBold, FontStyle::Normal, 14.0);
        assert!(
            resolved.is_none(),
            "no font_family → resolve_nsfont is None; the system-font fallback covers it"
        );
    }

    // The text-shadow offset must converge with web/iOS: web `text-shadow`
    // and iOS's layer shadow put `y: 2` BELOW the glyphs. macOS labels are
    // non-flipped (y-up), so the layer offset height negates to match. A
    // regression here (dropping the negation) flips the shadow above the
    // text on macOS only — the exact per-platform divergence Rule #7 bans.
    #[test]
    fn text_shadow_offset_negates_y_for_nonflipped_label() {
        assert_eq!(text_shadow_offset(1.0, 2.0), (1.0 as CGFloat, -2.0 as CGFloat));
        assert_eq!(text_shadow_offset(-3.0, 0.0), (-3.0 as CGFloat, 0.0 as CGFloat));
    }

    #[test]
    fn system_font_fallback_honors_requested_size_and_weight() {
        // The fallback face carries the author's size (dropped by the old bug)…
        let f = system_font(FontWeight::SemiBold, 14.0);
        let size: CGFloat = unsafe { msg_send![&*f, pointSize] };
        assert_eq!(size, 14.0, "fallback system font must honor the requested font_size");
        // …and its weight axis is threaded through: SemiBold maps to a heavier
        // NSFontWeight than Normal, so the label is visibly bolder than default.
        assert!(
            font_weight_to_nsfont(FontWeight::SemiBold) > font_weight_to_nsfont(FontWeight::Normal),
            "SemiBold must map to a heavier NSFontWeight than Normal"
        );
    }

    // ---- max_lines ------------------------------------------------------

    use crate::imp::MacosBackend;
    use objc2_foundation::{CGPoint, CGRect, MainThreadMarker};
    use std::rc::Rc;

    const LONG: &str = "The quick brown fox jumps over the lazy dog again and again \
                        and again until the end of time";

    #[test]
    fn label_line_limit_maps_none_and_zero_to_no_limit() {
        assert_eq!(label_line_limit(None), LABEL_NO_LINE_LIMIT);
        assert_eq!(label_line_limit(Some(0)), LABEL_NO_LINE_LIMIT);
        assert_eq!(
            label_line_limit(Some(1)),
            LabelLineLimit { max_lines: 1, truncates_last_visible_line: true }
        );
        assert_eq!(
            label_line_limit(Some(3)),
            LabelLineLimit { max_lines: 3, truncates_last_visible_line: true }
        );
    }

    fn cell_of(label: &NSView) -> Retained<NSObject> {
        unsafe { msg_send_id![label, cell] }
    }

    fn fitted_height(label: &NSView, width: f64) -> f64 {
        let cell = cell_of(label);
        let bounds = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width, height: 10_000.0 },
        };
        let size: CGSize = unsafe { msg_send![&cell, cellSizeForBounds: bounds] };
        size.height
    }

    fn line_config(label: &NSView) -> (isize, bool, bool) {
        let cell = cell_of(label);
        let max: isize = unsafe { msg_send![label, maximumNumberOfLines] };
        let trunc: bool = unsafe { msg_send![&cell, truncatesLastVisibleLine] };
        let wraps: bool = unsafe { msg_send![&cell, wraps] };
        (max, trunc, wraps)
    }

    /// A text node built by the real `create_text_impl`, plus its height when
    /// it holds a single short line (the one-line yardstick).
    fn text_node(backend: &mut MacosBackend, content: &str) -> crate::imp::MacosNode {
        backend.create_text_impl(content, &Default::default())
    }

    fn one_line_height(backend: &mut MacosBackend) -> f64 {
        let probe = text_node(backend, "Ag");
        fitted_height(probe.as_view(), 10_000.0)
    }

    fn style_with(max_lines: Option<u32>) -> Rc<StyleRules> {
        let mut s = StyleRules::default();
        s.max_lines = max_lines;
        Rc::new(s)
    }

    // The widget half: `apply_style` on a text node configures the label so
    // AppKit itself caps + ellipsizes it, and a restyle that drops the limit
    // restores the create-time defaults (wrap freely, no truncation).
    #[test]
    fn max_lines_configures_the_label_and_resets_when_removed() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = MacosBackend::new(mtm);
        let node = text_node(&mut backend, LONG);
        let view = node.as_view();
        assert_eq!(line_config(view), (0, false, true), "create-time default");

        backend.apply_style_impl(&node, &style_with(Some(1)));
        assert_eq!(line_config(view), (1, true, true));

        backend.apply_style_impl(&node, &style_with(Some(3)));
        assert_eq!(line_config(view), (3, true, true));

        backend.apply_style_impl(&node, &style_with(None));
        assert_eq!(line_config(view), (0, false, true), "None restores the default");

        backend.apply_style_impl(&node, &style_with(Some(2)));
        backend.apply_style_impl(&node, &style_with(Some(0)));
        assert_eq!(line_config(view), (0, false, true), "Some(0) means no limit");
    }

    // The measure half, end to end: the label's Taffy measure_fn (installed by
    // `create_text_impl`) must report at most `n` lines, through a real layout
    // pass. 100px is narrow enough that LONG wraps to many lines unlimited.
    #[test]
    fn max_lines_caps_the_measured_height_through_layout() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = MacosBackend::new(mtm);
        let line = one_line_height(&mut backend);
        let node = text_node(&mut backend, LONG);
        let label = backend.layout_of(node.as_view()).expect("text has a layout node");
        let root = backend.layout.new_node();
        backend.layout.add_child(root, label);

        let height_for = |backend: &mut MacosBackend, limit: Option<u32>| {
            backend.apply_style_impl(&node, &style_with(limit));
            backend.layout.compute(root, 100.0, 2_000.0);
            backend.layout.frame_of(label).height as f64
        };

        let unlimited = height_for(&mut backend, None);
        assert!(unlimited > line * 3.0, "LONG must wrap to several lines at 100px, got {unlimited}");
        let one = height_for(&mut backend, Some(1));
        assert!((one - line).abs() < 0.5, "max_lines 1 must measure one line ({line}), got {one}");
        let two = height_for(&mut backend, Some(2));
        assert!((two - 2.0 * line).abs() < 0.5, "max_lines 2 must measure two lines, got {two}");
        // Dropping the limit must re-measure back to the full height (the
        // measure signature includes `max_lines`, so the node is re-dirtied).
        let again = height_for(&mut backend, None);
        assert!((again - unlimited).abs() < 0.5, "removing the limit restores {unlimited}, got {again}");
    }

    // Styled-run labels carry an attributed string (no paragraph style); the
    // cell's line limit must govern it the same way. Guards against the
    // `lineBreakMode = TruncatingTail` mapping, under which an attributed
    // string measured fully wrapped (7 lines for `max_lines: 1`).
    #[test]
    fn max_lines_caps_a_styled_text_label() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = MacosBackend::new(mtm);
        let line = one_line_height(&mut backend);
        let runs = vec![
            runtime_shared::TextRun::plain("The quick brown fox "),
            runtime_shared::TextRun::plain(LONG),
        ];
        let node = backend.create_styled_text_impl(&runs, &Default::default());
        backend.apply_style_impl(&node, &style_with(Some(1)));
        let h = fitted_height(node.as_view(), 100.0);
        assert!((h - line).abs() < 0.5, "styled max_lines 1 must measure one line ({line}), got {h}");
        backend.apply_style_impl(&node, &style_with(Some(2)));
        let h = fitted_height(node.as_view(), 100.0);
        assert!((h - 2.0 * line).abs() < 0.5, "styled max_lines 2 must measure two lines, got {h}");
    }

    // One line never wraps: an embedded newline must not add a second line
    // (with a truncating `lineBreakMode` it did — two lines for `max_lines: 1`),
    // and the max-content width is the natural single-line width, so Taffy's
    // `overflow: hidden` shrink is what narrows it.
    #[test]
    fn max_lines_one_never_wraps() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = MacosBackend::new(mtm);
        let line = one_line_height(&mut backend);
        let node = text_node(&mut backend, "First line long enough to truncate\nsecond line");
        backend.apply_style_impl(&node, &style_with(Some(1)));
        let narrow = fitted_height(node.as_view(), 100.0);
        let wide = fitted_height(node.as_view(), 10_000.0);
        assert!((narrow - line).abs() < 0.5, "got {narrow}, want one line {line}");
        assert!((wide - line).abs() < 0.5, "got {wide}, want one line {line}");
    }
}
