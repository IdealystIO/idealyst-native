//! An app-authored `stylesheet!` should name theme tokens *typed*, with no
//! string literal and no restated palette hex.
//!
//! This is the end-to-end proof for the `stylesheet!` → `TokenVocabulary`
//! binding: the block header's `(t)` is bound to the declared vocabulary's
//! namespace, so `t.color.surface()` in a rules block flows through to the
//! resolved rules as `Tokenized::Token { name: "color-surface", .. }` and
//! re-resolves when a theme installs. Every block *kind* is exercised
//! (base, variant arm, interaction state), because each is emitted by a
//! separate closure site in the macro and a missing binding in any one of
//! them is a compile error only that block would catch.
//!
//! The file contains no palette hex on purpose — the fallback must arrive
//! from idea-theme's base palette, which is what makes a reskin the single
//! source of truth.

use idea_ui::{install_idea_theme, light_theme, IdeaThemeRef};

use runtime_core::{resolve_style, stylesheet, StyleApplication, Tokenized, VariantSet};

stylesheet! {
    pub Sidebar<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),
            gap: t.spacing.lg(),
            border_radius: t.radius.md(),
            font_size: t.typography.body_size(),
        }
        variant emphasis {
            #[default]
            normal(t) { background: t.color.surface() }
            accent(t) { background: t.intent.primary.soft_bg() }
        }
        state hovered(t) {
            border_color: t.color.border_hover()
        }
    }
}

/// The STATIC sheet application off a builder (a token reference is a
/// constant `Tokenized`, so the built style must be the static arm).
fn static_app(b: impl runtime_core::IntoStyleSource) -> StyleApplication {
    match b.into_style_prop() {
        runtime_vocabulary::StyleProp::Sheet(app) => *app,
        _ => panic!("a constant token reference must produce a static application"),
    }
}

fn background(app: StyleApplication) -> Tokenized<runtime_core::Color> {
    resolve_style(&app).background.clone().expect("sidebar base sets a background token")
}

#[test]
fn typed_token_references_flow_through_a_stylesheet() {
    idea_theme::testing::with_test_world(|| {
        let base_bg = background(static_app(Sidebar()));
        assert_eq!(base_bg.name(), Some("color-surface"));

        // The fallback arrives from idea-theme's base palette — this file
        // never spells a hex, so the two cannot drift.
        let canonical_surface = light_theme().colors.surface.value().0.clone();
        assert_eq!(
            base_bg.value().0,
            canonical_surface,
            "fallback must equal idea-theme's canonical surface, not a hardcoded hex"
        );

        // Install the theme, then the SAME reference resolves to the
        // installed value — a reskin re-flows this sheet with no edits.
        install_idea_theme(light_theme());
        assert_eq!(
            base_bg.resolve().0,
            canonical_surface,
            "installed theme value resolves through the token reference"
        );
    });
}

/// Lengths ride the same path as colors, and all three length namespaces
/// (spacing / radius / typography) reach the rules.
#[test]
fn typed_length_tokens_reach_the_resolved_rules() {
    idea_theme::testing::with_test_world(|| {
        let rules = resolve_style(&static_app(Sidebar()));
        assert_eq!(rules.gap.as_ref().and_then(|g| g.name()), Some("spacing-lg"));
        assert_eq!(
            rules.border_top_left_radius.as_ref().and_then(|r| r.name()),
            Some("radius-md"),
            "`border_radius` fans out to the four corner fields"
        );
        assert_eq!(
            rules.font_size.as_ref().and_then(|f| f.name()),
            Some("typography-body-size")
        );
    });
}

/// A variant arm binds the vocabulary independently of `base` — it's a
/// separate closure in the emitted sheet.
#[test]
fn variant_arms_bind_the_vocabulary() {
    idea_theme::testing::with_test_world(|| {
        let accent_bg = background(static_app(Sidebar().emphasis(SidebarEmphasis::Accent)));
        assert_eq!(accent_bg.name(), Some("intent-primary-soft-bg"));
    });
}

