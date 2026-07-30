//! Email-rendering backend — "SSG for emails."
//!
//! [`EmailBackend`] is a headless renderer like
//! [`backend-ssr`](https://docs.rs/backend-ssr), but tuned for email clients
//! instead of the browser. It realizes the SAME author scene every other
//! backend renders — once, synchronously, on the host (native) target — and
//! emits a self-contained, email-safe HTML document. No WASM, no browser, no
//! layout engine: email clients do their own layout from the HTML + inline
//! CSS.
//!
//! # Why a separate backend (vs. reusing SSR)
//!
//! SSR emits markup for a *browser* that will boot a wasm bundle: styles are
//! `ui-<hash>` classes in a `<head>` stylesheet, theme tokens are
//! `var(--token)` referencing a `:root` block, and `:hover`/`@media`/
//! `@container` overlays ride along. Email clients break every one of those
//! assumptions:
//!
//! * **Gmail strips `<style>`/`<head>` CSS.** So every style is baked
//!   **inline** on its node (`style="…"`), never a class.
//! * **No CSS custom properties.** So theme tokens are **resolved to literal
//!   values** at render time (via [`css::rules_to_css_resolved`]) against the
//!   theme installed through `caps::StyleOps::install_tokens` — never
//!   `var(--…)`.
//! * **No interaction, unreliable `@media`.** So state / breakpoint /
//!   container overlays are **dropped**; only the resolved base style emits.
//!
//! # Un-opinionated by design
//!
//! This backend maps primitives to HTML faithfully (`view`→`div`,
//! `text`→`span`, `image`→`img`, `link`→`a`) and takes **no stance** on
//! table-vs-flex layout. If an author puts flexbox on a `view`, that's what
//! ships (and that's on them in Outlook). Email-safe, bulletproof table
//! layout belongs in a component layer (`idea-ui-mail`) — the same way the
//! web backend stays layout-neutral and idea-ui owns the opinions.
//!
//! # Rendering surface
//!
//! [`newcore::render_email`] / [`newcore::render_email_with`] are the
//! entry points; the backend's capability implementation (the
//! `runtime_scene::Host` seam plus the 30 `runtime_vocabulary::caps`
//! traits) lives in [`newcore`]. This crate carried a second,
//! `Element`-walking `impl runtime_core::Backend` until the old core was
//! removed; every method body moved into the capability impls verbatim,
//! so the emitted HTML is unchanged — pinned against the old core's
//! frozen output by `tests/newcore_golden.rs` + `tests/goldens/`.

use runtime_shared::StyleRules;
use std::cell::RefCell;
use std::rc::Rc;

/// `runtime_scene::Host` + the 30 capability traits on [`EmailBackend`],
/// plus the one-shot-world render entries.
pub mod newcore;

/// A self-contained node handle — like a DOM node, not an arena index.
/// Children splice in via interior mutability (so a deferred reactive build
/// can attach into its slot without a backend reference).
pub type NodeRef = Rc<RefCell<HtmlNode>>;

