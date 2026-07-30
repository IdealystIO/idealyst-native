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

use codeblock::code_block;
use host_mock::Harness;
use runtime_shared::{Color, StyleRules, Tokenized};
use runtime_scene::Realized;
use runtime_vocabulary::glue::IntoElement;

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
