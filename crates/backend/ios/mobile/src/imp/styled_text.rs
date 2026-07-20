//! Styled text runs on UIKit — realize a `TextRun` list as ONE
//! `NSAttributedString` on the label's `attributedText`, so a
//! mixed-style paragraph (prose + inline-code chips) wraps as a
//! single unit through UIKit's own text engine.
//!
//! Every run is FULLY attributed (base paragraph font/color merged
//! with the run's deltas). UILabel does merge its own properties into
//! unattributed ranges, but NSTextField (macOS) does not — full
//! attribution keeps the two Apple realizations byte-identical in
//! shape and makes the output independent of either toolkit's merge
//! rules (CLAUDE.md §7). It also sidesteps the classic UIKit trap:
//! `label.font = …` AFTER `attributedText` stomps per-range fonts —
//! with full attribution, `apply_style` just re-realizes afterwards
//! (see the Label arm in `apply_style`).
//!
//! Theme swaps re-realize through the walker's cohort entry
//! (`Backend::update_styled_text`); run token colors resolve at
//! realize time. Chip colors SNAP on a theme swap while plain text
//! may fade through the transition system — accepted
//! divergence-in-time (final state identical), matching macOS.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_foundation::{NSAttributedString, NSObject};
use objc2_ui_kit::UILabel;
use runtime_core::{FontFamily, FontWeight, StyleRules, TextRun, TextRunStyle};

use backend_apple_core::font::FontRegistry;
use backend_apple_core::styled_text::{
    build_attributed, parse_font_stack, run_font_size, run_needs_font, RunAttrs, StackEntry,
};
use backend_ios_core::font::{resolve_uifont, system_font, ui_font_with_name};
use backend_ios_core::style::color_to_uicolor;
use runtime_core::effective_text_color;

/// Per-label styled-text state, keyed by view pointer in
/// `IosBackend::styled_texts`. `para` is the last paragraph style
/// `apply_style` delivered — the base the run deltas layer over.
pub(crate) struct StyledTextEntry {
    pub(crate) runs: Vec<TextRun>,
    pub(crate) para: Option<std::rc::Rc<StyleRules>>,
}

/// Build and install the attributed string for `runs` over the
/// paragraph style `para` (`None` = pre-style defaults: system font
/// at UIKit's 17pt label size, the installed theme's `color-text`).
pub(crate) fn realize(
    label: &UILabel,
    runs: &[TextRun],
    para: Option<&StyleRules>,
    registry: &FontRegistry,
) {
    // Base font — mirror `apply_text_style`'s resolution (17.0 is
    // UIKit's label default and that fn's own fallback size).
    let base_size = para
        .and_then(|s| s.font_size.as_ref().map(|t| t.resolve()))
        .map(|len| {
            let px = backend_ios_core::style::length_to_px(&len);
            if px > 0.0 { px } else { 17.0 }
        })
        .unwrap_or(17.0);
    let base_weight = para
        .and_then(|s| s.font_weight)
        .unwrap_or(FontWeight::Normal);
    let base_style = para
        .and_then(|s| s.font_style)
        .unwrap_or(runtime_core::FontStyle::Normal);
    let base_font = para
        .and_then(|s| {
            resolve_uifont(registry, s.font_family.as_ref(), base_weight, base_style, base_size)
        })
        .unwrap_or_else(|| system_font(base_weight, base_size));
    // Base color — the same theme-aware default a plain label gets
    // (`effective_text_color`: explicit para color wins, else the
    // installed theme's `color-text`).
    let base_color = effective_text_color(para.and_then(|s| s.color.as_ref())).resolve();
    let base_uicolor = color_to_uicolor(&base_color);

    let assembled: Vec<(&str, RunAttrs)> = runs
        .iter()
        .map(|run| {
            let attrs = match &run.style {
                None => RunAttrs {
                    font: Some(base_font.clone()),
                    foreground: Some(uicolor_as_nsobject(&base_uicolor)),
                    background: None,
                },
                Some(s) => RunAttrs {
                    font: Some(if run_needs_font(s) {
                        run_font(s, base_size, base_weight, registry)
                    } else {
                        base_font.clone()
                    }),
                    foreground: Some(match &s.color {
                        Some(c) => uicolor_as_nsobject(&color_to_uicolor(&c.resolve())),
                        None => uicolor_as_nsobject(&base_uicolor),
                    }),
                    background: s
                        .background
                        .as_ref()
                        .map(|b| uicolor_as_nsobject(&color_to_uicolor(&b.resolve()))),
                },
            };
            (run.text.as_str(), attrs)
        })
        .collect();

    let attributed = build_attributed(&assembled);
    let _: () =
        unsafe { msg_send![label, setAttributedText: &*attributed as &NSAttributedString] };
}

/// Font for a styled run: run deltas over the paragraph base. Walks a
/// `System` stack via the shared classifier so a CSS-ish
/// `"ui-monospace, SFMono-Regular, Menlo, monospace"` resolves to a
/// real monospace face.
fn run_font(
    s: &TextRunStyle,
    base_size: f64,
    base_weight: FontWeight,
    registry: &FontRegistry,
) -> Retained<NSObject> {
    let size = run_font_size(s, base_size);
    let weight = s.font_weight.unwrap_or(base_weight);
    match &s.font_family {
        Some(FontFamily::Typeface(_)) => resolve_uifont(
            registry,
            s.font_family.as_ref(),
            weight,
            runtime_core::FontStyle::Normal,
            size,
        )
        .unwrap_or_else(|| system_font(weight, size)),
        Some(FontFamily::System(stack)) => {
            for entry in parse_font_stack(stack) {
                match entry {
                    StackEntry::Monospace => {
                        if let Some(f) = monospaced_system_font(size, weight) {
                            return f;
                        }
                        if let Some(f) = ui_font_with_name("Menlo", size) {
                            return f;
                        }
                    }
                    StackEntry::SansSerif => return system_font(weight, size),
                    StackEntry::Named(name) => {
                        if let Some(f) = ui_font_with_name(name, size) {
                            return f;
                        }
                    }
                }
            }
            system_font(weight, size)
        }
        None => system_font(weight, size),
    }
}

/// `+[UIFont monospacedSystemFontOfSize:weight:]` (iOS 13+) — SF Mono.
fn monospaced_system_font(size: f64, weight: FontWeight) -> Option<Retained<NSObject>> {
    let w = backend_ios_core::style::font_weight_to_uikit(weight);
    let font: Option<Retained<NSObject>> = unsafe {
        objc2::msg_send_id![
            objc2::class!(UIFont),
            monospacedSystemFontOfSize: size,
            weight: w
        ]
    };
    font
}

fn uicolor_as_nsobject(color: &Retained<objc2_ui_kit::UIColor>) -> Retained<NSObject> {
    unsafe {
        Retained::retain(Retained::as_ptr(color) as *mut NSObject)
            .expect("retain UIColor as NSObject")
    }
}
