//! Styled text runs on AppKit — realize a `TextRun` list as ONE
//! `NSAttributedString` on the label's `attributedStringValue`, so a
//! mixed-style paragraph (prose + inline-code chips) wraps as a
//! single unit through AppKit's own text engine.
//!
//! Every run is FULLY attributed (base paragraph font/color merged
//! with the run's deltas), never left to inherit from the field:
//! `NSCell` draws an attributed string as-is and does not reliably
//! merge the field's own font/textColor into unattributed ranges the
//! way `UILabel` does — a plain-inherit run would risk rendering in
//! the AppKit default font instead of the paragraph style. Explicit
//! attribution makes the output deterministic (CLAUDE.md §7:
//! converge output).
//!
//! Re-realization happens on every `apply_style` that hits a
//! styled-text label (paragraph style changed) and on theme swaps via
//! the walker's cohort entry (`Backend::update_styled_text`) — run
//! token colors resolve at realize time, so both paths re-resolve
//! against the current theme. Chip colors therefore SNAP on a theme
//! swap while plain text may fade through the transition system;
//! accepted divergence-in-time (final state identical) because
//! per-range animated attributed strings would need a custom text
//! layer.

use objc2::rc::Retained;
use objc2::msg_send;
use objc2_app_kit::NSTextField;
use objc2_foundation::{NSAttributedString, NSObject};
use runtime_shared::{FontFamily, FontWeight, StyleRules, TextRun, TextRunStyle};

use backend_apple_core::font::FontRegistry;
use backend_apple_core::styled_text::{
    build_attributed, parse_font_stack, run_font_size, run_needs_font, underline_mask,
    with_italic_trait, RunAttrs, StackEntry,
};

use super::text_style::{ns_font_with_name, resolve_nsfont, system_font};
use super::{color_to_nscolor, theme_text_color};

/// Per-label styled-text state, keyed by view pointer in
/// `MacosBackend::styled_texts`. `para` is the last paragraph style
/// `apply_style` delivered — the base the run deltas layer over.
pub(crate) struct StyledTextEntry {
    pub(crate) runs: Vec<TextRun>,
    pub(crate) para: Option<std::rc::Rc<StyleRules>>,
}

/// Build and install the attributed string for `runs` over the
/// paragraph style `para` (`None` = pre-style defaults: system font at
/// AppKit's 13pt label size, the installed theme's `color-text`).
pub(crate) fn realize(
    label: &NSTextField,
    runs: &[TextRun],
    para: Option<&StyleRules>,
    registry: &FontRegistry,
) {
    // Base font — mirror `apply_text_style`'s resolution exactly so a
    // plain run renders byte-identical to plain text under the same
    // style. 13.0 is AppKit's label default and `apply_text_style`'s
    // own fallback size.
    let base_size = para
        .and_then(|s| s.font_size.as_ref().map(|t| t.resolve()))
        .map(|len| {
            let px = super::text_style::length_to_px(&len);
            if px > 0.0 { px } else { 13.0 }
        })
        .unwrap_or(13.0);
    let base_weight = para
        .and_then(|s| s.font_weight)
        .unwrap_or(FontWeight::Normal);
    let base_style = para
        .and_then(|s| s.font_style)
        .unwrap_or(runtime_shared::FontStyle::Normal);
    let base_font = para
        .and_then(|s| resolve_nsfont(registry, s.font_family.as_ref(), base_weight, base_style, base_size))
        .unwrap_or_else(|| system_font(base_weight, base_size));
    // Base color — explicit paragraph color, else the theme's
    // `color-text` (the same default `create_text` seeds).
    let base_color = para
        .and_then(|s| s.color.as_ref())
        .map(|t| t.resolve())
        .unwrap_or_else(theme_text_color);
    let base_nscolor = color_to_nscolor(&base_color);

    let assembled: Vec<(&str, RunAttrs)> = runs
        .iter()
        .map(|run| {
            let attrs = match &run.style {
                None => RunAttrs {
                    font: Some(retain_as_nsobject(&base_font)),
                    foreground: Some(nscolor_as_nsobject(&base_nscolor)),
                    background: None,
                    underline: None,
                    underline_color: None,
                },
                Some(s) => RunAttrs {
                    font: Some(if run_needs_font(s) {
                        run_font(s, base_size, base_weight, base_style, registry)
                    } else {
                        retain_as_nsobject(&base_font)
                    }),
                    foreground: Some(match &s.color {
                        Some(c) => nscolor_as_nsobject(&color_to_nscolor(&c.resolve())),
                        None => nscolor_as_nsobject(&base_nscolor),
                    }),
                    background: s
                        .background
                        .as_ref()
                        .map(|b| nscolor_as_nsobject(&color_to_nscolor(&b.resolve()))),
                    underline: s.underline.as_ref().map(|u| underline_mask(u.style)),
                    // No explicit color ⇒ omit the attribute entirely so
                    // AppKit draws the line in the run's own foreground
                    // color (its documented default). Writing the base
                    // color here instead would pin diagnostics lines to
                    // the paragraph color on a theme swap.
                    underline_color: s
                        .underline
                        .as_ref()
                        .and_then(|u| u.color.as_ref())
                        .map(|c| nscolor_as_nsobject(&color_to_nscolor(&c.resolve()))),
                },
            };
            (run.text.as_str(), attrs)
        })
        .collect();

    let attributed = build_attributed(&assembled);
    let _: () = unsafe {
        msg_send![label, setAttributedStringValue: &*attributed as &NSAttributedString]
    };
}

