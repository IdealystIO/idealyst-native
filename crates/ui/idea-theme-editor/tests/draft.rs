//! `ThemeDraft` against a real installed theme — the half that can't be
//! unit-tested inside the crate because it needs a world with tokens in
//! it.

use idea_theme::testing::{commit as flush_signals, with_test_world};
use idea_theme::{install_idea_theme, light_theme, TokenEntry, TokenValue};
use idea_theme_editor::{DraftKind, ParseError, ThemeDraft, EXTENSION_NAMESPACE};
use runtime_core::{token_value, Color, Length};

/// Install the base palette and run `f` inside a world that has it.
fn with_theme<R>(f: impl FnOnce() -> R) -> R {
    with_test_world(|| {
        install_idea_theme(light_theme());
        f()
    })
}

#[test]
fn draft_covers_every_installed_token() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        // The base palette installs 74 tokens; the draft is one control
        // per token, so a token that stopped being editable shows up
        // here rather than as a missing row someone notices later.
        let live = runtime_core::token_names().len();
        assert_eq!(draft.entries().len(), live, "one entry per live token");

        for name in runtime_core::token_names() {
            assert!(draft.find(name).is_some(), "`{name}` is installed but has no control");
        }
    });
}

/// Controls open showing what the app is actually painting, not the
/// vocabulary's defaults — the distinction that matters the moment an
/// app installs anything other than the stock palette.
#[test]
fn entries_seed_from_the_live_value_not_the_base_palette() {
    with_theme(|| {
        runtime_core::update_tokens(&[TokenEntry {
            name: "color-surface",
            value: TokenValue::Color(Color("#123456".into())),
        }]);
        let draft = ThemeDraft::from_live();
        assert_eq!(draft.find("color-surface").unwrap().text.get(), "#123456");
    });
}

#[test]
fn namespaces_group_the_vocabulary_in_layout_order() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        assert_eq!(
            draft.namespaces(),
            vec!["color", "intent", "spacing", "radius", "typography"],
            "sections follow declaration order, so the panel doesn't reshuffle"
        );
    });
}

#[test]
fn commit_writes_through_to_the_live_theme() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        draft.find("color-table-header").unwrap().text.set("#e8edf5".into());
        flush_signals();
        assert_eq!(draft.commit("color-table-header"), Ok(()));
        assert_eq!(
            token_value("color-table-header"),
            Some(TokenValue::Color(Color("#e8edf5".into()))),
        );
    });
}

/// A half-typed value must not reach the app. The editor commits on
/// every keystroke, so "12p" on the way to "12px" happens on literally
/// every length edit — repainting with it would flash the app through
/// garbage.
#[test]
fn commit_rejects_unparseable_text_and_leaves_the_theme_alone() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        let before = token_value("spacing-md");
        draft.find("spacing-md").unwrap().text.set("12p".into());
        flush_signals();

        assert_eq!(draft.commit("spacing-md"), Err(ParseError::BadLength("12p".into())));
        assert_eq!(token_value("spacing-md"), before, "the theme is untouched");
    });
}

#[test]
fn commit_of_an_unknown_token_reports_rather_than_panics() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        assert_eq!(
            draft.commit("color-nope"),
            Err(ParseError::UnknownToken("color-nope".into())),
        );
    });
}

#[test]
fn commit_all_reports_the_failures_and_applies_the_rest() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        draft.find("color-surface").unwrap().text.set("#abcdef".into());
        flush_signals();
        draft.find("spacing-md").unwrap().text.set("nonsense".into());
        flush_signals();

        let failures = draft.commit_all();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, "spacing-md");
        assert_eq!(
            token_value("color-surface"),
            Some(TokenValue::Color(Color("#abcdef".into()))),
            "the parseable edits still land",
        );
    });
}

#[test]
fn json_round_trips_through_a_fresh_draft() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        draft.find("color-surface").unwrap().text.set("#101010".into());
        flush_signals();
        draft.find("spacing-md").unwrap().text.set("18px".into());
        flush_signals();
        let saved = draft.to_json();

        // A second draft, opened after reverting to the base palette,
        // must land exactly where the first one was.
        draft.revert();
        flush_signals();
        assert_eq!(token_value("color-surface"), Some(TokenValue::Color(Color("#ffffff".into()))));

        let applied = draft.load_json(&saved).expect("the file this crate wrote must load");
        flush_signals();
        assert_eq!(applied, draft.entries().len(), "every token in the file applied");
        assert_eq!(token_value("color-surface"), Some(TokenValue::Color(Color("#101010".into()))));
        assert_eq!(token_value("spacing-md"), Some(TokenValue::Length(Length::Px(18.0))));
        assert_eq!(draft.find("spacing-md").unwrap().text.get(), "18px", "controls follow");
    });
}

/// `radius-pill` is `Length::Full`. A save file that wrote it as a
/// number would reload the pill as a fixed radius — the exact bug the
/// token was changed to `Full` to kill.
#[test]
fn json_round_trips_the_pill_as_full() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        assert_eq!(draft.find("radius-pill").unwrap().text.get(), "full");
        let saved = draft.to_json();
        assert!(saved.contains("\"radius-pill\": \"full\""), "{saved}");

        draft.load_json(&saved).unwrap();
        flush_signals();
        assert_eq!(token_value("radius-pill"), Some(TokenValue::Length(Length::Full)));
    });
}

