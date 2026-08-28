//! `ThemeEditor` render coverage — the panel mounts against a real
//! theme and draws a labelled row per token.
//!
//! Mounts on `host_mock::Harness` (the recording scene `Host` +
//! capability mock) and greps the recorded op log, the same shape
//! `idea-ui-nav`'s render tests use.

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_theme_editor::{ThemeDraft, ThemeEditor};

/// Mount a panel over the base palette and return a text dump of
/// everything rendered, plus the draft it was built from.
fn rendered() -> String {
    let h = host_mock::Harness::new();
    let root = h.world.enter(|| {
        // The theme installs per WORLD, so it runs inside the mount's
        // `enter` — and `from_live` must run after it, since it reads
        // the token table the install fills.
        install_idea_theme(light_theme());
        let draft = ThemeDraft::from_live();
        ThemeEditor(draft)
    });
    let _realized = h.mount(root);
    h.flush();
    h.take_log().join("\n")
}

/// Every namespace gets a section, and tokens from each end up on the
/// page. A panel that mounts but renders nothing would pass a model
/// test and fail a user.
#[test]
fn panel_renders_a_section_per_namespace() {
    let dump = rendered();
    for heading in ["color", "intent", "spacing", "radius", "typography"] {
        assert!(dump.contains(heading), "no `{heading}` section in the panel:\n{dump}");
    }
}

/// Rows are labelled with the token NAME, which is the thing a user
/// searches the panel for and the key a save file is written under.
#[test]
fn rows_are_labelled_with_their_token_name() {
    let dump = rendered();
    for token in [
        "color-surface",
        "color-table-header",
        "intent-primary-solid-bg",
        "spacing-md",
        "radius-pill",
        "typography-body-size",
    ] {
        assert!(dump.contains(token), "no row for `{token}`:\n{dump}");
    }
}

/// The panel covers the whole palette — one row per live token. Pinned
/// as a count so a token that silently stops rendering (a namespace
/// dropped from the layout, say) fails here rather than being noticed
/// as a missing row much later.
#[test]
fn panel_renders_a_row_for_every_token() {
    let h = host_mock::Harness::new();
    let (root, expected) = h.world.enter(|| {
        install_idea_theme(light_theme());
        let draft = ThemeDraft::from_live();
        let expected = draft.entries().len();
        (ThemeEditor(draft), expected)
    });
    let _realized = h.mount(root);
    h.flush();
    let dump = h.take_log().join("\n");

    let h2 = host_mock::Harness::new();
    let names: Vec<&'static str> = h2.world.enter(|| {
        install_idea_theme(light_theme());
        ThemeDraft::from_live().entries().iter().map(|e| e.name).collect()
    });
    assert_eq!(names.len(), expected);
    let missing: Vec<&str> = names.into_iter().filter(|n| !dump.contains(n)).collect();
    assert!(missing.is_empty(), "tokens with no row: {missing:?}");
}
