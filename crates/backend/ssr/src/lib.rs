//! Server-side rendering backend.
//!
//! `SsrBackend` is a headless renderer: instead of mutating a live DOM
//! (web) or a native view tree (iOS/Android), it accumulates an in-memory
//! HTML node tree and serializes it to a string. It runs on the host
//! (native) target — the same author tree that renders on every other
//! backend is realized once, synchronously, and emitted as HTML + a
//! `<head>` stylesheet for first paint, crawlers, and link unfurlers.
//!
//! The emitted markup is adoptable, not throwaway: the served page loads
//! the normal WebBackend wasm bundle, which HYDRATES this DOM. Because
//! styling reuses the same [`css::rules_to_css`] the web backend uses,
//! the first paint matches what the live app renders — no flash when the
//! bundle takes over.
//!
//! # Rendering surface
//!
//! [`newcore::render_to_string`] / [`newcore::render_path`] /
//! [`newcore::render_path_with`] are the per-request entry points,
//! [`newcore::render_all`] the SSG crawl, and [`newcore::serve`] the
//! preview server; the backend's capability implementation (the
//! `runtime_scene::Host` seam plus the 30 `runtime_vocabulary::caps`
//! traits) lives in [`newcore`]. This crate carried a second,
//! `Element`-walking `impl runtime_core::Backend` until the old core was
//! removed; every method body moved into the capability impls verbatim,
//! so the emitted bytes are unchanged — pinned against the old core's
//! frozen output by `tests/newcore_byte_identity.rs` + `tests/goldens/`
//! and by `websites/website/tests/ssg_parity.rs`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[cfg(feature = "serve")]
mod serve;
#[cfg(feature = "serve")]
pub use serve::{resolve_bundle_module, ServeConfig};

// New-core (idea-lite) render path: `SsrBackend` as a direct
// `runtime_scene::Host` + 30-caps implementor, with per-request
// `runtime_world::World` lifecycles (`newcore::render_path`). Additive —
// nothing in the old-core path below changes under the feature.
/// `runtime_scene::Host` + the 30 capability traits on [`SsrBackend`],
/// plus the per-request-world render entries.
pub mod newcore;

/// A self-contained node handle — like a DOM node or a `UIView`, not an
/// arena index. Children splice in via interior mutability without
/// going through the backend, which is what lets deferred chrome (a
/// drawer sidebar built post-mount via `build_node`) attach itself into
/// its slot from a closure that has no backend reference.
pub type NodeRef = Rc<RefCell<HtmlNode>>;