/// One element in the accumulated HTML tree. Styles are stored as the
/// resolved `Rc<StyleRules>` and flattened to inline CSS at serialization
/// time, so token resolution always sees the FULLY-installed theme (a style
/// applied before `install_tokens` still resolves correctly).
pub struct HtmlNode {
    tag: &'static str,
    /// Text content for leaf text nodes (escaped at serialization time).
    text: Option<String>,
    /// A default inline style the primitive itself imposes (e.g. the link
    /// anchor reset). Emitted BEFORE author styles so author styles win.
    default_style: Option<&'static str>,
    /// Author styles applied via `apply_style` — resolved to inline CSS at
    /// serialize time. A `Vec` because a node can be re-styled; later entries
    /// override earlier via CSS source order.
    styles: Vec<Rc<StyleRules>>,
    /// Extra attributes (e.g. `src`, `alt`, `href`) as (name, value) pairs.
    attrs: Vec<(&'static str, String)>,
    children: Vec<NodeRef>,
}

impl HtmlNode {
    fn new(tag: &'static str) -> Self {
        Self {
            tag,
            text: None,
            default_style: None,
            styles: Vec::new(),
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }
}

fn nref(n: HtmlNode) -> NodeRef {
    Rc::new(RefCell::new(n))
}

/// Append an author style, deduping an immediate re-application of
/// value-identical rules. Reactive style delivery re-applies a node's
/// rules without the node having "changed" — the old core's state
/// re-fires and the new core's theme-cohort reapply (which re-delivers
/// every registered node's rules on a token install during mount) both
/// do this — and email's append-only model would otherwise duplicate
/// the inline CSS (`background: X; width: Y; background: X; width: Y`).
/// Visually harmless (CSS source order last-wins on equal rules) but it
/// bloats every email and breaks old/new byte parity
/// (`tests/newcore_golden.rs::corpus_tokens_and_dropped_overlays`).
/// A genuinely CHANGED style still appends; later entries keep winning
/// via source order.
fn push_style_dedup(node: &NodeRef, style: &Rc<StyleRules>) {
    let mut n = node.borrow_mut();
    if let Some(last) = n.styles.last() {
        if Rc::ptr_eq(last, style) || **last == **style {
            return;
        }
    }
    n.styles.push(style.clone());
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

/// Append a space-separated class (kept for parity with author
/// `attach_html_class` calls — harmless in email, most clients ignore it).
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

/// Compose a node's inline `style` value: its primitive default (if any),
/// then every author style with tokens resolved against `tokens`.
fn compose_style(n: &HtmlNode, tokens: &[runtime_shared::TokenEntry]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = n.default_style {
        if !d.is_empty() {
            parts.push(d.to_string());
        }
    }
    for style in &n.styles {
        let css = css::rules_to_css_resolved(style, tokens);
        if !css.is_empty() {
            parts.push(css);
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

fn serialize(node: &NodeRef, tokens: &[runtime_shared::TokenEntry], out: &mut String) {
    let n = node.borrow();
    out.push('<');
    out.push_str(n.tag);
    if let Some(style) = compose_style(&n, tokens) {
        out.push_str(" style=\"");
        escape_attr(&style, out);
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
        serialize(child, tokens, out);
    }
    out.push_str("</");
    out.push_str(n.tag);
    out.push('>');
}

/// HTML void elements have no closing tag and take no children.
fn is_void(tag: &str) -> bool {
    matches!(tag, "img" | "input" | "br" | "hr" | "meta" | "link")
}

/// Tags that introduce a line break when extracting the plaintext
/// alternative — so `text` inside stacked `view`s reads as separate lines.
fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "div" | "p" | "br" | "hr" | "tr" | "table" | "li" | "ul" | "ol" | "section" | "article"
            | "header" | "footer" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote"
    )
}

/// Walk the tree collecting text content, inserting a newline after each text
/// leaf and each block boundary — a best-effort `text/plain` alternative. Text
/// primitives lower to inline `<span>`s, so without the per-leaf break every
/// heading/paragraph/button label would run together on one line.
fn collect_text(node: &NodeRef, out: &mut String) {
    let n = node.borrow();
    let had_text = n.text.is_some();
    if let Some(text) = &n.text {
        out.push_str(text);
    }
    for child in &n.children {
        collect_text(child, out);
    }
    if (had_text || is_block_tag(n.tag)) && !out.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
}

/// Escape text content: `&`, `<`, `>`.
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

/// Escape a double-quoted attribute value.
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
// Microtask scheduler — identical rationale to backend-ssr: a reactive
// `when`/`each` (or deferred build) defers work via `schedule_microtask`,
// which must run AFTER the mount borrow is released, not inline (double
// borrow). We queue microtasks and drain them post-mount. Frames/timers are
// dropped: a static email has no animation loop.
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

    struct EmailScheduler;
    impl Scheduler for EmailScheduler {
        fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
            QUEUE.with(|q| q.borrow_mut().push_back(f));
        }
        fn after_animation_frame(&self, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
        fn after_ms(&self, _delay_ms: i32, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
        fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
    }

    pub(crate) fn ensure_installed() {
        if !runtime_shared::scheduling::is_scheduler_installed() {
            runtime_shared::scheduling::install_scheduler(Box::new(EmailScheduler));
        }
    }

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

/// Default inline style for a `link` primitive's `<a>`: strip the client's
/// blue/underlined anchor defaults so the author's styling shows through.
const LINK_RESET_STYLE: &str = "color: inherit; text-decoration: none";

#[derive(Default)]
pub struct EmailBackend {
    root: Option<NodeRef>,
    metadata: runtime_shared::PageMetadata,
    /// The active theme's tokens, captured from `install_tokens` /
    /// `update_tokens`. Every node's inline CSS resolves against this at
    /// serialize time (baked as literals — email has no CSS variables).
    tokens: Vec<runtime_shared::TokenEntry>,
    /// Host-surface background captured from `set_app_background`, resolved
    /// and applied to the document `<body>`.
    app_bg: Option<runtime_shared::Tokenized<runtime_shared::Color>>,
}

impl EmailBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Serialize the body tree to an HTML fragment (inline styles), rooted at
    /// the node passed to [`Backend::finish`]. Empty if nothing mounted.
    pub fn body_html(&self) -> String {
        let mut out = String::new();
        if let Some(root) = &self.root {
            serialize(root, &self.tokens, &mut out);
        }
        out
    }

    /// Best-effort `text/plain` alternative extracted from the tree.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        if let Some(root) = &self.root {
            collect_text(root, &mut out);
        }
        out.trim().to_string()
    }

    /// The resolved `<body>` background CSS value, if `set_app_background`
    /// was called (tokens baked to a literal — no `var(--…)`).
    fn body_bg(&self) -> Option<String> {
        self.app_bg.as_ref().map(|c| match c {
            runtime_shared::Tokenized::Literal(color) => color.0.clone(),
            runtime_shared::Tokenized::Token { name, fallback } => self
                .tokens
                .iter()
                .find(|e| e.name == *name)
                .and_then(|e| match &e.value {
                    runtime_shared::TokenValue::Color(c) => Some(c.0.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| fallback.0.clone()),
        })
    }
}


// ---------------------------------------------------------------------------
// Public render API
// ---------------------------------------------------------------------------

/// The output of rendering an email template: a complete, self-contained
/// HTML document ready to send as the `text/html` part, plus a best-effort
/// `text/plain` alternative and the subject (from the template's page
/// metadata title, if it set one).
pub struct RenderedEmail {
    /// A full `<!DOCTYPE html>` document with all styles inline — the
    /// `text/html` MIME part.
    pub html: String,
    /// Best-effort plaintext extracted from the tree — the `text/plain` part.
    pub text: String,
    /// The subject, if the template declared one via
    /// [`runtime_shared::set_page_metadata`] (title). `None` otherwise — the
    /// caller supplies the subject when sending.
    pub subject: Option<String>,
}

/// Viewport email templates are rendered at. Email is a narrow, fixed-width
/// medium; responsive author code that reads `viewport_size()` gets the
/// mobile-ish layout, which is the right default for a 600px email column.
const EMAIL_VIEWPORT: runtime_shared::ViewportSize = runtime_shared::ViewportSize::new(600.0, 800.0);

/// Wrap a rendered body fragment in a complete, email-safe HTML document.
///
/// The skeleton is deliberately minimal and layout-neutral: an
/// email-compatible doctype, `<meta charset>` + viewport, a
/// `color-scheme`/`x-apple-disable-message-reformatting` hint, an optional
/// `<title>`, and a tiny `<style>` carrying only the resets that can't be
/// inlined (`body` margin, image rendering). All content styling is inline on
/// the nodes themselves — no class rules, no `:root` variables.
fn email_document(body: &str, title: Option<&str>, body_bg: Option<String>) -> String {
    let mut doc = String::from(
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \
         \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd\">\n\
         <html xmlns=\"http://www.w3.org/1999/xhtml\" lang=\"en\">\n<head>",
    );
    doc.push_str("<meta charset=\"utf-8\"/>");
    doc.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\"/>");
    // Tell clients the mail supports both schemes (so they don't invert), and
    // stop iOS Mail from auto-resizing small text.
    doc.push_str("<meta name=\"color-scheme\" content=\"light dark\"/>");
    doc.push_str("<meta name=\"x-apple-disable-message-reformatting\"/>");
    if let Some(t) = title {
        doc.push_str("<title>");
        escape_text(t, &mut doc);
        doc.push_str("</title>");
    }
    // Only the resets that genuinely can't be inlined. Clients that strip
    // `<style>` (Gmail) still get correct content because everything else is
    // inline; this just improves the clients that keep it.
    doc.push_str(
        "<style>body{margin:0;padding:0;width:100%!important;\
         -webkit-text-size-adjust:100%;-ms-text-size-adjust:100%;}\
         img{border:0;line-height:100%;outline:none;text-decoration:none;-ms-interpolation-mode:bicubic;}\
         table{border-collapse:collapse!important;}</style>",
    );
    let body_style = match body_bg {
        Some(bg) => format!(" style=\"margin:0;padding:0;background:{}\"", bg),
        None => " style=\"margin:0;padding:0\"".to_string(),
    };
    doc.push_str("</head>\n<body");
    doc.push_str(&body_style);
    doc.push('>');
    doc.push_str(body);
    doc.push_str("</body>\n</html>");
    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Color, Length, StyleRules, TokenEntry, TokenValue, Tokenized};
    // The mechanism now lives on the capability traits (the `Backend`
    // mega-trait is gone), so the unit tests call it through them.
    use runtime_vocabulary::caps::{LifecycleOps, StyleOps, TextOps, ViewOps};

    /// `view { text }` realized through the real vocabulary handlers
    /// serializes to nested `<div><span>` markup — headless email
    /// rendering of the author tree end to end.
    #[test]
    fn view_with_text_renders_nested_html() {
        let out = newcore::render_email(|| {
            use runtime_vocabulary::builders::{text, view};
            view().child(text().content("hi")).build()
        });
        assert!(
            out.html.contains("<div><span>hi</span></div>"),
            "got: {}",
            out.html
        );
    }

    /// Author text is HTML-escaped so template strings can't inject markup.
    #[test]
    fn text_content_is_escaped() {
        let out = newcore::render_email(|| {
            use runtime_vocabulary::builders::text;
            text().content("a<b>&c").build()
        });
        assert!(
            out.html.contains("<span>a&lt;b&gt;&amp;c</span>"),
            "got: {}",
            out.html
        );
    }

    /// `apply_style` bakes the style INLINE on the node — no class, no head
    /// stylesheet — which is the whole point for email (Gmail strips `<style>`).
    #[test]
    fn apply_style_emits_inline_style_not_class() {
        let mut b = EmailBackend::new();
        let mut rules = StyleRules::default();
        rules.background = Some(Tokenized::Literal(Color("#ff0000".into())));
        let v = b.create_view(&AccessibilityProps::default());
        b.apply_style(&v, &Rc::new(rules));
        b.finish(v);
        let html = b.body_html();
        assert_eq!(html, r#"<div style="background: #ff0000"></div>"#);
        assert!(!html.contains("class="), "email must not use classes: {html}");
    }

    /// A tokenized style resolves to the INSTALLED theme value as a literal —
    /// never `var(--…)`. This is the core email-vs-web styling difference.
    #[test]
    fn tokenized_style_resolves_to_installed_literal() {
        let mut b = EmailBackend::new();
        b.install_tokens(&[TokenEntry {
            name: "color-brand",
            value: TokenValue::Color(Color("#6d28d9".into())),
        }]);
        let mut rules = StyleRules::default();
        rules.background = Some(Tokenized::token("color-brand", Color("#000000".into())));
        let v = b.create_view(&AccessibilityProps::default());
        b.apply_style(&v, &Rc::new(rules));
        b.finish(v);
        let html = b.body_html();
        assert!(html.contains("background: #6d28d9"), "got: {html}");
        assert!(!html.contains("var("), "no CSS variables in email: {html}");
    }

    /// Styles resolve against the theme installed AFTER the style was applied
    /// (resolution is deferred to serialize time) — so token/style ordering
    /// during mount never matters.
    #[test]
    fn tokens_installed_after_apply_style_still_resolve() {
        let mut b = EmailBackend::new();
        let mut rules = StyleRules::default();
        rules.color = Some(Tokenized::token("color-text", Color("#111111".into())));
        let v = b.create_view(&AccessibilityProps::default());
        b.apply_style(&v, &Rc::new(rules)); // applied BEFORE tokens exist
        b.install_tokens(&[TokenEntry {
            name: "color-text",
            value: TokenValue::Color(Color("#eeeeee".into())),
        }]);
        b.finish(v);
        assert!(b.body_html().contains("color: #eeeeee"), "got: {}", b.body_html());
    }

    /// Interaction-state / breakpoint / container overlays are DROPPED — only
    /// the resolved base style survives (email has no hover/media/container).
    #[test]
    fn state_and_breakpoint_overlays_are_dropped() {
        use runtime_shared::{Breakpoint, StateBits};
        let mut b = EmailBackend::new();
        let v = b.create_view(&AccessibilityProps::default());
        let mut base = StyleRules::default();
        base.background = Some(Tokenized::Literal(Color("#ffffff".into())));
        let mut hover = StyleRules::default();
        hover.background = Some(Tokenized::Literal(Color("#000000".into())));
        let mut wide = StyleRules::default();
        wide.width = Some(Tokenized::Literal(Length::Px(500.0)));
        b.apply_styled_variants(
            &v,
            &Rc::new(base),
            &[(StateBits::HOVERED, Rc::new(hover))],
            &[(Breakpoint::Md, Rc::new(wide))],
            &[],
        );
        b.finish(v);
        let html = b.body_html();
        assert!(html.contains("background: #ffffff"), "base kept: {html}");
        assert!(!html.contains("#000000"), "hover overlay dropped: {html}");
        assert!(!html.contains("500px"), "breakpoint overlay dropped: {html}");
        assert!(!html.contains("@media"), "no media queries in email: {html}");
    }

    /// Regression: re-applying value-identical rules to a node (what a
    /// reactive style re-delivery does — the theme-cohort reapply on an
    /// in-build token install) must NOT duplicate the inline CSS. A
    /// genuinely changed style still appends (later wins via CSS source
    /// order).
    #[test]
    fn reapplying_identical_style_does_not_duplicate_inline_css() {
        let mut b = EmailBackend::new();
        let v = b.create_view(&AccessibilityProps::default());
        let mk = |bg: &str| {
            Rc::new(StyleRules {
                background: Some(Tokenized::Literal(Color(bg.into()))),
                ..Default::default()
            })
        };
        // Same value, DIFFERENT Rc — the theme cohort re-resolves per apply.
        b.apply_style(&v, &mk("#ff0000"));
        b.apply_style(&v, &mk("#ff0000"));
        // Changed value appends.
        b.apply_style(&v, &mk("#00ff00"));
        b.finish(v);
        assert_eq!(
            b.body_html(),
            r#"<div style="background: #ff0000; background: #00ff00"></div>"#,
        );
    }

    /// `render_email` produces a complete, self-contained document and a
    /// plaintext alternative.
    #[test]
    fn render_email_wraps_full_document() {
        let out = newcore::render_email(|| {
            use runtime_vocabulary::builders::{text, view};
            view().child(text().content("Hello world")).build()
        });
        assert!(out.html.starts_with("<!DOCTYPE html"), "got: {}", out.html);
        assert!(out.html.contains("<meta charset=\"utf-8\"/>"));
        assert!(out.html.contains("Hello world"));
        assert!(out.html.contains("</html>"));
        assert_eq!(out.text, "Hello world");
    }

    /// A `link` primitive becomes an `<a href>` with the anchor reset inline
    /// and (for external) `target`/`rel`.
    #[test]
    fn link_renders_anchor_with_reset() {
        use runtime_shared::primitives::link::LinkConfig;
        use runtime_vocabulary::caps::LinkOps;
        let mut b = EmailBackend::new();
        let node = b.create_link(
            LinkConfig {
                route: "home",
                url: "https://example.com".to_string(),
                external: true,
                on_activate: Rc::new(|| {}),
            },
            &AccessibilityProps::default(),
        );
        b.finish(node);
        let html = b.body_html();
        assert!(html.contains(r#"href="https://example.com""#), "got: {html}");
        assert!(html.contains("text-decoration: none"), "anchor reset inline: {html}");
        assert!(html.contains(r#"target="_blank""#), "external link opens new tab: {html}");
    }

    /// `set_app_background` resolves onto the document `<body>` as a literal.
    #[test]
    fn app_background_applies_to_body() {
        use runtime_vocabulary::caps::AppEnvOps;
        let out = newcore::render_email_with(
            |b| b.set_app_background(&Tokenized::Literal(Color("#0b1020".into()))),
            || {
                use runtime_vocabulary::builders::{text, view};
                view().child(text().content("hi")).build()
            },
        );
        assert!(
            out.html.contains("background:#0b1020"),
            "body background applied, got: {}",
            out.html
        );
    }
}
