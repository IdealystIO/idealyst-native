//! Field report: a custom app `stylesheet!` should track theme tokens by
//! name without restating a fallback hex. `theme_token!` / `theme_length!`
//! provide that — but they expand to a braced block (`{ const _…; expr }`)
//! for the compile-time name check, so this test's real job is to prove
//! that shape *parses as a `stylesheet!` field value* (a pure compile check)
//! AND that the referenced token flows through to the resolved rules with
//! idea-theme's canonical value as its fallback, no hex written here.

use idea_ui::{install_idea_theme, light_theme, theme_length, theme_token};
use runtime_core::{resolve_style, stylesheet, IntoStyleSource, StyleSource, Tokenized};

// A custom, app-authored sheet — the "custom sidebar" from the report.
// Every theme-dependent value is a token reference by name; the file
// contains no palette hex at all. `theme_token!` appears in both the base
// block and a variant block to exercise both parse positions.
stylesheet! {
    pub Sidebar<()> {
        base(_t) {
            background: theme_token!("color-surface"),
            gap: theme_length!("spacing-lg"),
        }
        variant emphasis {
            #[default]
            normal(_t) { background: theme_token!("color-surface") }
            accent(_t) { background: theme_token!("intent-primary-soft-bg") }
        }
    }
}

/// Resolve the constant (non-reactive) background token off a builder — the
/// token reference is a constant `Tokenized`, so the sheet is `Static`.
fn background(src: StyleSource) -> Tokenized<runtime_core::Color> {
    match src {
        StyleSource::Static(app) => resolve_style(&app)
            .background
            .clone()
            .expect("sidebar base sets a background token"),
        _ => panic!("a constant token reference must produce StyleSource::Static"),
    }
}

#[test]
fn theme_token_references_flow_through_a_stylesheet() {
    // Base: references `color-surface` by name. Its fallback must be
    // idea-theme's canonical base value — restated nowhere in this file.
    let base_bg = background(Sidebar().into_style_source());
    assert_eq!(base_bg.name(), Some("color-surface"));
    let canonical_surface = light_theme().colors.surface.value().0.clone();
    assert_eq!(
        base_bg.value().0, canonical_surface,
        "fallback must equal idea-theme's canonical surface, not a hardcoded hex"
    );

    // Install the theme, then the SAME by-name reference resolves to the
    // installed value — a reskin re-flows this sheet with no edits to it.
    install_idea_theme(light_theme());
    assert_eq!(
        base_bg.resolve().0, canonical_surface,
        "installed theme value resolves through the token reference"
    );

    // The accent variant references a different canonical token.
    let accent_bg = background(Sidebar().emphasis(SidebarEmphasis::Accent).into_style_source());
    assert_eq!(accent_bg.name(), Some("intent-primary-soft-bg"));
}