/// One element in the accumulated HTML tree. Public only because it's
/// named by the backend's `Node` associated type; its fields are
/// private (build nodes via the `Backend` methods, not directly).
pub struct HtmlNode {
    tag: &'static str,
    /// Text content for leaf text nodes (escaped at serialization time).
    text: Option<String>,
    /// Inline CSS declaration body from `css::rules_to_css`.
    style: Option<String>,
    /// Extra attributes (e.g. `src`, `alt`) as (name, value) pairs.
    attrs: Vec<(&'static str, String)>,
    /// `true` for `ScrollView` nodes — emits `overflow: auto` ahead of
    /// any author style. Scrolling is the ScrollView primitive's job, not
    /// an `overflow` style (which the framework keeps clip-only), so this
    /// lives on the node, not in `StyleRules`.
    scroll: bool,
    /// `true` for the text primitive (`create_text`). A `shadow` on a text
    /// node lowers to `text-shadow` (hugging the glyphs) instead of
    /// `box-shadow`, matching the web backend; see `apply_style`.
    is_text: bool,
    /// The minted style class currently applied through
    /// `apply_style`/`apply_styled_variants` (ONE mutable styled-class
    /// slot per node — the web backend's per-node dynamic-slot model).
    /// A re-apply REPLACES this class instead of accumulating: a
    /// reactive style whose value changes between the mount-time apply
    /// and the request's flush (e.g. a scroll-spy TOC link resolving
    /// its `active` variant) must serialize wearing only the FINAL
    /// class, exactly like the live DOM after the same flush. Classes
    /// stamped via `attach_html_class` (structural markers,
    /// `ui-cq-container`) live outside this slot and are never removed.
    styled_class: Option<String>,
    children: Vec<NodeRef>,
}

impl HtmlNode {
    fn new(tag: &'static str) -> Self {
        Self {
            tag,
            text: None,
            style: None,
            attrs: Vec::new(),
            scroll: false,
            is_text: false,
            styled_class: None,
            children: Vec::new(),
        }
    }
}

/// Wrap a freshly-built node in a shareable handle.
fn nref(n: HtmlNode) -> NodeRef {
    Rc::new(RefCell::new(n))
}

/// Set (or replace) an attribute on a node.
fn set_attr(node: &NodeRef, name: &'static str, value: String) {
    let mut n = node.borrow_mut();
    if let Some(slot) = n.attrs.iter_mut().find(|(k, _)| *k == name) {
        slot.1 = value;
    } else {
        n.attrs.push((name, value));
    }
}

/// Remove an attribute from a node if present.
fn remove_attr(node: &NodeRef, name: &'static str) {
    node.borrow_mut().attrs.retain(|(k, _)| *k != name);
}

/// Append a space-separated class to a node's `class` attribute (so a
/// chrome handler can stamp `ui-nav-root` then `ui-nav-drawer-root` on
/// the same node, matching the live web navigator).
fn add_class(node: &NodeRef, class: &str) {
    let mut n = node.borrow_mut();
    if let Some(slot) = n.attrs.iter_mut().find(|(k, _)| *k == "class") {
        if !slot.1.split(' ').any(|c| c == class) {
            slot.1.push(' ');
            slot.1.push_str(class);
        }
    } else {
        n.attrs.push(("class", class.to_string()));
    }
}

/// Remove a single class token from a node's `class` attribute (keeps
/// the attribute, possibly empty-cleaned, and every other token).
fn remove_class(node: &NodeRef, class: &str) {
    let mut n = node.borrow_mut();
    if let Some(slot) = n.attrs.iter_mut().find(|(k, _)| *k == "class") {
        let remaining: Vec<&str> = slot.1.split(' ').filter(|c| *c != class && !c.is_empty()).collect();
        slot.1 = remaining.join(" ");
    }
    n.attrs.retain(|(k, v)| *k != "class" || !v.is_empty());
}

/// Outcome of [`set_styled_class`] — what happened to the node's
/// minted-class slot, so the backend can keep rule refcounts exact.
enum StyledClassChange {
    /// Same class as before — no ref changes.
    Unchanged,
    /// First minted class on this node.
    Fresh,
    /// Replaced a different minted class (the removed one).
    Swapped(String),
}

/// Swap the node's minted styled class (the ONE mutable slot
/// `apply_style`/`apply_styled_variants` own — see
/// [`HtmlNode::styled_class`]): removes the previously-minted class if
/// it differs, records + stamps the new one.
fn set_styled_class(node: &NodeRef, class: &str) -> StyledClassChange {
    let prev = node.borrow().styled_class.clone();
    let change = match prev {
        Some(prev) if prev == class => StyledClassChange::Unchanged,
        Some(prev) => {
            remove_class(node, &prev);
            StyledClassChange::Swapped(prev)
        }
        None => StyledClassChange::Fresh,
    };
    node.borrow_mut().styled_class = Some(class.to_string());
    add_class(node, class);
    change
}

/// Append an inline CSS declaration (`prop:value`) to a node's inline
/// style, merging with any existing one. Used for the drawer navigator's
/// per-instance `--drawer-width` custom property (see
/// `Backend::attach_html_style`) so the SSR first paint matches the width
/// the live web client sets on hydration.
fn add_inline_style(node: &NodeRef, prop: &str, value: &str) {
    let mut n = node.borrow_mut();
    let decl = format!("{prop}:{value}");
    match n.style.as_mut() {
        Some(existing) if !existing.is_empty() => {
            existing.push_str("; ");
            existing.push_str(&decl);
        }
        _ => n.style = Some(decl),
    }
}

fn serialize(node: &NodeRef, out: &mut String) {
    let n = node.borrow();
    out.push('<');
    out.push_str(n.tag);
    // ScrollView nodes get `overflow: auto` ahead of any author style.
    let author = n.style.as_deref().unwrap_or("");
    let style_attr: Option<String> = if n.scroll {
        Some(if author.is_empty() {
            "overflow: auto".to_string()
        } else {
            format!("overflow: auto; {author}")
        })
    } else if !author.is_empty() {
        Some(author.to_string())
    } else {
        None
    };
    if let Some(style) = &style_attr {
        out.push_str(" style=\"");
        escape_attr(style, out);
        out.push('"');
    }
    for (name, value) in &n.attrs {
        out.push(' ');
        out.push_str(name);
        out.push_str("=\"");
        escape_attr(value, out);
        out.push('"');
    }
    out.push('>');
    if is_void(n.tag) {
        return;
    }
    if let Some(text) = &n.text {
        escape_text(text, out);
    }
    for child in &n.children {
        serialize(child, out);
    }
    out.push_str("</");
    out.push_str(n.tag);
    out.push('>');
}

/// HTML void elements have no closing tag and take no children.
fn is_void(tag: &str) -> bool {
    matches!(tag, "img" | "input" | "br" | "hr" | "meta" | "link")
}

#[derive(Default)]
pub struct SsrBackend {
    root: Option<NodeRef>,
    metadata: runtime_shared::PageMetadata,
    /// Stylesheets registered via [`Backend::register_raw_css`] (e.g. the
    /// navigator layout sheet). Deduped, emitted in the document `<head>`.
    raw_css: Vec<String>,
    /// The active theme's tokens, captured from
    /// [`Backend::install_tokens`]/[`Backend::update_tokens`]. Emitted as
    /// a `:root { … }` block so the SSR first paint resolves
    /// `var(--token, fallback)` to the real theme value (matching the
    /// live web build, which installs the same variables at runtime).
    tokens: Vec<runtime_shared::TokenEntry>,
    /// The theme's default text font, captured from
    /// [`Backend::apply_default_text_font`] (premint host driver).
    /// Emitted as `--iy-default-font` on `:root` so preminted rule
    /// bodies (`font-family: var(--iy-default-font, inherit)`) resolve
    /// on the SSR first paint exactly like the live web build.
    default_text_font: Option<runtime_shared::FontFamily>,
    /// Served URL per registered asset id (fonts/images), from
    /// [`Backend::register_asset`]. Fonts feed the `@font-face` rules
    /// below; image URLs are read at `create_image` time.
    asset_urls: HashMap<runtime_shared::assets::AssetId, String>,
    /// `@font-face` rules from [`Backend::register_typeface`], emitted in
    /// `<head>` so the SSR first paint uses the real fonts (matching the
    /// live web build, which links the same served font files).
    font_faces: Vec<String>,
    /// Content-keyed style classes from [`Backend::apply_style`]
    /// (`ui-<hash>` → declaration body), deduped — the same class+rule
    /// model the web backend uses, emitted as a `<head>` stylesheet
    /// instead of inline `style="…"`. `BTreeMap` for deterministic output.
    style_rules: std::collections::BTreeMap<String, String>,
    /// Responsive breakpoint overlays from [`Backend::apply_styled_variants`],
    /// emitted as `@media (min-width: …) { .ui-<hash> { … } }` rules so the
    /// SSR first paint already respects size boundaries — a mobile request
    /// gets the mobile layout in static HTML, with no JS/hydration needed
    /// to correct it. Same `css::breakpoint_media_rule` the web backend
    /// inserts at runtime, so the rule is byte-identical across both.
    ///
    /// Keyed by `{class}@{rank}` so the `BTreeMap` orders media rules
    /// **ascending by breakpoint rank within each class** — stacked
    /// min-width queries then cascade mobile-first (higher breakpoints,
    /// later in source, win on conflicting properties). Emitted AFTER the
    /// plain class rules in `head_css` so a matching `@media` overrides the
    /// base declaration.
    media_rules: std::collections::BTreeMap<String, String>,
    /// Wearer counts per minted class (how many nodes currently carry
    /// it in their [`HtmlNode::styled_class`] slot). When a re-apply
    /// swaps a node's class and the count hits zero, the superseded
    /// rule (plus its pseudo/media derivatives) is dropped from the
    /// head stylesheet — the web backend's dynamic-slot refcount model.
    /// Without this, a reactive style resolving to its final value at
    /// flush left the mount-time rule behind as a dead head entry that
    /// old-core SSR (single final apply) never emits.
    style_rule_refs: HashMap<String, u32>,
    /// Host-surface background captured from [`Backend::set_app_background`].
    /// Emitted in `<head>` as `html, body { background: …; }` so the SSR
    /// first paint matches what the web backend installs at runtime. For a
    /// `Tokenized::Token` we emit `var(--<name>)` (live-reactive after
    /// hydration via the `:root` setProperty path); for `Tokenized::Literal`
    /// we emit the resolved color value.
    app_bg: Option<runtime_shared::Tokenized<runtime_shared::Color>>,
    /// Scrollbar (thumb, track) captured from [`Backend::set_scrollbar_theme`].
    /// Emitted in `<head>` as `scrollbar-color` + `::-webkit-scrollbar-*`
    /// rules — same shape the web backend installs at runtime so the SSR
    /// first paint matches.
    scrollbar: Option<(
        runtime_shared::Tokenized<runtime_shared::Color>,
        runtime_shared::Tokenized<runtime_shared::Color>,
    )>,
}

impl SsrBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Book a node's minted-class change into the rule refcounts,
    /// dropping a superseded class's rules once nothing wears it (see
    /// [`Self::style_rule_refs`]). Node teardown does NOT decrement —
    /// the old core keeps rules of dropped nodes in the head too, and
    /// the cross-core byte gate compares against that behavior.
    fn book_styled_class(&mut self, class: &str, change: StyledClassChange) {
        match change {
            StyledClassChange::Unchanged => {}
            StyledClassChange::Fresh => {
                *self.style_rule_refs.entry(class.to_string()).or_insert(0) += 1;
            }
            StyledClassChange::Swapped(prev) => {
                *self.style_rule_refs.entry(class.to_string()).or_insert(0) += 1;
                let dead = match self.style_rule_refs.get_mut(&prev) {
                    Some(n) => {
                        *n = n.saturating_sub(1);
                        *n == 0
                    }
                    None => false,
                };
                if dead {
                    self.style_rule_refs.remove(&prev);
                    // The base rule, its pseudo-class derivatives
                    // (`{class}:hover`, `{class}[disabled]`), and its
                    // media/container derivatives (`{class}@…`).
                    let pseudo_colon = format!("{prev}:");
                    let pseudo_attr = format!("{prev}[");
                    self.style_rules.retain(|k, _| {
                        k != &prev && !k.starts_with(&pseudo_colon) && !k.starts_with(&pseudo_attr)
                    });
                    let media_prefix = format!("{prev}@");
                    self.media_rules.retain(|k, _| !k.starts_with(&media_prefix));
                }
            }
        }
    }

    /// Serialize the tree to an HTML string, rooted at the node passed to
    /// [`Backend::finish`]. Empty string if nothing was mounted.
    pub fn into_html(&self) -> String {
        let mut out = String::new();
        if let Some(root) = &self.root {
            serialize(root, &mut out);
        }
        out
    }

