//! Golden-output corpus: the email backend's rendered output must match
//! the **frozen bytes the OLD core produced** for the same logical
//! template, exactly, with **zero normalization**.
//!
//! Model: `backend-ssr/tests/newcore_byte_identity.rs`.
//!
//! # The frozen corpus is the contract
//!
//! Each item compares this crate's render against
//! `tests/goldens/<item>.html` plus `tests/goldens/<item>.txt` (which
//! carries the subject header + the plaintext alternative). Those files
//! were written by the old walker + `Element` render path before it was
//! deleted; a mismatch means a real behavior change, NOT a stale
//! artifact. `IDEALYST_FREEZE_GOLDENS=1` can now only RE-BASELINE against
//! the current renderer, permanently discarding the old core's testimony
//! — see `tests/goldens/README.md`.
//!
//! For the headline item this matters most: `idea_ui_mail_welcome.{html,txt}`
//! is the last artifact in the tree that records what the REAL
//! idea-ui-mail welcome template rendered to under the old core. The
//! builder replica below must stay in lockstep with
//! `crates/ui/idea-ui-mail/src/lib.rs`; if the components change, the
//! replica and the golden move together and the diff is the substance of
//! the change.

use std::rc::Rc;

use backend_email::RenderedEmail;
use runtime_shared::{
    AlignItems, Color, FlexDirection, FontWeight, JustifyContent, Length, StyleApplication,
    StyleRules, StyleSheet, TextAlign, TokenEntry, TokenValue, Tokenized,
};

// ===========================================================================
// Harness
// ===========================================================================

fn render(build: impl FnOnce() -> runtime_scene::Element) -> RenderedEmail {
    backend_email::newcore::render_email(build)
}

// ---------------------------------------------------------------------------
// Frozen-artifact gate
// ---------------------------------------------------------------------------

fn goldens() -> parity_goldens::Goldens {
    parity_goldens::Goldens::new(env!("CARGO_MANIFEST_DIR"))
}

/// The gate: the rendered email must match the frozen old-core bytes
/// exactly (html + subject + plaintext), with no normalization.
fn assert_matches_frozen(name: &str, email: &RenderedEmail) {
    goldens().check_text(&format!("{name}.html"), &email.html);
    goldens().check_text(&format!("{name}.txt"), &plaintext_artifact(email));
}

/// `subject:`-prefixed header line + the plaintext alternative, so one
/// frozen file carries both non-HTML halves of a `RenderedEmail`.
fn plaintext_artifact(email: &RenderedEmail) -> String {
    format!(
        "subject: {}\n---\n{}",
        match &email.subject {
            Some(s) => format!("{s:?}"),
            None => "<none>".to_string(),
        },
        email.text,
    )
}

// ===========================================================================
// Shared fixtures
// ===========================================================================

fn color(s: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(s.to_string()))
}

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

fn test_rules(width: f32, background: &str) -> StyleRules {
    StyleRules {
        width: Some(px(width)),
        background: Some(color(background)),
        ..Default::default()
    }
}

fn surface_token(value: &str) -> TokenEntry {
    TokenEntry {
        name: "color-surface",
        value: TokenValue::Color(Color(value.to_string())),
    }
}

/// A sheet with a token-referencing base and a `state hovered` overlay
/// — email must emit ONLY the resolved base (overlays dropped).
fn hover_sheet() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_vs| StyleRules {
            background: Some(Tokenized::token("color-surface", Color("#000000".into()))),
            width: Some(px(100.0)),
            ..Default::default()
        })
        .variant("__state_hovered", "on", |_vs| StyleRules {
            background: Some(color("#ff00ff")),
            ..Default::default()
        }),
    )
}

// ===========================================================================
// 1. Static styled tree + link + toggle glyphs
// ===========================================================================

