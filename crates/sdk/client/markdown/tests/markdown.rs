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

/// `MarkdownHandle: PartialEq` compares the mounted node, so a handle equals its own
/// clones and never equals a handle onto a different mounted `Markdown`.
/// Required for the handle to sit in a `Signal` at all (`Signal<T>` bounds
/// `T: PartialEq` at creation and `get`), and only this crate can supply
/// the impl — the orphan rule blocks an app crate.
///
/// The unequal half is the load-bearing one: every handle on a target
/// shares the same `&'static` ops vtable, so an impl that compared `ops`
/// would collapse two distinct nodes into one and swallow a re-target.
#[test]
fn markdown_handles_compare_by_mounted_node_identity() {
    let h = harness();

    let ra: Ref<MarkdownHandle> = Ref::new();
    let rb: Ref<MarkdownHandle> = Ref::new();
    let _a: Realized<u32> = h.mount(markdown("# Hi", MdTheme::light()).bind(ra.clone()).into_element());
    let _b: Realized<u32> = h.mount(markdown("# Hi", MdTheme::light()).bind(rb.clone()).into_element());

    let ha = ra.with(|handle| handle.clone()).expect("a mounted");
    let hb = rb.with(|handle| handle.clone()).expect("b mounted");

    assert!(ha == ha.clone(), "clones of one handle must compare equal");
    assert!(ha != hb, "handles onto two different mounted nodes must compare unequal");
}

// ===========================================================================
// 1.1.0 registration seams
// ===========================================================================

/// `defer` must be safe to call on a NON-WEB target.
///
/// Web is the only target that code-splits, so off-web `defer` installs
/// the handler eagerly instead of declaring the kind late-bound. Without
/// that arm a host or native app calling `defer` would park every
/// markdown node behind a layout-transparent placeholder forever — no
/// panic, no log, just missing content, because no chunk is coming to
/// drain it.
///
/// Regression test for that arm: rendering through `defer` must produce
/// the same mounted node `register` produces. A `defer` that only
/// declared (the wasm behavior) would park the item and realize nothing.
#[test]
fn defer_registers_eagerly_off_web() {
    let h = Harness::with_registry(|r| markdown::defer(r));
    let _realized: Realized<_> = h.mount(markdown("# Heading\n\nBody **bold**", MdTheme::light()).into_element());
    let log = h.ops().join("\n");
    assert!(
        log.contains("element \"div\""),
        "`defer` installed the handler off-web, so the item mounted: {log}"
    );
    assert!(
        log.contains("element \"p\""),
        "the handler built real block DOM, so the item was not parked: {log}"
    );
}

/// The `register_from_chunk` stub must be inert off-web.
///
/// A `#[component(lazy)]` body calls it unconditionally, so it has to
/// compile everywhere. Off-web it must queue nothing: `defer` already
/// registered, and a late registration for a kind never declared
/// deferred panics inside `Registry::register_deferred` on the next
/// realize.
#[test]
fn register_from_chunk_is_inert_off_web() {
    markdown::register_from_chunk::<host_mock::HostMock>();

    let h = Harness::with_registry(|r| markdown::register(r));
    let _realized: Realized<_> =
        h.mount(markdown("plain", MdTheme::light()).into_element());
    assert!(
        h.ops().join("\n").contains("element \"div\""),
        "realize after the inert stub is unaffected: {}",
        h.ops().join("\n")
    );
}
