//! Golden-output corpus: the SAME logical email template rendered
//! through the old-core path ([`backend_email::render_email`], walker +
//! `Element`) and the new-core path
//! ([`backend_email::newcore::render_email`], one-shot `World` +
//! vocabulary handlers) must emit **byte-identical** `html`, `text`,
//! and `subject`.
//!
//! Model: `backend-ssr/tests/newcore_byte_identity.rs`. Like there, the
//! two sides are twin-authored (old-core builders vs vocabulary
//! builders) because `ui!`'s lowering is a build-graph-wide switch —
//! one binary cannot compile the same component body for both cores.
//! The headline item renders the REAL idea-ui-mail welcome template on
//! the old side (the components emit old-core `Element` in this build)
//! against a pinned vocabulary-builder replica on the new side; a
//! divergence therefore means the cores disagree, not that the replica
//! is "allowed to drift" — update the replica in lockstep with
//! idea-ui-mail when the template itself changes.

#![cfg(feature = "new-core")]

use std::rc::Rc;

use backend_email::RenderedEmail;
use runtime_core::{
    AlignItems, Color, FlexDirection, FontWeight, JustifyContent, Length, StyleApplication,
    StyleRules, StyleSheet, TextAlign, TokenEntry, TokenValue, Tokenized,
};

// ===========================================================================
// Harness
// ===========================================================================

fn render_old(app: impl FnOnce() -> runtime_core::Element) -> RenderedEmail {
    backend_email::render_email(app)
}

fn render_new(build: impl FnOnce() -> runtime_scene::Element) -> RenderedEmail {
    backend_email::newcore::render_email(build)
}

/// Assert exact string equality with a byte-level first-divergence
/// report (same shape as the SSR corpus's helper).
fn assert_bytes(name: &str, part: &str, old: &str, new: &str) {
    if old == new {
        return;
    }
    let at = old
        .bytes()
        .zip(new.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| old.len().min(new.len()));
    let lo = at.saturating_sub(60);
    let old_hi = (at + 60).min(old.len());
    let new_hi = (at + 60).min(new.len());
    panic!(
        "byte divergence in corpus item `{name}` ({part}) at byte {at}\n\
         old (len {}): …{}…\n\
         new (len {}): …{}…\n\
         full old:\n{}\n\
         full new:\n{}",
        old.len(),
        &old[lo..old_hi],
        new.len(),
        &new[lo..new_hi],
        old,
        new,
    );
}

