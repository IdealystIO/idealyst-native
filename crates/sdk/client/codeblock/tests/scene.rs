//! The authored surface — `code_block(spans).with_style(…)` — mounted
//! through the scene registry on the shared `host-mock` recording
//! substrate (crates/dev/host-mock).
//!
//! The op-log assertions pin the portable handler's DOM shape
//! (`create_element("pre")` → per-run `create_text` + literal-color
//! `apply_style` + `insert`, author style on the `<pre>`): this is the
//! regression fence for the website's SSG output — if the handler's
//! emission order or per-span styling drifts, these fail before the SSG
//! diff does.

use codeblock::{code_block, code_editor, Decoration, DecorationStyle, Underline};
use host_mock::Harness;
use runtime_scene::Realized;
use runtime_shared::{Color, StyleRules, Tokenized};
use runtime_vocabulary::glue::IntoElement;
use runtime_world::{signal, Signal};

fn harness() -> Harness {
    // The SDK's boot registration seam — the same fn an app passes to
    // `backend_web::newcore::start_in` / `backend_ssr::newcore::
    // render_path_with`.
    let h = Harness::with_registry(|r| codeblock::register(r));
    // Mirror the historical Mini's recorded op set: update_text /
    // on_node_unstyled / mark_container were no-ops there, so the
    // exact-log expectations predate those families.
    h.mute(&["update_text", "on_node_unstyled", "mark_container"]);
    // The Mini logged apply_style as a color digest, not the canonical
    // width line — keep the byte-stable expectations.
    h.set_style_line(|n, style| {
        let color = match &style.color {
            Some(Tokenized::Literal(c)) => c.0.clone(),
            Some(_) => "<token>".into(),
            None => "<none>".into(),
        };
        format!("style n{n} color={color}")
    });
    h
}

fn spans() -> Vec<(String, Color)> {
    vec![
        ("fn ".into(), Color("#888".into())),
        ("hello".into(), Color("#0a0".into())),
    ]
}

#[test]
fn mounts_one_pre_with_one_colored_text_per_run() {
    let h = harness();
    let element = code_block(spans()).into_element();
    let _realized: Realized<u32> = h.mount(element);

    // Exact op-log: one outer `<pre>`, then per run create_text →
    // literal-color apply_style → insert, in span order.
    let log = h.ops().join("\n");
    assert_eq!(
        log,
        "create n0 element \"pre\"\n\
         create n1 text \"fn \"\n\
         style n1 color=#888\n\
         insert n0 <- n1\n\
         create n2 text \"hello\"\n\
         style n2 color=#0a0\n\
         insert n0 <- n2",
        "codeblock <pre>/span DOM shape drifted"
    );
}

#[test]
fn author_style_lands_on_the_pre() {
    let h = harness();
    let mut author = StyleRules::default();
    author.color = Some(Tokenized::Literal(Color("#123456".into())));
    let element = code_block(spans()).with_style(author).into_element();
    let _realized: Realized<u32> = h.mount(element);

    // The author style is the LAST style op and targets the outer node
    // (n0, the `<pre>`).
    let log = h.ops();
    assert_eq!(
        log.last().map(String::as_str),
        Some("style n0 color=#123456"),
        "author style must attach to the outer <pre>: {log:?}"
    );
}

// ============================================================================
// code_editor — the editable, decorated sibling.
// ============================================================================

/// A harness that records the ops the editor's shape depends on. Unlike
/// the `code_block` harness above it does NOT mute `update_styled_text`:
/// the whole point of the editor is that a keystroke re-decorates in
/// place rather than rebuilding nodes.
fn editor_harness() -> Harness {
    let h = Harness::with_registry(|r| codeblock::register(r));
    h.record_all();
    h
}

/// A tokenizer stand-in: colors the leading `fn` keyword, nothing else.
/// It exists to prove the primitive never looks at the text itself —
/// every range here comes from the caller.
fn keyword_decorations(text: &str) -> Vec<Decoration> {
    match text.find("fn") {
        Some(at) => vec![Decoration::color(at..at + 2, "#a626a4")],
        None => Vec::new(),
    }
}

/// Mount a decorate-by-fn editor over a fresh buffer signal. Signals
/// are created inside the harness world (`signal()` outside
/// `World::enter` panics by design), and the `Realized` comes back with
/// the signal because dropping it disposes the subtree — including the
/// re-decorate effect the edit tests are about to exercise.
fn mount_editor(h: &Harness, initial: &str) -> (Signal<String>, Realized<u32>) {
    let src = h.world.enter(|| signal(String::from(initial)));
    let element = code_editor(src, move |next| src.set(next))
        .decorate(keyword_decorations)
        .into_element();
    let realized = h.mount(element);
    (src, realized)
}

