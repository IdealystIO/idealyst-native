//! `Typography(spans = …)` renders text in several styles as ONE text
//! node, mounted through the real `realize` path against `host-mock`.
//!
//! The CrewForge roster card drew a bold name and a muted employee number
//! as two sibling Typography nodes in a row; when the name wrapped, the
//! number sat beside the two-line block instead of following the last
//! word. Only one text node — whose runs the platform text engine wraps
//! together — can flow like a sentence, so these tests pin that a spans
//! Typography is exactly one `create_styled_text` carrying the runs, and
//! that the runs stay live (signal-driven spans, theme swaps).

use idea_ui::{
    dark_theme, install_idea_theme, light_theme, set_idea_theme, tone, typography_kind, TextSpan, Typography,
};
use runtime_core::{rx, signal, ui, FontStyle, FontWeight, TextRun};

/// Every node the harness minted, by its create line.
fn creates(h: &host_mock::Harness) -> Vec<String> {
    h.ops().into_iter().filter(|o| o.starts_with("create ")).collect()
}

fn runs_of(h: &host_mock::Harness, node: u32) -> Vec<TextRun> {
    h.shared.live_runs.borrow().get(&node).cloned().expect("a styled-text node")
}

fn color_token(run: &TextRun) -> Option<&'static str> {
    run.style.as_ref()?.color.as_ref()?.name()
}

#[test]
fn spans_render_as_one_text_node_with_styled_runs() {
    let h = host_mock::Harness::new();
    h.record_all();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        ui! {
            Typography(kind = typography_kind::H5, spans = vec![
                TextSpan::new("Jonas Lindqvist").bold(),
                TextSpan::new(" #1234").muted(),
            ])
        }
    });
    let _realized = h.mount(tree);
    h.flush();

    let created = creates(&h);
    assert_eq!(
        created,
        vec!["create n0 styled_text [\"Jonas Lindqvist\", \" #1234\"]".to_string()],
        "one text node holds both spans — no sibling text nodes"
    );
    let runs = runs_of(&h, 0);
    assert_eq!(runs[0].style.as_ref().and_then(|s| s.font_weight), Some(FontWeight::Bold));
    assert_eq!(color_token(&runs[0]), None, "an unset color inherits the paragraph's");
    assert_eq!(color_token(&runs[1]), Some("color-text-muted"));
    assert!(
        h.ops().iter().any(|o| o.starts_with("apply_style n0")),
        "the Typography style lands on the same node: {:?}",
        h.ops()
    );
}

#[test]
fn span_tone_italic_mono_and_size_reach_the_run() {
    let h = host_mock::Harness::new();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        ui! {
            Typography(spans = vec![
                TextSpan::new("plain "),
                TextSpan::new("danger").tone(tone::Danger).muted(),
                TextSpan::new(" it").italic(),
                TextSpan::new(" id-42").mono(),
                TextSpan::new(" small").size(typography_kind::Caption),
            ])
        }
    });
    let _realized = h.mount(tree);
    let runs = runs_of(&h, 0);
    assert!(runs[0].style.is_none(), "a span with no style is a plain run");
    assert_eq!(color_token(&runs[1]), Some("intent-danger-fg"), "tone wins over muted");
    assert_eq!(runs[2].style.as_ref().and_then(|s| s.font_style), Some(FontStyle::Italic));
    assert_eq!(
        runs[3].style.as_ref().and_then(|s| s.font_family.clone()),
        Some(runtime_core::FontFamily::System(idea_ui::MONO_FONT_STACK.into()))
    );
    assert_eq!(
        runs[4].style.as_ref().and_then(|s| s.font_size.as_ref()).and_then(|t| t.name()),
        Some("typography-caption-size"),
    );
}

#[test]
fn signal_driven_spans_update_the_same_node() {
    let h = host_mock::Harness::new();
    h.record_all();
    let (tree, name) = h.world.enter(|| {
        install_idea_theme(light_theme());
        let name = signal(String::from("Jonas"));
        let tree = ui! {
            Typography(spans = rx!(vec![
                TextSpan::new(name.get()).bold(),
                TextSpan::new(" #1234").muted(),
            ]))
        };
        (tree, name)
    });
    let _realized = h.mount(tree);
    h.flush();
    h.world.enter(|| name.set("Jonas Lindqvist".into()));
    h.flush();

    assert_eq!(creates(&h).len(), 1, "the change re-renders in place: {:?}", h.ops());
    assert!(
        h.ops().contains(&"update_styled_text n0 [\"Jonas Lindqvist\", \" #1234\"]".to_string()),
        "{:?}",
        h.ops()
    );
    let runs = runs_of(&h, 0);
    assert_eq!(runs[0].style.as_ref().and_then(|s| s.font_weight), Some(FontWeight::Bold));
    assert_eq!(color_token(&runs[1]), Some("color-text-muted"));
}

/// Native backends resolve a run's token color when they build the
/// attributed string, so the node must be re-realized on a theme swap or
/// a muted span keeps the light theme's gray in dark mode.
#[test]
fn spans_rerealize_on_a_theme_swap() {
    let h = host_mock::Harness::new();
    h.record_all();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        ui! { Typography(spans = vec![TextSpan::new("a"), TextSpan::new(" b").muted()]) }
    });
    let _realized = h.mount(tree);
    h.flush();
    h.world.enter(|| set_idea_theme(dark_theme()));
    h.flush();
    assert!(
        h.ops().contains(&"update_styled_text n0 [\"a\", \" b\"]".to_string()),
        "{:?}",
        h.ops()
    );
}

#[test]
fn empty_spans_keep_the_plain_content_path() {
    let h = host_mock::Harness::new();
    h.record_all();
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        ui! { Typography(content = "Hello") }
    });
    let _realized = h.mount(tree);
    assert_eq!(creates(&h), vec!["create n0 text \"Hello\"".to_string()]);
}