fn assert_emails_identical(name: &str, old: &RenderedEmail, new: &RenderedEmail) {
    assert_bytes(name, "html", &old.html, &new.html);
    assert_bytes(name, "text", &old.text, &new.text);
    assert_eq!(old.subject, new.subject, "corpus item `{name}` (subject)");
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

/// Old-core route to a resolved-rules apply: literal rules in a static
/// sheet resolve to themselves, matching the new side's static
/// `.style(rules)` (the same equivalence the SSR corpus relies on).
fn static_style(rules: StyleRules) -> StyleApplication {
    StyleApplication::new(Rc::new(StyleSheet::r#static(rules)))
}

fn surface_token(value: &str) -> TokenEntry {
    TokenEntry {
        name: "color-surface",
        value: TokenValue::Color(Color(value.to_string())),
    }
}

/// A sheet with a token-referencing base and a `state hovered` overlay
/// — email must emit ONLY the resolved base (overlays dropped), on both
/// cores.
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
    let old = render_old(|| {
        use runtime_core::{external_link, signal, text, toggle, view, IntoElement};
        let on = signal(true);
        view(vec![
            text("hello").into_element(),
            text("styled leaf")
                .with_style(static_style(test_rules(80.0, "#445566")))
                .into_element(),
            external_link("https://example.com", vec![text("docs").into_element()])
                .into_element(),
            toggle(on, |_| {}).into_element(),
        ])
        .with_style(static_style(test_rules(120.0, "#112233")))
        .into_element()
    });
    let new = render_new(|| {
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
    assert_emails_identical("static_styled_tree", &old, &new);
    // Sanity: the corpus item is live.
    assert!(old.html.contains("background: #112233"), "styles inline: {}", old.html);
    assert!(old.html.contains(r#"href="https://example.com""#), "link: {}", old.html);
    assert!(old.html.contains('\u{2611}'), "toggle glyph: {}", old.html);
}

// ===========================================================================
// 2. Installed tokens resolve to literals; state overlays dropped
// ===========================================================================

#[test]
fn corpus_tokens_and_dropped_overlays() {
    let old = render_old(|| {
        use runtime_core::{text, view, IntoElement};
        runtime_core::install_tokens(&[surface_token("#101010")]);
        view(vec![
            view(vec![])
                .with_style(StyleApplication::new(hover_sheet()))
                .into_element(),
            text("themed").into_element(),
        ])
        .into_element()
    });
    let new = render_new(|| {
        use runtime_vocabulary::builders::{text, view};
        runtime_vocabulary::theme::install_tokens(&[surface_token("#101010")]);
        view()
            .child(view().style(StyleApplication::new(hover_sheet())))
            .child(text().content("themed"))
            .build()
    });
    assert_emails_identical("tokens_and_dropped_overlays", &old, &new);
    assert!(old.html.contains("background: #101010"), "token resolved: {}", old.html);
    assert!(!old.html.contains("#ff00ff"), "hover overlay dropped: {}", old.html);
    assert!(!old.html.contains("var("), "no CSS variables: {}", old.html);
}

// ===========================================================================
// 3. Setup seam: app background via the backend hook (same-shape entry)
// ===========================================================================

#[test]
fn corpus_setup_app_background() {
    let old = backend_email::render_email_with(
        |b| {
            runtime_core::Backend::set_app_background(
                b,
                &Tokenized::Literal(Color("#0b1020".into())),
            )
        },
        || {
            use runtime_core::{text, view, IntoElement};
            view(vec![text("hi").into_element()]).into_element()
        },
    );
    let new = backend_email::newcore::render_email_with(
        |b| {
            runtime_core::Backend::set_app_background(
                b,
                &Tokenized::Literal(Color("#0b1020".into())),
            )
        },
        || {
            use runtime_vocabulary::builders::{text, view};
            view().child(text().content("hi")).build()
        },
    );
    assert_emails_identical("setup_app_background", &old, &new);
    assert!(old.html.contains("background:#0b1020"), "body bg: {}", old.html);
}

// ===========================================================================
// 4. Dyn branch — committed initial state, both branches
// ===========================================================================

#[test]
fn corpus_dyn_branch_both_initial_states() {
    for initial in [true, false] {
        let old = render_old(move || {
            use runtime_core::{signal, text, view, when, IntoElement};
            let show = signal(initial);
            view(vec![
                text("before").into_element(),
                when(
                    move || show.get(),
                    || text("shown").into_element(),
                    || text("hidden").into_element(),
                ),
                text("after").into_element(),
            ])
            .into_element()
        });
        let new = render_new(move || {
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
        assert_emails_identical(
            if initial { "dyn_branch_then" } else { "dyn_branch_else" },
            &old,
            &new,
        );
        let want = if initial { "shown" } else { "hidden" };
        assert!(old.html.contains(want), "committed branch rendered: {}", old.html);
    }
}

// ===========================================================================
// 5. The idea-ui-mail welcome template (the headline golden)
// ===========================================================================
//
// Old side: the REAL idea-ui-mail components (this build compiles them
// on the old-core `ui!` lowering). New side: the same template authored
// against the vocabulary builders, with each component's StyleRules
// replicated verbatim from `crates/ui/idea-ui-mail/src/lib.rs`. Keep
// the replica in lockstep with the components.

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
    // Old core: the real components.
    let old = render_old(|| {
        use idea_ui_mail::{Button, Divider, EmailBody, EmailContainer, Heading, Section, Text};
        use runtime_core::ui;
        ui! {
            EmailBody(background = "#f0f0f5") {
                EmailContainer() {
                    Section(padding = 32.0) {
                        Heading(content = "Welcome aboard")
                        Text(content = "Thanks for signing up — you're all set.")
                        Button(label = "Get started", href = "https://app.dev/start")
                        Divider()
                        Text(content = "Questions? Just reply to this email.")
                    }
                }
            }
        }
    });

    // New core: the same template, vocabulary-builder replica.
    let new = render_new(|| {
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

    assert_emails_identical("idea_ui_mail_welcome", &old, &new);
    // Sanity: the template is live and email-safe on both sides.
    assert!(old.html.contains("Welcome aboard"), "heading: {}", old.html);
    assert!(old.html.contains(r#"href="https://app.dev/start""#), "cta: {}", old.html);
    assert!(!old.html.contains("class="), "no classes in email: {}", old.html);
    assert!(!old.html.contains("var("), "no CSS variables in email: {}", old.html);
    assert!(old.text.contains("Welcome aboard"), "plaintext: {:?}", old.text);
}