/// So does a `state` overlay, which the macro stores under the reserved
/// `__state_*` axis and emits from yet another closure site.
///
/// The overlay sets `border_color`, a property no other block in this
/// sheet touches: axes merge in alphabetical axis-name order, so
/// `__state_hovered` resolves BEFORE `emphasis` and a shared property
/// would be overwritten by the variant default — which would test merge
/// precedence rather than the binding this test is about.
#[test]
fn state_overlays_bind_the_vocabulary() {
    idea_theme::testing::with_test_world(|| {
        let sheet = sidebar_style();
        let vs = VariantSet::new().with("__state_hovered", "on");
        let hovered = sheet
            .resolve(&vs)
            .border_top_color
            .clone()
            .expect("the hovered overlay sets a border-color token");
        assert_eq!(hovered.name(), Some("color-border-hover"));
    });
}

// ---------------------------------------------------------------------------
// Backward compatibility: the literal-string form still works
// ---------------------------------------------------------------------------

/// The typed path is an ADDITION, not a replacement. `Tokenized::token`
/// is still the core primitive — runtime-core knows no palette, so a
/// hand-written reference carrying its own fallback has to keep working,
/// and it's the only way to name a token no vocabulary describes (an
/// app's own). This sheet mixes both spellings in one block, and in one
/// sheet declaring a vocabulary, to pin that they coexist.
stylesheet! {
    pub Legacy<IdeaThemeRef> {
        base(t) {
            // typed — vocabulary accessor
            background: t.color.surface(),
            // literal string — the pre-existing form, untouched
            color: Tokenized::token("color-text", runtime_core::Color("#0f172a".into())),
            // a token NO vocabulary describes: an app-defined name. This
            // form is the supported way to reference one.
            gap: Tokenized::token("app-gutter", runtime_core::Length::Px(20.0)),
        }
    }
}

#[test]
fn literal_string_token_references_still_work_alongside_typed_ones() {
    idea_theme::testing::with_test_world(|| {
        let rules = resolve_style(&static_app(Legacy()));

        // Typed and literal produce the same kind of reference.
        assert_eq!(rules.background.as_ref().and_then(|b| b.name()), Some("color-surface"));
        assert_eq!(rules.color.as_ref().and_then(|c| c.name()), Some("color-text"));

        // An app-defined name no vocabulary knows still resolves through
        // the registry once installed — this is the escape hatch working.
        let gap = rules.gap.clone().expect("gap is set");
        assert_eq!(gap.name(), Some("app-gutter"));
        assert_eq!(gap.value(), &runtime_core::Length::Px(20.0), "its own fallback is honoured");

        runtime_core::install_tokens(&[runtime_core::TokenEntry {
            name: "app-gutter",
            value: runtime_core::TokenValue::Length(runtime_core::Length::Px(33.0)),
        }]);
        assert_eq!(
            gap.resolve(),
            runtime_core::Length::Px(33.0),
            "an installed app token wins over the literal fallback, same as a vocabulary token"
        );
    });
}

/// A sheet declaring `<()>` — no vocabulary at all — keeps compiling and
/// keeps resolving string references. This is the shape ~220 sheets in
/// the tree still use, so it is not a legacy path.
stylesheet! {
    pub NoVocab<()> {
        base(_t) {
            background: Tokenized::token("color-surface", runtime_core::Color("#ffffff".into())),
        }
    }
}

#[test]
fn a_sheet_with_no_vocabulary_still_references_tokens_by_name() {
    idea_theme::testing::with_test_world(|| {
        let bg = background(static_app(NoVocab()));
        assert_eq!(bg.name(), Some("color-surface"));
        install_idea_theme(light_theme());
        assert_eq!(bg.resolve().0, light_theme().colors.surface.value().0);
    });
}