    /// CSS for the document `<head>`, in cascade order:
    /// 1. base reset (`box-sizing`, `<button>` reset) — specificity 0, so
    ///    author classes win;
    /// 2. `@font-face` rules (real fonts on first paint);
    /// 3. the theme's `:root` token variables (so `var(--token, …)`
    ///    resolves to the real theme value, matching web);
    /// 4. host-surface theming (body background, scrollbar) — emitted
    ///    AFTER the `:root` block so any `var(--…)` references resolve;
    /// 5. registered stylesheets (navigator layout, etc.);
    /// 6. the content-keyed per-node style classes (`apply_style`);
    /// 7. responsive `@media (min-width: …)` breakpoint overlays — LAST,
    ///    so a matching media query overrides the base class rule above it.
    pub fn head_css(&self) -> String {
        let mut out = css::base_reset_css();
        out.push_str(&self.font_faces.concat());
        out.push_str(&css::tokens_to_root_css(&self.tokens));
        if let Some(ff) = &self.default_text_font {
            // BOTH the variable and a real, INHERITABLE `font-family`.
            //
            // The variable alone is what preminted rule bodies read
            // (`font-family: var(--iy-default-font, inherit)`), but only
            // STATIC style applications get the theme font folded into
            // their own rules (`fill_default_text_font`). A reactively
            // styled node deliberately skips that fold, so with the
            // variable alone it received no `font-family` from anywhere
            // and inherited — falling back to the browser's serif when no
            // ancestor set one, while an identically-styled static
            // sibling rendered in the theme font.
            //
            // Declaring `font-family` on `:root` is what makes the
            // "reactive nodes ride the document channel" contract in
            // `style_attach`'s dynamic branch actually true. It is
            // deliberately NOT a fold into the node's rules: folding
            // would change the minted class hash for every reactive node
            // and break SSR/live class-name parity, which is exactly why
            // the dynamic path doesn't fold. Inheritance costs no hash.
            let value = css::font_family_css_value(ff);
            out.push_str(&format!(
                ":root {{ {}: {value}; font-family: var({}); }}",
                css::DEFAULT_TEXT_FONT_VAR,
                css::DEFAULT_TEXT_FONT_VAR,
            ));
        }
        if let Some(color) = &self.app_bg {
            out.push_str(&format!(
                "html, body {{ background: {}; }}",
                color_css_value(color),
            ));
        }
        if let Some((thumb, track)) = &self.scrollbar {
            let thumb_v = color_css_value(thumb);
            let track_v = color_css_value(track);
            out.push_str(&format!(
                "html {{ scrollbar-color: {thumb_v} {track_v}; }}\
                 ::-webkit-scrollbar {{ width: 10px; height: 10px; }}\
                 ::-webkit-scrollbar-track {{ background: {track_v}; }}\
                 ::-webkit-scrollbar-thumb {{ background: {thumb_v}; border-radius: 5px; }}",
            ));
        }
        out.push_str(&self.raw_css.concat());
        for (class, body) in &self.style_rules {
            out.push('.');
            out.push_str(class);
            out.push('{');
            out.push_str(body);
            out.push('}');
        }
        for rule in self.media_rules.values() {
            out.push_str(rule);
        }
        out
    }
}

/// Render a `Tokenized<Color>` to its CSS value form: `var(--<name>)`
/// for a token reference (so live-reactive after hydration via `:root`
/// setProperty), the raw color string for a literal. Shared by SSR's
/// host-surface rules so the emitted CSS matches what the web backend
/// installs at runtime.
fn color_css_value(color: &runtime_shared::Tokenized<runtime_shared::Color>) -> String {
    match color.name() {
        Some(name) => format!("var(--{name})"),
        None => color.value().0.clone(),
    }
}



/// Escape text content: `&`, `<`, `>` (sufficient for element bodies).
fn escape_text(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
}

/// Escape a double-quoted attribute value: text escapes plus `"`.
fn escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

// ---------------------------------------------------------------------------
// Microtask scheduler.
//
// SSR is synchronous and has no event loop, but chrome that renders an
// author `Element` (a drawer sidebar) must call `host.build_node`, which
// can't run inside the `create_navigator` borrow. Handlers defer it via
// `schedule_microtask`; without an installed scheduler that runs INLINE
// (still inside the borrow → double-borrow panic). So SSR installs a
// scheduler that QUEUES microtasks, and `render_path` drains the queue
// after `mount` (borrow released) so deferred builds run cleanly.
// ---------------------------------------------------------------------------

mod scheduler {
    use runtime_shared::scheduling::{ScheduleHandle, Scheduler};
    use std::cell::RefCell;
    use std::collections::VecDeque;

    thread_local! {
        static QUEUE: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> =
            RefCell::new(VecDeque::new());
    }

    struct NoopHandle;
    impl ScheduleHandle for NoopHandle {
        fn cancel(&mut self) {}
    }

    struct SsrScheduler;
    impl Scheduler for SsrScheduler {
        fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
            QUEUE.with(|q| q.borrow_mut().push_back(f));
        }
        // A static first paint has no frames or timers: drop these
        // callbacks (the live bundle drives animation on hydration).
        fn after_animation_frame(
            &self,
            _f: Box<dyn FnOnce() + 'static>,
        ) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
        fn after_ms(
            &self,
            _delay_ms: i32,
            _f: Box<dyn FnOnce() + 'static>,
        ) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
        fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
    }

    pub(crate) fn ensure_installed() {
        if !runtime_shared::scheduling::is_scheduler_installed() {
            runtime_shared::scheduling::install_scheduler(Box::new(SsrScheduler));
        }
    }

    /// Run every queued microtask (and any they enqueue) to completion.
    /// Called by `render_path` after `mount`, with no backend borrow held.
    pub(crate) fn drain() {
        loop {
            let next = QUEUE.with(|q| q.borrow_mut().pop_front());
            match next {
                Some(task) => task(),
                None => break,
            }
        }
    }
}


/// A rendered page: the body HTML (styles inline) plus the `<head>`
/// metadata an author screen declared via
/// [`runtime_shared::set_page_metadata`]. Slice that wires the page shell
/// turns `metadata` into `<title>` / `<meta>` / Open Graph tags.
pub struct RenderedPage {
    pub html: String,
    pub metadata: runtime_shared::PageMetadata,
    /// Stylesheets the render registered via
    /// [`Backend::register_raw_css`] (navigator layout sheet, etc.).
    /// [`render_document`] emits these in `<head>` so the server's first
    /// paint matches the live web layout.
    pub head_css: String,
}

/// Font URLs from the `@font-face` rules in `head_css`, in order, deduped.
/// `src:url("…")` only appears in `@font-face` (class rules use
/// `background[-image]:url(…)`), so a scan for that marker reliably yields
/// just the font files to preload.
fn font_src_urls(head_css: &str) -> Vec<&str> {
    const MARK: &str = "src:url(\"";
    let mut urls: Vec<&str> = Vec::new();
    let mut rest = head_css;
    while let Some(i) = rest.find(MARK) {
        rest = &rest[i + MARK.len()..];
        match rest.find('"') {
            Some(end) => {
                let url = &rest[..end];
                if !urls.contains(&url) {
                    urls.push(url);
                }
                rest = &rest[end..];
            }
            None => break,
        }
    }
    urls
}

/// MIME type for a font preload `<link type="…">`, by URL extension.
/// `None` for unknown extensions (the preload still works without it).
fn font_mime_type(url: &str) -> Option<&'static str> {
    let ext = url.rsplit('.').next()?;
    Some(match ext.to_ascii_lowercase().as_str() {
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        _ => return None,
    })
}

fn push_meta_name(out: &mut String, name: &str, content: &str) {
    out.push_str("<meta name=\"");
    out.push_str(name);
    out.push_str("\" content=\"");
    escape_attr(content, out);
    out.push_str("\">");
}

fn push_meta_prop(out: &mut String, property: &str, content: &str) {
    out.push_str("<meta property=\"");
    out.push_str(property);
    out.push_str("\" content=\"");
    escape_attr(content, out);
    out.push_str("\">");
}

