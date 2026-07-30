//! `Markdown(source = …)` (struct-literal `BuildElement` dispatch) and
//! `markdown(src, theme).with_style(…).bind(r)` mounted through the
//! scene registry on the shared `host-mock` recording substrate.
//!
//! The op-log assertion pins the semantic-DOM shape: outer `<div>`
//! (theme base color/size), per block a semantic element styled then
//! filled with per-run styled text, block inserted into the root AFTER
//! its runs. This is the regression fence — if the handler's emission
//! order or per-run styling drifts, this fails before any live parity
//! diff does.

use host_mock::Harness;
use markdown::{markdown, Markdown, MarkdownHandle, MdTheme};
use runtime_shared::{FontStyle, FontWeight, Length, Ref, StyleRules, Tokenized};
use runtime_scene::Realized;
use runtime_vocabulary::glue::{self, BuildElement, IntoElement, Reactive};

fn harness() -> Harness {
    // The SDK's boot registration seam — the same fn an app passes to
    // `backend_web::newcore::start_in` / `backend_ssr::newcore::
    // render_path_with`.
    let h = Harness::with_registry(|r| markdown::register(r));
    // Mirror the historical Mini's recorded op set (codeblock idiom):
    // update_text / on_node_unstyled / mark_container were no-ops
    // there, so exact-log expectations predate those families.
    h.mute(&["update_text", "on_node_unstyled", "mark_container"]);
    // Digest the style facets the markdown handler actually drives
    // (literal color / font-size / weight / style) so the log stays
    // byte-stable while still pinning the per-node styling.
    h.set_style_line(|n, s| {
        let mut line = format!("style n{n}");
        if let Some(Tokenized::Literal(c)) = &s.color {
            line.push_str(&format!(" color={}", c.0));
        }
        if let Some(Tokenized::Literal(Length::Px(px))) = &s.font_size {
            line.push_str(&format!(" size={px}"));
        }
        if matches!(s.font_weight, Some(FontWeight::Bold)) {
            line.push_str(" bold");
        }
        if matches!(s.font_style, Some(FontStyle::Italic)) {
            line.push_str(" italic");
        }
        line
    });
    h
}

/// The byte-parity fence: exact op-log for a small doc, derived from
/// the old web handler's call sequence (create root div + root style →
/// per block: create element, style it, per run create_text + style +
/// insert, then insert the block into the root). Light theme:
/// text=#1f2328, heading=#0b0c0e, base 16px, h1 = 16 × 2.0 = 32px;
/// heading runs take the heading color, paragraph runs the text color,
/// the `**bold**` run adds `font_weight: Bold`.
#[test]
fn mounts_the_old_web_handler_dom_shape() {
    let h = harness();
    let element = markdown("# Hello\n\nWorld **bold**", MdTheme::light()).into_element();
    let _realized: Realized<u32> = h.mount(element);

    let log = h.ops().join("\n");
    assert_eq!(
        log,
        "create n0 element \"div\"\n\
         style n0 color=#1f2328 size=16\n\
         create n1 element \"h1\"\n\
         style n1 color=#0b0c0e size=32 bold\n\
         create n2 text \"Hello\"\n\
         style n2 color=#0b0c0e\n\
         insert n1 <- n2\n\
         insert n0 <- n1\n\
         create n3 element \"p\"\n\
         style n3\n\
         create n4 text \"World \"\n\
         style n4 color=#1f2328\n\
         insert n3 <- n4\n\
         create n5 text \"bold\"\n\
         style n5 color=#1f2328 bold\n\
         insert n3 <- n5\n\
         insert n0 <- n3",
        "markdown semantic-DOM shape drifted"
    );
}

/// Author style attaches to the OUTER node, AFTER the DOM builds (the
/// old walker's external style-attach order).
#[test]
fn author_style_lands_on_the_outer_div_last() {
    let h = harness();
    let mut author = StyleRules::default();
    author.color = Some(Tokenized::Literal(runtime_shared::Color("#123456".into())));
    let element = markdown("# Hi", MdTheme::light())
        .with_style(author)
        .into_element();
    let _realized: Realized<u32> = h.mount(element);

    let log = h.ops();
    assert_eq!(
        log.last().map(String::as_str),
        Some("style n0 color=#123456"),
        "author style must attach to the outer <div> after the doc DOM: {log:?}"
    );
}

/// `.bind` fills the `Ref<MarkdownHandle>` at mount with the outer node.
#[test]
fn bind_fills_the_ref_at_mount() {
    let h = harness();
    let r: Ref<MarkdownHandle> = Ref::new();
    let element = markdown("# Hi", MdTheme::light())
        .bind(r.clone())
        .into_element();
    let _realized: Realized<u32> = h.mount(element);

    let node = r
        .with(|handle| {
            *handle
                .node()
                .downcast_ref::<u32>()
                .expect("host-mock node is u32")
        })
        .expect("ref filled at mount");
    assert_eq!(node, 0, "the handle wraps the outer root node");
}

/// The `Markdown` tag (struct-literal `BuildElement` dispatch through
/// the `pub type Markdown = MarkdownProps` alias) mounts through the
/// `switch` path and REBUILDS when the reactive source changes —
/// the old `#[component]` body's contract, reproduced manually.
#[test]
fn markdown_tag_switch_rebuilds_on_source_change() {
    let h = harness();
    let src = h.world.enter(|| glue::signal("# One".to_string()));
    let element = Markdown {
        source: Reactive::derive(move || src.get()),
        theme: Reactive::Static(MdTheme::light()),
        ..Markdown::defaults()
    }
    .build();
    let _realized: Realized<u32> = h.mount(element);

    let log = h.take_log().join("\n");
    assert!(log.contains("text \"One\""), "initial doc mounted: {log}");

    h.world.enter(|| src.set("# Two".to_string()));
    h.flush();
    let log = h.take_log().join("\n");
    assert!(
        log.contains("text \"Two\""),
        "source change must rebuild the doc: {log}"
    );

    // Dedupe-on-equal-scrutinee (the `switch` contract): an
    // equal re-set must not remount (guarded Signal::set + PartialEq
    // keying both hold the mounted arm).
    h.world.enter(|| src.set("# Two".to_string()));
    h.flush();
    let log = h.take_log().join("\n");
    assert!(
        !log.contains("create"),
        "equal source must not rebuild the doc: {log}"
    );
}