/// A load is all-or-nothing. A file with one bad value must not leave
/// the app half-themed — a state no one chose and nothing can undo.
#[test]
fn a_bad_value_aborts_the_whole_load() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        let before = token_value("color-surface");

        let report = draft
            .load_json(r##"{"color-surface": "#111111", "spacing-md": "banana"}"##)
            .expect_err("a bad value must fail the load");
        assert_eq!(report.applied, 0);
        assert_eq!(report.invalid.len(), 1);
        assert_eq!(report.invalid[0].0, "spacing-md");
        assert_eq!(token_value("color-surface"), before, "nothing was applied");
        assert_eq!(
            draft.find("color-surface").unwrap().text.get(),
            "#ffffff",
            "and no control was moved",
        );
    });
}

/// Loading a file saved from a different theme names what it couldn't
/// place, rather than silently dropping it. This is the most likely
/// real load failure, so it gets the actionable error.
#[test]
fn a_file_from_another_theme_names_the_tokens_it_could_not_place() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        let report = draft
            .load_json(r##"{"color-surface": "#111111", "color-from-elsewhere": "#222"}"##)
            .expect_err("an unknown token must fail the load");
        assert_eq!(report.unknown, vec!["color-from-elsewhere".to_string()]);
        assert!(format!("{report}").contains("color-from-elsewhere"));
    });
}

#[test]
fn malformed_json_fails_before_touching_anything() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        let before = token_value("color-surface");
        let report = draft.load_json("not json").expect_err("must fail");
        assert!(report.error.is_some());
        assert_eq!(token_value("color-surface"), before);
    });
}

#[test]
fn revert_restores_the_values_the_draft_opened_with() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        draft.find("color-surface").unwrap().text.set("#000000".into());
        flush_signals();
        draft.commit("color-surface").unwrap();

        draft.revert();
        flush_signals();
        assert_eq!(draft.find("color-surface").unwrap().text.get(), "#ffffff");
        assert_eq!(token_value("color-surface"), Some(TokenValue::Color(Color("#ffffff".into()))));
    });
}

/// Regression: the panel commits on EVERY keystroke, and a signal write
/// only stages until the world flushes. Committing by reading the
/// signal therefore applied the PREVIOUS keystroke — the theme trailed
/// the input by one character, forever, on every edit. `commit_text`
/// takes the text the handler already holds.
///
/// Both halves are asserted: that `commit_text` lands without a flush,
/// and that `commit` genuinely does read stale text — which is the
/// reason the second method exists and the thing that would silently
/// come back if someone folded them together.
#[test]
fn regression_keystroke_commit_does_not_lag_by_one_edit() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        let entry = draft.find("color-surface").unwrap();

        // Exactly what the row's `on_change` does — no flush between.
        entry.text.set("#abcabc".into());
        draft.commit_text("color-surface", "#abcabc").unwrap();
        assert_eq!(
            token_value("color-surface"),
            Some(TokenValue::Color(Color("#abcabc".into()))),
            "commit_text applies this keystroke, not the last one",
        );

        // The shape that was wrong, and it is worse than a one-edit lag:
        // `commit` reads the last FLUSHED text, which after two
        // un-flushed keystrokes is still the value the draft opened
        // with — so a keystroke handler built on `commit` would keep
        // shoving the theme back to where it started.
        entry.text.set("#defdef".into());
        draft.commit("color-surface").unwrap();
        assert_eq!(
            token_value("color-surface"),
            Some(TokenValue::Color(Color("#ffffff".into()))),
            "commit reads FLUSHED text — this is why the input path uses commit_text",
        );

        // After a flush it catches up, which is the contract `commit`
        // documents for button-press callers.
        flush_signals();
        draft.commit("color-surface").unwrap();
        assert_eq!(
            token_value("color-surface"),
            Some(TokenValue::Color(Color("#defdef".into()))),
        );
    });
}

// ---------------------------------------------------------------------------
// Codegen
// ---------------------------------------------------------------------------

#[test]
fn to_rust_is_none_until_something_changes() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        assert_eq!(draft.to_rust(), None, "an untouched draft has nothing to export");
    });
}

#[test]
fn to_rust_emits_only_the_edits() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        draft.find("color-table-header").unwrap().text.set("#e8edf5".into());
        flush_signals();
        draft.find("spacing-md").unwrap().text.set("14px".into());
        flush_signals();

        let src = draft.to_rust().expect("edits export");
        assert!(
            src.contains(r##"theme.colors.table_header = Tokenized::Literal(Color("#e8edf5".into()));"##),
            "{src}"
        );
        // `14.0`, never `14` — an integer literal is not an `f32`, and
        // the type error would land in the pasting crate.
        assert!(src.contains("theme.spacing.md = 14.0;"), "{src}");
        // Untouched tokens stay out: the export is a diff, not a
        // restatement of the whole palette.
        assert!(!src.contains("color-surface"), "{src}");
        assert!(!src.contains("intent"), "{src}");
    });
}