/// Wrap a [`RenderedPage`] in a complete HTML document: `<head>` with the
/// page's title / description / Open Graph tags (what crawlers and link
/// unfurlers read), and a `<body>` whose `#app` mount holds the
/// server-rendered screen.
///
/// `bundle_module` controls hydration:
/// - `None` — **just transmit the rendered screen.** No `<script>`; the
///   page is the SSR output verbatim (the right mode for SEO, link
///   unfurling, and a static preview — no JS, no duplication).
/// - `Some(path)` — also emit a module script that boots the WebBackend
///   bundle at `path` (e.g. `/pkg/website.js`), which **replaces**
///   `#app`'s contents on boot (the v1 "hydrate by replacing" path; see
///   the web backend's `finish`, which clears `#app` first). The bundle
///   must be current — an older bundle that predates that clear will
///   *append* a second copy instead of replacing.
///
/// `bundle_module` is developer-provided config, not user input.
pub fn render_document(
    page: &RenderedPage,
    bundle_module: Option<&str>,
    extra_head: Option<&str>,
) -> String {
    let m = &page.metadata;
    let mut doc = String::from("<!DOCTYPE html>\n<html lang=\"en\">\n<head>");
    doc.push_str("<meta charset=\"utf-8\">");
    doc.push_str(
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
    );
    // Preload the fonts the page links, BEFORE the stylesheet that
    // references them, so the browser fetches them at top priority right
    // away. With `font-display:swap` this shrinks (ideally to zero on a
    // warm/fast connection) the window where text shows in the fallback
    // before the web font arrives — no "default font flash." Extracted
    // from the `@font-face` rules in `head_css` (`src:url("…")` only
    // appears there). `crossorigin` is mandatory: fonts are CORS-fetched
    // even same-origin, and the preload must match or it's ignored +
    // refetched (which is also why the bundle no longer re-injects its
    // own `@font-face` on hydration — see `backend-web` typeface dedup).
    for url in font_src_urls(&page.head_css) {
        doc.push_str("<link rel=\"preload\" as=\"font\" crossorigin");
        if let Some(ty) = font_mime_type(url) {
            doc.push_str(" type=\"");
            doc.push_str(ty);
            doc.push('"');
        }
        doc.push_str(" href=\"");
        escape_attr(url, &mut doc);
        doc.push_str("\">");
    }
    // Baseline so a navigator app-shell works: no body margin, and the
    // mount is a fixed-viewport-height flex column. The navigator's body
    // is a ScrollView, so content scrolls inside it (sidebar stays put)
    // rather than the whole page scrolling.
    doc.push_str(
        "<style>html,body{margin:0;height:100%}\
         #app{display:flex;flex-direction:column;height:100vh}\
         #app>*{flex:1;min-height:0}</style>",
    );
    // Stylesheets the render registered (navigator layout, etc.) — the
    // single source of truth shared with the live web backend, so the
    // first paint matches and there's no style-flash on hydration.
    if !page.head_css.is_empty() {
        doc.push_str("<style>");
        doc.push_str(&page.head_css);
        doc.push_str("</style>");
    }
    if let Some(title) = &m.title {
        doc.push_str("<title>");
        escape_text(title, &mut doc);
        doc.push_str("</title>");
        push_meta_prop(&mut doc, "og:title", title);
    }
    if let Some(desc) = &m.description {
        push_meta_name(&mut doc, "description", desc);
        push_meta_prop(&mut doc, "og:description", desc);
    }
    if let Some(img) = &m.og_image {
        push_meta_prop(&mut doc, "og:image", img);
    }
    if let Some(url) = &m.canonical_url {
        doc.push_str("<link rel=\"canonical\" href=\"");
        escape_attr(url, &mut doc);
        doc.push_str("\">");
    }
    // Caller-supplied head injection (favicon `<link>` tags today;
    // any other build-time-baked head metadata as it lands). Spliced
    // verbatim right before `</head>` so it sits after the framework-
    // owned tags (charset / viewport / preloads / styles / title) and
    // can override them by source order if it needs to.
    if let Some(snippet) = extra_head {
        if !snippet.is_empty() {
            doc.push_str(snippet);
        }
    }
    // Embed the viewport this page was rendered at, so a hydrating client
    // can seed the IDENTICAL value before its first render — making the
    // client's initial tree match the server's (clean DOM adoption) — then
    // read the real viewport and reactively reconcile. Without this, a
    // mobile client would render different nodes than the 1280px server
    // and adoption would diverge. See `WebBackend::hydrate`.
    doc.push_str("</head>\n<body><div id=\"app\" data-ssr-viewport=\"");
    doc.push_str(&format!("{}x{}", SSR_VIEWPORT.width as i32, SSR_VIEWPORT.height as i32));
    doc.push_str("\">");
    doc.push_str(&page.html);
    doc.push_str("</div>");
    if let Some(module) = bundle_module {
        doc.push_str("<script type=\"module\">import init from \"");
        doc.push_str(module);
        doc.push_str("\";init();</script>");
    }
    doc.push_str("</body>\n</html>");
    doc
}

/// Default viewport SSR assumes (desktop-ish). Responsive author code
/// reads `viewport_size()`; a server has no real window, so we pick a
/// sensible wide default for first paint.
const SSR_VIEWPORT: runtime_shared::ViewportSize = runtime_shared::ViewportSize::new(1280.0, 800.0);

/// Result of crawling an app's navigator hierarchy via [`render_all`].
/// `pages` maps each rendered literal path (e.g. `/`, `/about`) to its
/// [`RenderedPage`]. `skipped_parameterized` lists any patterns with
/// `:placeholder` segments that the crawler skipped — those need an
/// explicit list of param values to be SSG'd (future work).
pub struct CrawlResult {
    pub pages: HashMap<String, RenderedPage>,
    pub skipped_parameterized: Vec<&'static str>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Color, Tokenized};
    // The rendering mechanism now lives on the capability traits (the
    // `Backend` mega-trait is gone), so the unit tests reach it through
    // them; `Host` supplies the structural ops.
    use runtime_scene::Host;
    use runtime_vocabulary::caps::{
        AppEnvOps, AssetOps, DocumentOps, ExternalOps, LifecycleOps, LinkOps, PressableOps,
        StyleOps, TextOps, ViewOps,
    };
    use runtime_shared::StyleRules;

    /// A `view { text }` realized through the real vocabulary handlers
    /// serializes to nested `<div><span>` markup — proving headless
    /// server execution of the author tree produces HTML end to end.
    #[test]
    fn view_with_text_renders_nested_html() {
        let html = newcore::render_to_string(|| {
            use runtime_vocabulary::builders::{text, view};
            view().child(text().content("hi")).build()
        });
        assert_eq!(html, "<div><span>hi</span></div>");
    }

    /// A styled-text node serializes to the nested-span structure the
    /// live web backend builds (outer `<span>`, one child `<span>` per
    /// run) with each styled run's deltas as an inline style whose
    /// tokenized colors emit as `var(--token, fallback)` — so the SSR
    /// first paint is theme-correct and hydration adopts the DOM
    /// as-is.
    #[test]
    fn styled_text_renders_run_spans_with_inline_var_styles() {
        use runtime_shared::styled_text::{TextRun, TextRunStyle};
        let runs = vec![
            TextRun::plain("the "),
            TextRun::styled(
                "ui!",
                TextRunStyle {
                    font_family: Some(runtime_shared::FontFamily::System(
                        "ui-monospace, monospace".into(),
                    )),
                    background: Some(Tokenized::token("color-surface-alt", Color("#eee".into()))),
                    ..Default::default()
                },
            ),
            TextRun::plain(" macro"),
        ];
        let html = newcore::render_to_string(move || {
            runtime_vocabulary::builders::text().runs(runs).build()
        });
        assert_eq!(
            html,
            "<span><span>the </span>\
             <span style=\"font-family: ui-monospace, monospace; \
             background-color: var(--color-surface-alt, #eee)\">ui!</span>\
             <span> macro</span></span>",
        );
    }

    /// Text content is HTML-escaped so author strings can't inject
    /// markup into the server-rendered page.
    #[test]
    fn text_content_is_escaped() {
        let html = newcore::render_to_string(|| {
            runtime_vocabulary::builders::text().content("a<b>&c").build()
        });
        assert_eq!(html, "<span>a&lt;b&gt;&amp;c</span>");
    }