#[test]
fn mounts_one_decorated_layer_and_one_real_text_area() {
    let h = editor_harness();
    let _mounted = mount_editor(&h, "fn main");

    let log = h.ops().join("\n");
    // The decorated layer is ONE styled_text node carrying the run
    // split, inside a `<pre>` (whitespace preservation on web).
    assert!(
        log.contains(r#"styled_text ["fn", " main"]"#),
        "decorated layer must be one styled_text node split at the decoration: {log}"
    );
    assert!(log.contains(r#"element "pre""#), "decorated layer needs its <pre>: {log}");
    // And the editing layer is a real text_area, not a reimplementation.
    assert!(log.contains("text_area"), "editing layer must be a text_area: {log}");
}

/// The decorated layer must be created BEFORE the editing layer, so the
/// editor paints on top and takes the pointer and keyboard. Reversed,
/// the highlight sits over the caret and swallows clicks.
#[test]
fn decorated_layer_mounts_beneath_the_editing_layer() {
    let h = editor_harness();
    let _mounted = mount_editor(&h, "fn main");

    let log = h.ops().join("\n");
    let decorated = log.find("styled_text").expect("decorated layer mounted");
    let editor = log.find("text_area").expect("editing layer mounted");
    assert!(decorated < editor, "decorated layer must mount first: {log}");
}

/// An edit re-decorates the SAME node — one `update_styled_text`, no
/// node churn. This is what makes a per-keystroke re-tokenize
/// affordable, and it's the property that regresses if the handler ever
/// starts rebuilding the layer instead of updating it.
#[test]
fn an_edit_updates_the_decorated_layer_in_place() {
    let h = editor_harness();
    let (src, _mounted) = mount_editor(&h, "fn main");
    h.clear_ops();

    h.world.enter(|| src.set(String::from("fn other")));
    h.flush();

    let log = h.ops().join("\n");
    assert!(
        log.contains("update_styled_text") && log.contains(r#"["fn", " other"]"#),
        "the edit must re-decorate in place: {log}"
    );
    assert!(
        !log.contains("create "),
        "re-decorating must not create nodes: {log}"
    );
}

/// Decorations arriving from a signal (async diagnostics) must reach the
/// decorated layer on their own, with the buffer untouched.
#[test]
fn decorations_from_a_signal_refresh_the_layer_on_their_own() {
    let h = editor_harness();
    let (src, diagnostics) = h.world.enter(|| {
        (signal(String::from("let x")), signal(Vec::<Decoration>::new()))
    });
    let _realized: Realized<u32> = h.mount(
        code_editor(src, move |next| src.set(next))
            .decorations(diagnostics.read_only())
            .into_element(),
    );
    h.clear_ops();

    h.world.enter(|| {
        diagnostics.set(vec![Decoration::underline(
            0..3,
            Underline::dotted().colored("#c00"),
        )])
    });
    h.flush();

    let log = h.ops().join("\n");
    assert!(
        log.contains("update_styled_text") && log.contains(r#"["let", " x"]"#),
        "late-arriving decorations must re-split the layer: {log}"
    );
}

/// Diagnostics that describe a buffer the user has already shortened
/// must clamp, not panic — an async producer is always a frame behind.
#[test]
fn regression_stale_decorations_against_a_shortened_buffer_do_not_panic() {
    let h = editor_harness();
    let (src, diagnostics) = h.world.enter(|| {
        (
            signal(String::from("fn main() {}")),
            signal(vec![Decoration::new(
                0..999,
                DecorationStyle::default().with_color("#f00"),
            )]),
        )
    });
    let _realized: Realized<u32> = h.mount(
        code_editor(src, move |next| src.set(next))
            .decorations(diagnostics.read_only())
            .into_element(),
    );
    h.clear_ops();

    h.world.enter(|| src.set(String::from("fn")));
    h.flush();

    let log = h.ops().join("\n");
    assert!(
        log.contains(r#"["fn"]"#),
        "the stale range must clamp to the live buffer: {log}"
    );
}

#[test]
fn author_style_lands_on_the_editors_outer_box() {
    let h = editor_harness();
    let src = h.world.enter(|| signal(String::new()));
    let mut author = StyleRules::default();
    author.background = Some(Tokenized::Literal(Color("#101010".into())));
    let _realized: Realized<u32> = h.mount(
        code_editor(src, move |next| src.set(next))
            .with_style(author)
            .into_element(),
    );

    // The outer box is node 0 — created before the stack, the <pre>,
    // the decorated layer and the editing layer.
    let log = h.ops();
    assert!(
        log.iter().any(|l| l.starts_with("apply_style n0 ")),
        "author style must reach the outer node: {log:?}"
    );
}