#[test]
fn to_rust_uses_the_intent_field_path() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        draft.find("intent-primary-solid-bg").unwrap().text.set("#0066ff".into());
        flush_signals();
        let src = draft.to_rust().unwrap();
        assert!(src.contains("theme.intents.primary.solid_bg = "), "{src}");
    });
}

/// A token with no theme field can't be an assignment. It has to reach
/// the `update_tokens` block instead of being dropped — dropping it
/// would produce source that silently loses an edit the user made.
#[test]
fn to_rust_routes_fieldless_tokens_to_update_tokens() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        draft.find("radius-pill").unwrap().text.set("50%".into());
        flush_signals();
        let src = draft.to_rust().unwrap();
        assert!(src.contains("update_tokens(&["), "{src}");
        assert!(src.contains(r#"name: "radius-pill""#), "{src}");
        assert!(src.contains("TokenValue::Length(Length::Percent(50.0))"), "{src}");
        assert!(!src.contains("theme.radius.pill"), "there is no such field: {src}");
    });
}

/// A `%` or `auto` length has no number to assign to an `f32` field, so
/// it routes to `update_tokens` too. Generating
/// `theme.spacing.md = 50%;` would not compile.
#[test]
fn to_rust_routes_non_px_lengths_away_from_f32_fields() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        draft.find("spacing-md").unwrap().text.set("50%".into());
        flush_signals();
        let src = draft.to_rust().unwrap();
        assert!(!src.contains("theme.spacing.md ="), "a % is not an f32: {src}");
        assert!(src.contains(r#"name: "spacing-md""#), "{src}");
    });
}

/// Unparseable text is not an edit. Exporting it would generate source
/// carrying a value the app itself refused.
#[test]
fn to_rust_skips_rows_that_do_not_parse() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        draft.find("spacing-md").unwrap().text.set("banana".into());
        flush_signals();
        assert_eq!(draft.to_rust(), None);
    });
}

// ---------------------------------------------------------------------------
// Extension tokens
// ---------------------------------------------------------------------------

/// An extension token (`tone!`'s `tokens = [...]`) has no accessor, so
/// no descriptor describes it — but it IS in the live world, and the
/// editor enumerates the world. Without this it would be the one part
/// of a themed app the theme editor couldn't touch.
#[test]
fn extension_tokens_are_editable_and_grouped_apart() {
    with_theme(|| {
        runtime_core::update_tokens(&[TokenEntry {
            name: "tone-hype-fill-bg",
            value: TokenValue::Color(Color("#ff00ff".into())),
        }]);

        let draft = ThemeDraft::from_live();
        let entry = draft.find("tone-hype-fill-bg").expect("extension token is editable");
        assert_eq!(entry.namespace, EXTENSION_NAMESPACE);
        assert_eq!(entry.kind, DraftKind::Color, "kind comes from the live value");
        assert_eq!(entry.field_path, None, "no accessor, so no field to assign");
        assert!(draft.namespaces().contains(&EXTENSION_NAMESPACE));

        entry.text.set("#00ff00".into());
        flush_signals();
        draft.commit("tone-hype-fill-bg").unwrap();
        assert_eq!(
            token_value("tone-hype-fill-bg"),
            Some(TokenValue::Color(Color("#00ff00".into()))),
        );

        // And it exports — through the fieldless path, since there is
        // no `theme.…` place to assign.
        let src = draft.to_rust().unwrap();
        assert!(src.contains(r#"name: "tone-hype-fill-bg""#), "{src}");
    });
}

#[test]
fn extension_tokens_survive_a_json_round_trip() {
    with_theme(|| {
        runtime_core::update_tokens(&[TokenEntry {
            name: "tone-hype-fill-bg",
            value: TokenValue::Color(Color("#ff00ff".into())),
        }]);
        let draft = ThemeDraft::from_live();
        let saved = draft.to_json();
        assert!(saved.contains("tone-hype-fill-bg"), "{saved}");

        draft.find("tone-hype-fill-bg").unwrap().text.set("#123123".into());
        flush_signals();
        draft.commit_all();
        draft.load_json(&saved).unwrap();
        flush_signals();
        assert_eq!(
            token_value("tone-hype-fill-bg"),
            Some(TokenValue::Color(Color("#ff00ff".into()))),
        );
    });
}

#[test]
fn is_changed_tracks_the_control_against_its_starting_value() {
    with_theme(|| {
        let draft = ThemeDraft::from_live();
        let entry = draft.find("color-surface").unwrap();
        assert!(!entry.is_changed());

        entry.text.set("#000000".into());
        flush_signals();
        assert!(entry.is_changed());

        entry.text.set("#ffffff".into());
        flush_signals();
        assert!(!entry.is_changed(), "back to the opening value reads as unchanged");

        // Unparseable text is certainly not the initial value.
        entry.text.set("".into());
        flush_signals();
        assert!(entry.is_changed());
    });
}