    /// `apply_style` stamps a content-keyed class (`ui-<hash>`) on the
    /// node and records ONE matching rule in the head stylesheet — the
    /// same class+rule model the web backend uses, not an inline style.
    /// The class name is `css::hash_class_name(content_key)`, identical to
    /// web, and nodes sharing a style share one class/rule (dedup).
    #[test]
    fn apply_style_emits_class_and_rule() {
        let mut b = SsrBackend::new();
        let mut rules = StyleRules::default();
        rules.background = Some(Tokenized::Literal(Color("#ff0000".into())));
        let rules = Rc::new(rules);
        let expected_class = css::hash_class_name(&rules.content_key());

        let v1 = b.create_view(&AccessibilityProps::default());
        let v2 = b.create_view(&AccessibilityProps::default());
        b.apply_style(&v1, &rules);
        b.apply_style(&v2, &rules); // same style → same class, one rule

        // Both nodes carry the identical class; no inline style attr.
        let html_v1 = { let mut s = String::new(); serialize(&v1, &mut s); s };
        assert_eq!(html_v1, format!(r#"<div class="{expected_class}"></div>"#));

        // Exactly one deduped rule in the head stylesheet.
        let head = b.head_css();
        let rule = format!(".{expected_class}{{background: #ff0000}}");
        assert!(head.contains(&rule), "expected one rule {rule}, got: {head}");
        assert_eq!(head.matches(&format!(".{expected_class}{{")).count(), 1, "deduped");
        // Base reset is always present.
        assert!(head.contains("box-sizing: border-box"), "base reset present, got: {head}");
    }

    /// `shadow` is the BOX shadow on every node kind, and `text_shadow`
    /// is the glyph shadow — one field per CSS property. A text and a
    /// box node with identical rules therefore SHARE one class (the old
    /// single `shadow` field lowered per node kind, which forced
    /// shadowed text onto distinct `@t`-keyed classes and disqualified
    /// every shadowed sheet from preminting).
    #[test]
    fn shadow_fields_lower_one_property_each_and_classes_are_shared() {
        use runtime_shared::Shadow;
        let mut b = SsrBackend::new();
        let mut rules = StyleRules::default();
        rules.shadow = Some(Shadow { x: 1.0, y: 2.0, blur: 3.0, color: Color("#000000".into()) });
        rules.text_shadow =
            Some(Shadow { x: 4.0, y: 5.0, blur: 6.0, color: Color("#111111".into()) });
        let rules = Rc::new(rules);

        let txt = b.create_text("hi", &AccessibilityProps::default());
        let boxed = b.create_view(&AccessibilityProps::default());
        b.apply_style(&txt, &rules);
        b.apply_style(&boxed, &rules);

        // ONE shared content-keyed class for both node kinds.
        let class = css::hash_class_name(&rules.content_key());
        let txt_html = { let mut s = String::new(); serialize(&txt, &mut s); s };
        let box_html = { let mut s = String::new(); serialize(&boxed, &mut s); s };
        assert!(txt_html.contains(&class), "text node shares the class: {txt_html}");
        assert!(box_html.contains(&class), "box node shares the class: {box_html}");

        // The one rule body carries BOTH properties, each from its own field.
        let head = b.head_css();
        assert!(
            head.contains("box-shadow: 1px 2px 3px #000000"),
            "shadow → box-shadow, got: {head}"
        );
        assert!(
            head.contains("text-shadow: 4px 5px 6px #111111"),
            "text_shadow → text-shadow, got: {head}"
        );
        assert_eq!(head.matches(&format!(".{class}{{")).count(), 1, "one shared rule");
    }

    /// `apply_styled_states` emits the base rule plus a `:hover` pseudo
    /// rule, so hover styling works on the static first paint with no JS.
    #[test]
    fn apply_styled_states_emits_hover_rule() {
        use runtime_shared::StateBits;
        let mut b = SsrBackend::new();
        let v = b.create_view(&AccessibilityProps::default());
        let mut base = StyleRules::default();
        base.background = Some(Tokenized::Literal(Color("#ffffff".into())));
        let mut hover = StyleRules::default();
        hover.background = Some(Tokenized::Literal(Color("#eeeeee".into())));
        b.apply_styled_states(
            &v,
            &Rc::new(base),
            &[(StateBits::HOVERED, Rc::new(hover))],
        );
        let html = { let mut s = String::new(); serialize(&v, &mut s); s };
        let class = html
            .split("class=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap()
            .to_string();
        let head = b.head_css();
        assert!(head.contains(&format!(".{class}{{background: #ffffff}}")), "base rule, got: {head}");
        assert!(head.contains(&format!(".{class}:hover{{background: #eeeeee}}")), "hover rule, got: {head}");
    }

    /// REGRESSION: re-applying a style to a node REPLACES its minted
    /// class (the web backend's one-mutable-dynamic-slot-per-node
    /// model) instead of accumulating both. The bug: a reactive style
    /// whose value changed between the mount-time apply and the
    /// request's flush (the new core stages writes — e.g. a scroll-spy
    /// TOC link resolving its `active` variant post-mount) serialized
    /// wearing BOTH classes, while old-core SSR (and the live DOM
    /// after the same flush) carries only the final one — breaking SSG
    /// byte-parity and hydration adoption. Structural classes stamped
    /// via `attach_html_class` must survive the swap.
    #[test]
    fn regression_reapply_replaces_minted_class_keeps_structural() {
        let mut b = SsrBackend::new();
        let v = b.create_view(&AccessibilityProps::default());
        b.attach_html_class(&v, "ui-cq-container");

        let mut first = StyleRules::default();
        first.background = Some(Tokenized::Literal(Color("#111111".into())));
        let mut second = StyleRules::default();
        second.background = Some(Tokenized::Literal(Color("#222222".into())));
        let first = Rc::new(first);
        let second = Rc::new(second);
        let first_class = css::hash_class_name(&first.content_key());
        let second_class = css::hash_class_name(&second.content_key());

        b.apply_style(&v, &first);
        b.apply_style(&v, &second);

        let html = { let mut s = String::new(); serialize(&v, &mut s); s };
        assert!(
            html.contains(&format!("class=\"ui-cq-container {second_class}\"")),
            "node wears the structural class + ONLY the final minted class: {html}"
        );
        assert!(
            !html.contains(&first_class),
            "the superseded minted class must be removed: {html}"
        );

        // The superseded rule leaves the head stylesheet once nothing
        // wears it (web's dynamic-slot refcount model): old-core SSR
        // applies only the final style, so a lingering first-apply rule
        // breaks head_css byte-parity.
        let head = b.head_css();
        assert!(
            !head.contains(&first_class),
            "superseded rule must be dropped from head_css: {head}"
        );
        assert!(head.contains(&second_class), "final rule stays: {head}");

        // Same contract through the variants entry (the path reactive
        // sheet styles actually take).
        let v2 = b.create_view(&AccessibilityProps::default());
        b.apply_styled_variants(&v2, &first, &[], &[], &[]);
        b.apply_styled_variants(&v2, &second, &[], &[], &[]);
        let html2 = { let mut s = String::new(); serialize(&v2, &mut s); s };
        assert!(
            html2.contains(&second_class) && !html2.contains(&first_class),
            "apply_styled_variants re-apply swaps the minted class too: {html2}"
        );

        // Shared classes survive a swap by ONE of their wearers: v2
        // still wears `second`; re-applying `first` to a third node and
        // back must not delete `second`'s rule while v2 wears it.
        let v3 = b.create_view(&AccessibilityProps::default());
        b.apply_style(&v3, &second); // second: worn by v, v2, v3
        b.apply_style(&v3, &first); // v3 swaps away; second still worn
        let head = b.head_css();
        assert!(
            head.contains(&second_class),
            "rule with remaining wearers must survive another node's swap: {head}"
        );
    }

    /// A disabled-state overlay must select on the `[disabled]`
    /// ATTRIBUTE, not the `:disabled` pseudo-class — pressables render
    /// as `<div disabled>`, which `:disabled` never matches (it only
    /// matches real form controls), so the overlay silently never
    /// applied in SSR output. Web already used `[disabled]`; this
    /// pins SSR to the shared `css::state_pseudo` fix.
    #[test]
    fn regression_ssr_disabled_overlay_uses_attribute_selector() {
        use runtime_shared::StateBits;
        let mut b = SsrBackend::new();
        let v = b.create_view(&AccessibilityProps::default());
        let mut base = StyleRules::default();
        base.background = Some(Tokenized::Literal(Color("#ffffff".into())));
        let mut disabled = StyleRules::default();
        disabled.background = Some(Tokenized::Literal(Color("#cccccc".into())));
        b.apply_styled_states(
            &v,
            &Rc::new(base),
            &[(StateBits::DISABLED, Rc::new(disabled))],
        );
        let head = b.head_css();
        assert!(
            head.contains("[disabled]{background: #cccccc}"),
            "disabled overlay must use the attribute selector, got: {head}"
        );
        assert!(
            !head.contains(":disabled"),
            "the :disabled pseudo-class is inert on div pressables, got: {head}"
        );
    }

    /// A focused-state overlay owns the focus indicator, so the SSR
    /// rule must suppress the UA outline exactly like the web backend's
    /// minted rule — otherwise the server-rendered first paint
    /// double-draws the native ring under the themed one until
    /// hydration replaces the sheet.
    #[test]
    fn regression_ssr_focus_overlay_suppresses_ua_outline() {
        use runtime_shared::StateBits;
        let mut b = SsrBackend::new();
        let v = b.create_view(&AccessibilityProps::default());
        let mut base = StyleRules::default();
        base.background = Some(Tokenized::Literal(Color("#ffffff".into())));
        let mut focused = StyleRules::default();
        focused.background = Some(Tokenized::Literal(Color("#ddddff".into())));
        b.apply_styled_states(
            &v,
            &Rc::new(base),
            &[(StateBits::FOCUSED, Rc::new(focused))],
        );
        let head = b.head_css();
        assert!(
            head.contains(":focus{outline:none;background: #ddddff}"),
            "focus overlay must prepend outline:none, got: {head}"
        );
    }

    /// `apply_styled_variants` emits the base rule plus an
    /// `@media (min-width: …)` rule per breakpoint overlay — so the SSR
    /// first paint already respects size boundaries (the whole point:
    /// a mobile request gets the mobile layout in static HTML). This is
    /// the SSR fix for the "sidebar pinned on initial mobile render" bug.
    #[test]
    fn apply_styled_variants_emits_breakpoint_media_rule() {
        use runtime_shared::{Breakpoint, Length};
        let mut b = SsrBackend::new();
        let v = b.create_view(&AccessibilityProps::default());

        // Mobile-first base width; widen at md and again at lg. Pass the
        // overlays out of rank order to prove emission order is by rank,
        // not call order.
        let mut base = StyleRules::default();
        base.width = Some(Tokenized::Literal(Length::Px(100.0)));
        let mut md = StyleRules::default();
        md.width = Some(Tokenized::Literal(Length::Px(500.0)));
        let mut lg = StyleRules::default();
        lg.width = Some(Tokenized::Literal(Length::Px(900.0)));

        b.apply_styled_variants(
            &v,
            &Rc::new(base),
            &[],
            &[
                (Breakpoint::Lg, Rc::new(lg)),
                (Breakpoint::Md, Rc::new(md)),
            ],
            &[],
        );

        let html = { let mut s = String::new(); serialize(&v, &mut s); s };
        let class = html
            .split("class=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap()
            .to_string();

        let head = b.head_css();
        // Base (mobile) rule.
        assert!(head.contains(&format!(".{class}{{width: 100px}}")), "base rule, got: {head}");
        // One @media rule per overlay, wrapping the same class.
        assert!(
            head.contains(&format!("@media (min-width: 768px) {{ .{class} {{ width: 500px }} }}")),
            "md media rule, got: {head}"
        );
        assert!(
            head.contains(&format!("@media (min-width: 1024px) {{ .{class} {{ width: 900px }} }}")),
            "lg media rule, got: {head}"
        );
        // Mobile-first cascade: the md (768) rule must precede the lg
        // (1024) rule in source so higher breakpoints win at matching widths.
        let md_at = head.find("min-width: 768px").expect("md present");
        let lg_at = head.find("min-width: 1024px").expect("lg present");
        assert!(md_at < lg_at, "md media rule must come before lg (ascending). head: {head}");
        // And both media rules come AFTER the base class rule.
        let base_at = head.find(&format!(".{class}{{width: 100px}}")).expect("base present");
        assert!(base_at < md_at, "base class rule must precede the media rules. head: {head}");
    }

    /// `apply_styled_variants` emits an `@container (min-width: …)` rule per
    /// container overlay (ascending by threshold), and `mark_container`
    /// tags the containment context with `container-type: inline-size` — so
    /// the SSR first paint already carries the container-responsive layout.
    #[test]
    fn apply_styled_variants_emits_container_query_rule() {
        use runtime_shared::Length;
        let mut b = SsrBackend::new();
        let container = b.create_view(&AccessibilityProps::default());
        b.mark_container(&container);
        let v = b.create_view(&AccessibilityProps::default());

        let mut base = StyleRules::default();
        base.width = Some(Tokenized::Literal(Length::Px(100.0)));
        let mut small = StyleRules::default();
        small.width = Some(Tokenized::Literal(Length::Px(500.0)));
        let mut large = StyleRules::default();
        large.width = Some(Tokenized::Literal(Length::Px(900.0)));

        // Pass out of threshold order to prove emission sorts ascending.
        b.apply_styled_variants(
            &v,
            &Rc::new(base),
            &[],
            &[],
            &[(600.0, Rc::new(large)), (300.0, Rc::new(small))],
        );

        let html = { let mut s = String::new(); serialize(&v, &mut s); s };
        let class = html
            .split("class=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap()
            .to_string();

        let head = b.head_css();
        assert!(head.contains(&format!(".{class}{{width: 100px}}")), "base rule, got: {head}");
        assert!(
            head.contains(&format!(
                "@container (min-width: 300px) {{ .{class} {{ width: 500px }} }}"
            )),
            "300px container rule, got: {head}"
        );
        assert!(
            head.contains(&format!(
                "@container (min-width: 600px) {{ .{class} {{ width: 900px }} }}"
            )),
            "600px container rule, got: {head}"
        );
        // Mobile-first cascade: lower threshold precedes higher in source.
        let small_at = head.find("min-width: 300px").expect("300 present");
        let large_at = head.find("min-width: 600px").expect("600 present");
        assert!(small_at < large_at, "300px rule must precede 600px (ascending). head: {head}");
        // The containment context carries `container-type: inline-size`.
        assert!(
            head.contains(&format!(".{} {{ container-type: inline-size }}", css::CONTAINER_TYPE_CLASS))
                || head.contains(&format!(".{}{{container-type: inline-size}}", css::CONTAINER_TYPE_CLASS)),
            "container-type rule present, got: {head}"
        );
        // And the container node wears the marker class.
        let chtml = { let mut s = String::new(); serialize(&container, &mut s); s };
        assert!(chtml.contains(css::CONTAINER_TYPE_CLASS), "container node wears marker class, got: {chtml}");
    }

    /// `create_pressable` is a clickable `<div>` with button a11y,
    /// matching the web pressable. It carries NO inline cursor — `cursor`
    /// is an author/component style property now (`StyleRules::cursor`),
    /// so the static paint and the live web pressable agree (neither
    /// stamps an inline cursor) and hydration adoption stays consistent.
    #[test]
    fn create_pressable_has_role_and_no_inline_cursor() {
        let mut b = SsrBackend::new();
        let node = b.create_pressable(Rc::new(|| {}), &AccessibilityProps::default());
        let html = { let mut s = String::new(); serialize(&node, &mut s); s };
        assert!(
            !html.contains("cursor"),
            "no framework-imposed inline cursor (it's a style property now), got: {html}"
        );
        assert!(html.contains(r#"role="button""#), "button role, got: {html}");
        assert!(html.contains(r#"tabindex="0""#), "focusable, got: {html}");
    }

    /// A reactive `when`/`switch`/`each` anchor is `display: contents`
    /// (layout-transparent), matching web — so a `position: sticky` child
    /// keeps the real parent as its containing block and keeps sticking.
    #[test]
    fn reactive_anchor_is_display_contents() {
        let mut b = SsrBackend::new();
        let node = b.create_anchor();
        let html = { let mut s = String::new(); serialize(&node, &mut s); s };
        assert_eq!(html, r#"<div style="display: contents"></div>"#);
    }

    /// REGRESSION: `create_element("canvas")` must emit a real `<canvas>`, not
    /// the `div` fallback. This is the enabler for GPU-external SSR: an SSR
    /// external handler emits the host `<canvas>` the web graphics primitive
    /// adopts via `hydrate_next("canvas")`. Before the fix `"canvas"` fell to
    /// the `_ => "div"` arm, so SSR served a `<div>` the client couldn't adopt
    /// (tag mismatch → the first hydration divergence + cascade).
    #[test]
    fn create_element_canvas_emits_canvas_not_div() {
        let mut b = SsrBackend::new();
        let node = b.create_element("canvas");
        let html = { let mut s = String::new(); serialize(&node, &mut s); s };
        assert_eq!(html, "<canvas></canvas>");
        // Contrast: an unknown tag still falls back to div (the intern is
        // deliberate, not a blanket passthrough).
        let div = b.create_element("marquee");
        let dhtml = { let mut s = String::new(); serialize(&div, &mut s); s };
        assert_eq!(dhtml, "<div></div>");
    }

    /// REGRESSION (before/after) — a GPU external (canvas) SSR-renders its HOST
    /// element only when its scene handler is registered. This is exactly the
    /// mechanism `canvas_core::register_ssr` uses (a local marker prim
    /// stands in here so `backend-ssr` stays SDK-free):
    ///
    /// * BEFORE (no handler): the caps fallback `create_external` emits
    ///   `<div>` — the tag the `<canvas>`-expecting web `graphics` primitive
    ///   can't adopt, so hydration diverges at the first node and cascades.
    ///   (This is also the whole reason `create_external` stays overridden:
    ///   the caps DEFAULT would `unimplemented!()`-panic.)
    /// * AFTER (registered handler): SSR emits the real `<canvas>` the client
    ///   adopts via `hydrate_next("canvas")`; the GPU surface attaches
    ///   post-hydration.
    #[test]
    fn external_gpu_host_ssr_renders_canvas_only_with_handler() {
        use std::any::{Any, TypeId};
        use std::rc::Rc;

        struct GpuCanvas; // stand-in for canvas_core::CanvasProps

        // BEFORE: nothing registered → the generic `<div>` fallback.
        let mut b = SsrBackend::new();
        let payload: Rc<dyn Any> = Rc::new(GpuCanvas);
        let before = b.create_external(
            TypeId::of::<GpuCanvas>(),
            "gpu-canvas",
            &payload,
            &AccessibilityProps::default(),
        );
        let before_html = { let mut s = String::new(); serialize(&before, &mut s); s };
        assert_eq!(before_html, "<div></div>", "no SSR handler → the un-adoptable div");

        // AFTER: register the host handler on the scene registry (what
        // `canvas_core::register_ssr` does) and render through it.
        let html = newcore::render_path_with(
            "/",
            |registry| {
                registry.register::<GpuCanvas, _>(|cx, _payload, _children| {
                    cx.backend().borrow_mut().create_element("canvas")
                });
            },
            || runtime_scene::item(GpuCanvas, Vec::new()),
        );
        assert_eq!(html.html, "<canvas></canvas>", "host handler → adoptable <canvas>");
    }

    /// `create_link` resets the browser's anchor defaults (so links don't
    /// render blue/underlined) — same inline reset the web link uses.
    #[test]
    fn create_link_applies_anchor_reset() {
        use runtime_shared::primitives::link::LinkConfig;
        let mut b = SsrBackend::new();
        let config = LinkConfig {
            route: "about",
            url: "/about".to_string(),
            external: false,
            on_activate: Rc::new(|| {}),
        };
        let node = b.create_link(config, &AccessibilityProps::default());
        let html = { let mut s = String::new(); serialize(&node, &mut s); s };
        assert!(html.contains("text-decoration: none"), "anchor reset, got: {html}");
        assert!(html.contains(r#"href="/about""#), "href, got: {html}");
    }

    /// `render_document` wraps the body + metadata into a full HTML doc:
    /// `<head>` carries the title / description / OG tags unfurlers read,
    /// the body holds the SSR markup in `#app`, and a module script loads
    /// the real bundle.
    #[test]
    fn render_document_emits_head_and_shell() {
        let page = RenderedPage {
            html: "<div>hi</div>".into(),
            metadata: runtime_shared::PageMetadata {
                title: Some("Home".into()),
                description: Some("welcome".into()),
                og_image: Some("/og.png".into()),
                ..Default::default()
            },
            head_css: ".x{color:red}".into(),
        };
        let doc = render_document(&page, Some("/pkg/app.js"), None);

        assert!(doc.starts_with("<!DOCTYPE html>"));
        assert!(doc.contains("<title>Home</title>"));
        assert!(doc.contains(r#"<meta property="og:title" content="Home">"#));
        assert!(doc.contains(r#"<meta name="description" content="welcome">"#));
        assert!(doc.contains(r#"<meta property="og:image" content="/og.png">"#));
        assert!(doc.contains(r#"<div id="app" data-ssr-viewport="1280x800"><div>hi</div></div>"#));
        assert!(doc.contains(r#"import init from "/pkg/app.js";init();"#));
        // Registered stylesheets are emitted in <head>.
        assert!(doc.contains("<style>.x{color:red}</style>"));
    }

    /// With no bundle module, the document is the rendered screen only —
    /// no `<script>` (pure SSR preview / SEO, no hydration).
    #[test]
    fn render_document_without_bundle_omits_script() {
        let page = RenderedPage {
            html: "<div>hi</div>".into(),
            metadata: Default::default(),
            head_css: String::new(),
        };
        let doc = render_document(&page, None, None);
        assert!(doc.contains(r#"<div id="app" data-ssr-viewport="1280x800"><div>hi</div></div>"#));
        assert!(!doc.contains("<script"), "no bundle => no script, got: {doc}");
    }

    /// `font_src_urls` extracts ONLY `@font-face` `src:url(…)` — never a
    /// class rule's `background-image:url(…)` — so we preload fonts and
    /// nothing else.
    #[test]
    fn font_src_urls_extracts_only_font_face_srcs() {
        let css = "@font-face{font-weight:400;src:url(\"/fonts/A.ttf\") format(\"truetype\");}\
                   @font-face{font-weight:700;src:url(\"/fonts/B.ttf\");}\
                   .c{background-image:url(\"/img/x.png\")}";
        assert_eq!(font_src_urls(css), vec!["/fonts/A.ttf", "/fonts/B.ttf"]);
    }

    /// `extra_head` lets callers (the SSR wrapper binary, today) splice
    /// favicon `<link>` tags — baked at build time from
    /// `[package.metadata.idealyst.app.icon]` — into the document so
    /// every SSR-rendered page references the same icon set the
    /// `static_dir` path serves. The injection lands inside `<head>`,
    /// before the closing tag, after the framework-owned metadata.
    #[test]
    fn render_document_splices_extra_head_inside_head() {
        let page = RenderedPage {
            html: "<div>hi</div>".into(),
            metadata: Default::default(),
            head_css: String::new(),
        };
        let snippet =
            r#"<link rel="icon" type="image/x-icon" href="/favicon.ico" sizes="16x16">"#;
        let doc = render_document(&page, None, Some(snippet));
        assert!(
            doc.contains(snippet),
            "extra_head snippet must appear in the document, got: {doc}",
        );
        let snippet_pos = doc.find(snippet).unwrap();
        let head_close = doc.find("</head>").unwrap();
        assert!(
            snippet_pos < head_close,
            "extra_head must land BEFORE </head> (got snippet at {snippet_pos}, </head> at {head_close})",
        );
    }

    /// Empty `extra_head` must produce no change — equivalent to
    /// passing `None`. Lets the SSR wrapper always emit
    /// `Some(EXTRA_HEAD.to_string())` without conditional code paths
    /// for projects without an icon block.
    #[test]
    fn render_document_empty_extra_head_is_noop() {
        let page = RenderedPage {
            html: "<div>hi</div>".into(),
            metadata: Default::default(),
            head_css: String::new(),
        };
        let with_none = render_document(&page, None, None);
        let with_empty = render_document(&page, None, Some(""));
        assert_eq!(with_none, with_empty);
    }

    /// `render_document` preloads each linked font (with `crossorigin` +
    /// type, before the stylesheet) so the web font is ready by first
    /// paint — but does NOT preload non-font URLs (background images).
    /// REGRESSION GUARD: the preload must carry `crossorigin` or it's
    /// ignored and the font is fetched twice (the bug we hit).
    #[test]
    fn render_document_preloads_fonts_not_other_urls() {
        let page = RenderedPage {
            html: "<div>hi</div>".into(),
            metadata: Default::default(),
            head_css: "@font-face{font-family:\"Inter\";font-weight:400;font-display:swap;\
                       src:url(\"/fonts/Inter-Regular.ttf\") format(\"truetype\");}\
                       .x{background-image:url(\"/img/bg.png\")}"
                .into(),
        };
        let doc = render_document(&page, Some("/pkg/app.js"), None);
        assert!(
            doc.contains(
                r#"<link rel="preload" as="font" crossorigin type="font/ttf" href="/fonts/Inter-Regular.ttf">"#
            ),
            "font preload (with crossorigin) missing; got: {doc}"
        );
        assert_eq!(
            doc.matches("rel=\"preload\"").count(),
            1,
            "exactly one preload — the font, not the background image"
        );
    }

    /// REGRESSION GUARD: SSR must NEVER render the body of an
    /// `Element::Lazy`. On native targets the chunk's loader resolves
    /// synchronously on first poll (the chunk's `async fn` is just a
    /// regular fn compiled into the binary) — so if the trait default
    /// (`true`) wins through, the server emits the chunk's body. The
    /// client then hydrates that body against its own client-side
    /// placeholder, the tag-classes diverge, and the whole subtree
    /// remounts (cratering things like the home page's wgpu Simulator).
    /// The fix is this override returning `false`; this test pins it.
    #[test]
    fn renders_lazy_chunks_returns_false() {
        let b = SsrBackend::new();
        assert!(
            !b.renders_lazy_chunks(),
            "SSR must keep Element::Lazy at the placeholder (server can't paint \
             a lazy body; the live client loads the chunk after hydrating)"
        );
    }

    /// Theme tokens delivered via `install_tokens` are emitted as a
    /// The premint host driver publishes the theme's default text font
    /// through `apply_default_text_font`; SSR must emit it as the
    /// `--iy-default-font` `:root` variable so preminted rule bodies
    /// (`font-family: var(--iy-default-font, inherit)`) resolve on the
    /// server-rendered first paint. `None` clears it.
    #[test]
    fn regression_ssr_default_text_font_emits_root_variable() {
        let mut b = SsrBackend::new();
        b.apply_default_text_font(Some(&runtime_shared::FontFamily::System("Inter".into())));
        let head = b.head_css();
        assert!(
            head.contains("--iy-default-font: Inter;"),
            "default font var missing from head css: {head}"
        );

        b.apply_default_text_font(None);
        let head = b.head_css();
        assert!(
            !head.contains("--iy-default-font"),
            "cleared default font must not emit: {head}"
        );
    }

    /// A node whose style is attached REACTIVELY never gets the theme
    /// font folded into its own rules — `style_attach`'s dynamic branch
    /// deliberately skips `fill_default_text_font`, because folding
    /// would change the minted class hash and break SSR/live class-name
    /// parity. Its only supply is CSS inheritance from the document
    /// root.
    ///
    /// Publishing only the `--iy-default-font` VARIABLE does not supply
    /// it: the variable is read exclusively by PREMINTED rule bodies
    /// (`font-family: var(--iy-default-font, inherit)`), and the base
    /// reset declares `font-family` only on `:where(input, textarea)`.
    /// So with the variable alone these nodes inherited past the root
    /// and rendered in the browser's serif fallback, while an
    /// identically-styled STATIC sibling — whose class carries the
    /// folded font — rendered in the theme font.
    ///
    /// The document root must therefore declare a real, inheritable
    /// `font-family`, not just the variable. See the dynamic-branch
    /// comment in `runtime_vocabulary::style_attach`.
    #[test]
    fn regression_reactive_styled_node_inherits_theme_font() {
        let mut b = SsrBackend::new();
        b.apply_default_text_font(Some(&runtime_shared::FontFamily::System(
            "Inter, sans-serif".into(),
        )));
        let head = b.head_css();

        // The `:root` block must declare an inheritable font-family, not
        // merely define the custom property.
        let root_block = head
            .split(":root {")
            .find(|s| s.contains("--iy-default-font"))
            .and_then(|s| s.split('}').next())
            .unwrap_or_default()
            .to_string();
        assert!(
            root_block.contains("font-family:"),
            "the :root block must declare an inheritable font-family so \
             reactively-styled nodes (which never fold it) inherit the theme \
             font instead of the browser serif fallback; got: {head}"
        );

        // And it must resolve to the theme font, not some other stack.
        assert!(
            root_block.contains("Inter, sans-serif"),
            "the inherited font must be the theme's: {head}"
        );

        // Clearing removes the declaration too — otherwise the root keeps
        // pointing at an undefined variable.
        b.apply_default_text_font(None);
        let head = b.head_css();
        assert!(
            !head.contains("--iy-default-font"),
            "cleared default font must not leave a dangling declaration: {head}"
        );
    }

    /// `:root { … }` block in `head_css`, so the SSR first paint resolves
    /// `var(--token, fallback)` to the real theme value (matching the
    /// live web build). `update_tokens` merges (changed tokens only).
    #[test]
    fn install_tokens_emits_root_variables() {
        use runtime_shared::{Length, TokenEntry, TokenValue};
        let mut b = SsrBackend::new();
        b.install_tokens(&[
            TokenEntry { name: "color-text", value: TokenValue::Color(Color("#1a1a1f".into())) },
            TokenEntry { name: "spacing-md", value: TokenValue::Length(Length::Px(16.0)) },
        ]);
        let head = b.head_css();
        assert!(head.contains(":root{"), "expected a :root block, got: {head}");
        assert!(head.contains("--color-text:#1a1a1f;"), "got: {head}");
        assert!(head.contains("--spacing-md:16px;"), "got: {head}");

        // A partial update changes one token and leaves the rest intact.
        b.update_tokens(&[TokenEntry {
            name: "color-text",
            value: TokenValue::Color(Color("#000000".into())),
        }]);
        let head = b.head_css();
        assert!(head.contains("--color-text:#000000;"), "update should apply, got: {head}");
        assert!(head.contains("--spacing-md:16px;"), "unchanged token should persist, got: {head}");
    }

    /// `set_app_background` + `set_scrollbar_theme` must emit the
    /// matching `<head>` CSS in `head_css`, and a `Tokenized::Token`
    /// must become `var(--<name>)` (not the resolved value) so the
    /// SSR-rendered page stays live-reactive to `update_tokens` swaps
    /// after hydration — matching what the web backend installs at
    /// runtime via `impl_set_app_background` /
    /// `impl_set_scrollbar_theme`. A `Tokenized::Literal` emits the
    /// raw color string verbatim.
    #[test]
    fn set_app_background_and_scrollbar_theme_emit_var_for_tokens() {
        use runtime_shared::Tokenized;
        let mut b = SsrBackend::new();
        b.set_app_background(&Tokenized::Token {
            name: "color-background",
            fallback: Color("#0f1115".into()),
        });
        b.set_scrollbar_theme(
            &Tokenized::Token {
                name: "color-border-strong",
                fallback: Color("#525868".into()),
            },
            &Tokenized::Literal(Color("transparent".into())),
        );
        let head = b.head_css();
        // Token reference → var(--…), so :root setProperty on swap
        // automatically repaints the body without a second SSR pass.
        assert!(
            head.contains("html, body { background: var(--color-background); }"),
            "expected body rule with var(--color-background), got: {head}"
        );
        // Scrollbar uses the same indirection for the thumb token;
        // the literal track stays as the raw `transparent` string.
        assert!(
            head.contains("scrollbar-color: var(--color-border-strong) transparent"),
            "expected scrollbar-color w/ var + literal, got: {head}"
        );
        assert!(
            head.contains("::-webkit-scrollbar-thumb { background: var(--color-border-strong)"),
            "expected webkit thumb to use var(--…), got: {head}"
        );

        // Literal-only call must NOT emit `var(--…)` — it should bake
        // the resolved color string in. Re-call replaces, not appends.
        b.set_app_background(&Tokenized::Literal(Color("#abcdef".into())));
        let head = b.head_css();
        assert!(
            head.contains("html, body { background: #abcdef; }"),
            "literal must bake the color value, got: {head}"
        );
        assert!(
            !head.contains("html, body { background: var("),
            "re-call must replace, not append, got: {head}"
        );
    }

    /// A registered typeface emits `@font-face` rules in `head_css`,
    /// linking the served font URL (matching the live web build) — so the
    /// SSR first paint uses the real font, not a fallback. `register_asset`
    /// must run first to supply the per-face served URL.
    #[test]
    fn register_typeface_emits_font_face_rules() {
        use runtime_shared::assets::{AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId};
        use runtime_shared::{FontStyle, FontWeight};
        let mut b = SsrBackend::new();
        let reg = AssetId(7);
        let bold = AssetId(8);
        b.register_asset(reg, AssetTag::Font, &AssetSource::Bundled { path: "fonts/Inter-Regular.ttf" });
        b.register_asset(bold, AssetTag::Font, &AssetSource::Bundled { path: "fonts/Inter-Bold.ttf" });
        b.register_typeface(
            TypefaceId(1),
            "Inter",
            &[
                TypefaceFace { weight: FontWeight::Normal, style: FontStyle::Normal, asset: reg, source: AssetSource::Bundled { path: "fonts/Inter-Regular.ttf" } },
                TypefaceFace { weight: FontWeight::Bold, style: FontStyle::Normal, asset: bold, source: AssetSource::Bundled { path: "fonts/Inter-Bold.ttf" } },
            ],
            SystemFallback::SansSerif,
        );
        let head = b.head_css();
        assert!(head.contains("@font-face{font-family:\"Inter\""), "expected @font-face, got: {head}");
        assert!(head.contains("src:url(\"/fonts/Inter-Regular.ttf\")"), "got: {head}");
        assert!(head.contains("src:url(\"/fonts/Inter-Bold.ttf\")"), "got: {head}");
        assert!(head.contains("font-weight:700"), "got: {head}");
    }
}