#[test]
fn corpus_static_styled_tree() {
    let new = render(|| {
        use runtime_vocabulary::builders::{link, text, toggle, view};
        use runtime_world::signal;
        let on = signal(true);
        view()
            .style(test_rules(120.0, "#112233"))
            .child(text().content("hello"))
            .child(text().content("styled leaf").style(test_rules(80.0, "#445566")))
            .child(
                link()
                    .url("https://example.com")
                    .external(true)
                    .child(text().content("docs")),
            )
            .child(toggle().value(on).on_change(|_| {}))
            .build()
    });
    assert_matches_frozen("static_styled_tree", &new);
    // Sanity: the corpus item is live.
    assert!(new.html.contains("background: #112233"), "styles inline: {}", new.html);
    assert!(new.html.contains(r#"href="https://example.com""#), "link: {}", new.html);
    assert!(new.html.contains('\u{2611}'), "toggle glyph: {}", new.html);
}

// ===========================================================================
// 2. Installed tokens resolve to literals; state overlays dropped
// ===========================================================================

#[test]
fn corpus_tokens_and_dropped_overlays() {
    let new = render(|| {
        use runtime_vocabulary::builders::{text, view};
        runtime_vocabulary::theme::install_tokens(&[surface_token("#101010")]);
        view()
            .child(view().style(StyleApplication::new(hover_sheet())))
            .child(text().content("themed"))
            .build()
    });
    assert_matches_frozen("tokens_and_dropped_overlays", &new);
    assert!(new.html.contains("background: #101010"), "token resolved: {}", new.html);
    assert!(!new.html.contains("#ff00ff"), "hover overlay dropped: {}", new.html);
    assert!(!new.html.contains("var("), "no CSS variables: {}", new.html);
}

// ===========================================================================
// 3. Setup seam: app background via the backend hook
// ===========================================================================

#[test]
fn corpus_setup_app_background() {
    let new = backend_email::newcore::render_email_with(
        |b| {
            runtime_vocabulary::caps::AppEnvOps::set_app_background(
                b,
                &Tokenized::Literal(Color("#0b1020".into())),
            )
        },
        || {
            use runtime_vocabulary::builders::{text, view};
            view().child(text().content("hi")).build()
        },
    );
    assert_matches_frozen("setup_app_background", &new);
    assert!(new.html.contains("background:#0b1020"), "body bg: {}", new.html);
}

// ===========================================================================
// 4. Dyn branch — committed initial state, both branches
// ===========================================================================

#[test]
fn corpus_dyn_branch_both_initial_states() {
    for initial in [true, false] {
        let new = render(move || {
            use runtime_scene::dyn_keyed;
            use runtime_vocabulary::builders::{text, view};
            use runtime_world::signal;
            let show = signal(initial);
            view()
                .child(text().content("before"))
                .child(dyn_keyed(
                    move || show.get(),
                    move |&on| {
                        if on {
                            text().content("shown").build()
                        } else {
                            text().content("hidden").build()
                        }
                    },
                ))
                .child(text().content("after"))
                .build()
        });
        assert_matches_frozen(
            if initial { "dyn_branch_then" } else { "dyn_branch_else" },
            &new,
        );
        let want = if initial { "shown" } else { "hidden" };
        assert!(new.html.contains(want), "committed branch rendered: {}", new.html);
    }
}

// ===========================================================================
// 5. The idea-ui-mail welcome template (the headline golden)
// ===========================================================================
//
// Each component's StyleRules are replicated verbatim from
// `crates/ui/idea-ui-mail/src/lib.rs`. Keep the replica in lockstep with
// the components — the frozen files record what the real components
// rendered to.

fn pad_all(rules: &mut StyleRules, v: f32) {
    rules.padding_top = Some(px(v));
    rules.padding_bottom = Some(px(v));
    rules.padding_left = Some(px(v));
    rules.padding_right = Some(px(v));
}

fn radius_all(rules: &mut StyleRules, v: f32) {
    rules.border_top_left_radius = Some(px(v));
    rules.border_top_right_radius = Some(px(v));
    rules.border_bottom_left_radius = Some(px(v));
    rules.border_bottom_right_radius = Some(px(v));
}

/// idea-ui-mail `EmailBody` (background override).
fn body_rules(background: &str) -> StyleRules {
    let mut rules = StyleRules {
        background: Some(color(background)),
        width: Some(Tokenized::Literal(Length::Percent(100.0))),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        ..Default::default()
    };
    pad_all(&mut rules, 24.0);
    rules
}

/// idea-ui-mail `EmailContainer` (defaults).
fn container_rules() -> StyleRules {
    let mut rules = StyleRules {
        background: Some(color("#ffffff")),
        width: Some(Tokenized::Literal(Length::Percent(100.0))),
        max_width: Some(px(600.0)),
        flex_direction: Some(FlexDirection::Column),
        ..Default::default()
    };
    radius_all(&mut rules, 12.0);
    rules
}

/// idea-ui-mail `Section` (padding override, default gap).
fn section_rules(padding: f32) -> StyleRules {
    let mut rules = StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(px(16.0)),
        ..Default::default()
    };
    pad_all(&mut rules, padding);
    rules
}

/// idea-ui-mail `Heading` (defaults).
fn heading_rules() -> StyleRules {
    StyleRules {
        color: Some(color("#101828")),
        font_size: Some(px(24.0)),
        font_weight: Some(FontWeight::Bold),
        line_height: Some(Tokenized::Literal(24.0 * 1.25)),
        text_align: Some(TextAlign::Left),
        ..Default::default()
    }
}

/// idea-ui-mail `Text` (defaults).
fn body_text_rules() -> StyleRules {
    StyleRules {
        color: Some(color("#475467")),
        font_size: Some(px(16.0)),
        line_height: Some(Tokenized::Literal(16.0 * 1.5)),
        text_align: Some(TextAlign::Left),
        ..Default::default()
    }
}

/// idea-ui-mail `Button` label (defaults).
fn button_label_rules() -> StyleRules {
    StyleRules {
        color: Some(color("#ffffff")),
        font_weight: Some(FontWeight::SemiBold),
        font_size: Some(px(16.0)),
        ..Default::default()
    }
}

/// idea-ui-mail `Button` box (defaults).
fn button_box_rules() -> StyleRules {
    let mut rules = StyleRules {
        background: Some(color("#6d28d9")),
        justify_content: Some(JustifyContent::Center),
        align_items: Some(AlignItems::Center),
        ..Default::default()
    };
    rules.padding_top = Some(px(12.0));
    rules.padding_bottom = Some(px(12.0));
    rules.padding_left = Some(px(20.0));
    rules.padding_right = Some(px(20.0));
    radius_all(&mut rules, 8.0);
    rules
}

/// idea-ui-mail `Divider` (defaults).
fn divider_rules() -> StyleRules {
    StyleRules {
        border_top_width: Some(Tokenized::Literal(1.0)),
        border_top_color: Some(color("#e4e6ef")),
        height: Some(px(0.0)),
        margin_top: Some(px(16.0)),
        margin_bottom: Some(px(16.0)),
        ..Default::default()
    }
}

#[test]
fn golden_idea_ui_mail_welcome_template() {
    let new = render(|| {
        use runtime_vocabulary::builders::{link, text, view};
        view()
            .style(body_rules("#f0f0f5"))
            .child(
                view().style(container_rules()).child(
                    view()
                        .style(section_rules(32.0))
                        .child(text().content("Welcome aboard").style(heading_rules()))
                        .child(
                            text()
                                .content("Thanks for signing up — you're all set.")
                                .style(body_text_rules()),
                        )
                        .child(
                            link()
                                .url("https://app.dev/start")
                                .external(true)
                                .style(button_box_rules())
                                .child(
                                    text().content("Get started").style(button_label_rules()),
                                ),
                        )
                        .child(view().style(divider_rules()))
                        .child(
                            text()
                                .content("Questions? Just reply to this email.")
                                .style(body_text_rules()),
                        ),
                ),
            )
            .build()
    });

    assert_matches_frozen("idea_ui_mail_welcome", &new);
    // Sanity: the template is live and email-safe.
    assert!(new.html.contains("Welcome aboard"), "heading: {}", new.html);
    assert!(new.html.contains(r#"href="https://app.dev/start""#), "cta: {}", new.html);
    assert!(!new.html.contains("class="), "no classes in email: {}", new.html);
    assert!(!new.html.contains("var("), "no CSS variables in email: {}", new.html);
    assert!(new.text.contains("Welcome aboard"), "plaintext: {:?}", new.text);
}

// ===========================================================================
// 6. The anchoring contract the goldens depend on
// ===========================================================================

/// `Host::supports_splice` used to be inherited from a `Backend` trait
/// DEFAULT (`false`); `Host` makes it required, so the value is now an
/// explicit body in `newcore.rs`. Every frozen artifact in
/// `tests/goldens/` was recorded in ANCHORED mode — flipping this to
/// `true` would move reactive regions out from under their anchor
/// `<div>`s and change the emitted HTML wholesale.
#[test]
fn newcore_host_is_anchored() {
    use runtime_scene::Host;
    let b = backend_email::EmailBackend::new();
    assert!(
        !Host::supports_splice(&b),
        "email must render ANCHORED (frozen goldens pin the anchor <div>s)"
    );
}