/// Font for a styled run: run deltas over the paragraph base. Walks a
/// `System` stack via the shared classifier so a CSS-ish
/// `"ui-monospace, SF Mono, Menlo, monospace"` resolves to a real
/// monospace face instead of falling through to the system sans (the
/// whole-stack `fontWithName:` lookup can never match).
fn run_font(
    s: &TextRunStyle,
    base_size: f64,
    base_weight: FontWeight,
    base_style: runtime_shared::FontStyle,
    registry: &FontRegistry,
) -> Retained<NSObject> {
    let size = run_font_size(s, base_size);
    let weight = s.font_weight.unwrap_or(base_weight);
    let style = s.font_style.unwrap_or(base_style);
    let upright = match &s.font_family {
        Some(FontFamily::Typeface(_)) => {
            // A registered typeface resolves its own italic FACE (the
            // registry keys on weight+style), so the descriptor pass
            // below finds the trait already set and is a no-op.
            resolve_nsfont(registry, s.font_family.as_ref(), weight, style, size)
                .unwrap_or_else(|| system_font(weight, size))
        }
        Some(FontFamily::System(stack)) => {
            let mut resolved = None;
            for entry in parse_font_stack(stack) {
                match entry {
                    StackEntry::Monospace => {
                        if let Some(f) = monospaced_system_font(size, weight) {
                            resolved = Some(f);
                        } else {
                            resolved = ns_font_with_name("Menlo", size);
                        }
                    }
                    StackEntry::SansSerif => resolved = Some(system_font(weight, size)),
                    StackEntry::Named(name) => resolved = ns_font_with_name(name, size),
                }
                if resolved.is_some() {
                    break;
                }
            }
            resolved.unwrap_or_else(|| system_font(weight, size))
        }
        None => system_font(weight, size),
    };
    match style {
        // Italic is a derived FACE, not an attribute: ask the resolved
        // font's descriptor for its italic variant. Families with no
        // italic cut return `None` from the descriptor lookup, and we
        // keep the upright face rather than rendering an oblique
        // synthesis AppKit would refuse anyway.
        runtime_shared::FontStyle::Italic => {
            with_italic_trait(&upright, objc2::class!(NSFont)).unwrap_or(upright)
        }
        runtime_shared::FontStyle::Normal => upright,
    }
}

/// `+[NSFont monospacedSystemFontOfSize:weight:]` (10.15+) — SF Mono.
fn monospaced_system_font(size: f64, weight: FontWeight) -> Option<Retained<NSObject>> {
    let w = super::text_style::font_weight_to_nsfont(weight);
    let font: Option<Retained<NSObject>> = unsafe {
        objc2::msg_send_id![
            objc2::class!(NSFont),
            monospacedSystemFontOfSize: size,
            weight: w
        ]
    };
    font
}

/// Re-retain a shared base object for another run's attrs entry
/// (`RunAttrs` owns its objects; several runs may share the base).
fn retain_as_nsobject(obj: &Retained<NSObject>) -> Retained<NSObject> {
    obj.clone()
}

fn nscolor_as_nsobject(color: &Retained<objc2_app_kit::NSColor>) -> Retained<NSObject> {
    unsafe {
        Retained::retain(Retained::as_ptr(color) as *mut NSObject)
            .expect("retain NSColor as NSObject")
    }
}
