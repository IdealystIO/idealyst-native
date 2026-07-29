//! Email-rendering backend — "SSG for emails."
//!
//! [`EmailBackend`] is a headless [`Backend`](runtime_core::Backend), like
//! [`backend-ssr`](https://docs.rs/backend-ssr), but tuned for email clients
//! instead of the browser. It walks the SAME author `Element` tree every
//! other backend renders — once, synchronously, on the host (native) target
//! — and emits a self-contained, email-safe HTML document from a set of
//! input props. No WASM, no browser, no layout engine: email clients do their
//! own layout from the HTML + inline CSS.
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
//!   theme installed through [`Backend::install_tokens`] — never `var(--…)`.
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

//! # New core (`new-core` feature)
//!
//! The crate also carries the idea-lite (new-core) render leg:
//! [`newcore::render_email`] / [`newcore::render_email_with`] mirror the
//! old entries 1:1 but realize the tree on a fresh `runtime_world::World`
//! through the vocabulary's builtin handlers, with [`EmailBackend`]
//! implementing `runtime_scene::Host` + all 30 capability traits
//! directly. Additive — the old-core render path is untouched with or
//! without the feature, and `tests/newcore_golden.rs` pins the two legs
//! to byte-identical output for the same logical template.

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{Backend, StyleRules};
use std::cell::RefCell;
use std::rc::Rc;

// New-core (idea-lite) adoption: Host + the 30 capability traits on
// `EmailBackend`, plus the one-shot-world render entries. Additive —
// see the module docs.
#[cfg(feature = "new-core")]
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
fn compose_style(n: &HtmlNode, tokens: &[runtime_core::TokenEntry]) -> Option<String> {
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

fn serialize(node: &NodeRef, tokens: &[runtime_core::TokenEntry], out: &mut String) {
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
    use runtime_core::scheduling::{ScheduleHandle, Scheduler};
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
        if !runtime_core::scheduling::is_scheduler_installed() {
            runtime_core::scheduling::install_scheduler(Box::new(EmailScheduler));
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
    metadata: runtime_core::PageMetadata,
    /// The active theme's tokens, captured from `install_tokens` /
    /// `update_tokens`. Every node's inline CSS resolves against this at
    /// serialize time (baked as literals — email has no CSS variables).
    tokens: Vec<runtime_core::TokenEntry>,
    /// Host-surface background captured from `set_app_background`, resolved
    /// and applied to the document `<body>`.
    app_bg: Option<runtime_core::Tokenized<runtime_core::Color>>,
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
            runtime_core::Tokenized::Literal(color) => color.0.clone(),
            runtime_core::Tokenized::Token { name, fallback } => self
                .tokens
                .iter()
                .find(|e| e.name == *name)
                .and_then(|e| match &e.value {
                    runtime_core::TokenValue::Color(c) => Some(c.0.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| fallback.0.clone()),
        })
    }
}

impl Backend for EmailBackend {
    type Node = NodeRef;

    fn platform(&self) -> runtime_core::Platform {
        // Email is closest to the web surface (HTML/CSS output); author code
        // branching on `platform()` treats it like the web target.
        runtime_core::Platform::Web
    }

    /// Keep `Element::Lazy` at its placeholder — a static email can't paint
    /// lazy/GPU content, and there's no client to load the chunk later.
    fn renders_lazy_chunks(&self) -> bool {
        false
    }

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        nref(HtmlNode::new("div"))
    }

    fn create_element(&mut self, tag: &str) -> Self::Node {
        // Intern the structural tags an external/component might emit; unknown
        // tags fall back to `div`. Mirrors backend-ssr's tag set.
        let tag: &'static str = match tag {
            "p" => "p",
            "ul" => "ul",
            "ol" => "ol",
            "li" => "li",
            "blockquote" => "blockquote",
            "table" => "table",
            "thead" => "thead",
            "tbody" => "tbody",
            "tr" => "tr",
            "td" => "td",
            "th" => "th",
            "section" => "section",
            "article" => "article",
            "header" => "header",
            "footer" => "footer",
            "h1" => "h1",
            "h2" => "h2",
            "h3" => "h3",
            "h4" => "h4",
            "h5" => "h5",
            "h6" => "h6",
            "a" => "a",
            "br" => "br",
            "hr" => "hr",
            _ => "div",
        };
        nref(HtmlNode::new(tag))
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        let mut node = HtmlNode::new("span");
        node.text = Some(content.to_string());
        nref(node)
    }

    fn create_button(
        &mut self,
        label: &str,
        _on_click: &runtime_core::Action,
        _leading_icon: Option<&runtime_core::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_core::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // No JS in email: a "button" is just a styled inline box. Authors who
        // want a clickable CTA wrap a `link` (idea-ui-mail's Button does).
        let mut node = HtmlNode::new("span");
        node.text = Some(label.to_string());
        nref(node)
    }

    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("img");
        node.attrs.push(("src", src.to_string()));
        node.attrs.push(("alt", alt.unwrap_or("").to_string()));
        // Email images should not stretch; keep intrinsic unless styled.
        node.attrs.push(("border", "0".to_string()));
        nref(node)
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        // A reactive `when`/`switch`/`each` placeholder. `display: contents`
        // keeps it layout-transparent (matching web/SSR) — but email clients
        // vary on `display: contents`, so we instead emit a plain wrapper
        // with no box of its own via zero styling; children flow inside it.
        nref(HtmlNode::new("div"))
    }

    fn create_pressable(
        &mut self,
        _on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // No interaction in email; a pressable is just a container.
        nref(HtmlNode::new("div"))
    }

    fn create_icon(
        &mut self,
        data: &runtime_core::primitives::icon::IconData,
        color: Option<&runtime_core::Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Inline SVG. Support varies across email clients (Apple Mail yes,
        // Gmail/Outlook no) — the un-opinionated choice is to emit it and let
        // idea-ui-mail steer authors toward hosted `<img>` icons where it
        // matters. Same SVG shape backend-ssr emits.
        let (vw, vh) = data.view_box;
        let mut svg = HtmlNode::new("svg");
        svg.attrs.push(("viewBox", format!("0 0 {} {}", vw, vh)));
        svg.attrs.push(("xmlns", "http://www.w3.org/2000/svg".to_string()));
        svg.attrs.push(("width", "1em".to_string()));
        svg.attrs.push(("height", "1em".to_string()));
        let icon_color = color
            .map(|c| c.0.clone())
            .unwrap_or_else(|| "currentColor".to_string());
        if data.filled {
            svg.attrs.push(("fill", icon_color));
            svg.attrs.push(("stroke", "none".to_string()));
        } else {
            svg.attrs.push(("fill", "none".to_string()));
            svg.attrs.push(("stroke", icon_color));
            svg.attrs.push(("stroke-width", "2".to_string()));
            svg.attrs.push(("stroke-linecap", "round".to_string()));
            svg.attrs.push(("stroke-linejoin", "round".to_string()));
        }
        svg.default_style = Some("display:inline-block;vertical-align:middle;");
        let fill_rule = match data.fill_rule {
            runtime_core::primitives::icon::FillRule::NonZero => "nonzero",
            runtime_core::primitives::icon::FillRule::EvenOdd => "evenodd",
        };
        for path_d in data.paths {
            let mut path = HtmlNode::new("path");
            path.attrs.push(("d", (*path_d).to_string()));
            path.attrs.push(("fill-rule", fill_rule.to_string()));
            svg.children.push(nref(path));
        }
        nref(svg)
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        parent.borrow_mut().children.push(child);
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        let mut p = parent.borrow_mut();
        let index = index.min(p.children.len());
        p.children.insert(index, child);
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        node.borrow_mut().text = Some(content.to_string());
    }

    fn clear_children(&mut self, node: &Self::Node) {
        node.borrow_mut().children.clear();
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        // Store the resolved base style; flattened to inline CSS (tokens baked
        // to literals) at serialize time. NO class, NO head stylesheet.
        push_style_dedup(node, style);
    }

    // Email opts into the "native state" model only so the walker hands us the
    // base + overlays in one call (`apply_styled_states`) — we keep the base
    // and DROP the overlays. There is no `:hover`/`:active`/`:focus` in email.
    fn handles_states_natively(&self) -> bool {
        true
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        _overlays: &[(runtime_core::StateBits, Rc<StyleRules>)],
    ) {
        push_style_dedup(node, base);
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        _overlays: &[(runtime_core::StateBits, Rc<StyleRules>)],
        _breakpoint_overlays: &[(runtime_core::Breakpoint, Rc<StyleRules>)],
        _container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        // Email has no interaction and unreliable `@media`/`@container`
        // support — emit only the resolved base, drop every overlay.
        push_style_dedup(node, base);
    }

    fn create_link(
        &mut self,
        config: runtime_core::primitives::link::LinkConfig,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("a");
        node.default_style = Some(LINK_RESET_STYLE);
        node.attrs.push(("href", config.url.clone()));
        if config.external {
            node.attrs.push(("target", "_blank".to_string()));
            node.attrs.push(("rel", "noopener noreferrer".to_string()));
        }
        nref(node)
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _on_blur: Option<runtime_core::primitives::text_input::BlurHandler>,
        _secure: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Forms don't work in email; degrade to the current value as text.
        let mut node = HtmlNode::new("span");
        let shown = if initial_value.is_empty() {
            placeholder.unwrap_or("")
        } else {
            initial_value
        };
        node.text = Some(shown.to_string());
        nref(node)
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        node.borrow_mut().text = Some(value.to_string());
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        _wrap: bool,
        _min_rows: Option<u32>,
        _max_rows: Option<u32>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("div");
        let shown = if initial_value.is_empty() {
            placeholder.unwrap_or("")
        } else {
            initial_value
        };
        node.text = Some(shown.to_string());
        nref(node)
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        node.borrow_mut().text = Some(value.to_string());
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        _on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Render a static indicator glyph — checkboxes don't toggle in email.
        let mut node = HtmlNode::new("span");
        node.text = Some(if initial_value { "\u{2611}" } else { "\u{2610}" }.to_string());
        nref(node)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        node.borrow_mut().text =
            Some(if value { "\u{2611}" } else { "\u{2610}" }.to_string());
    }

    fn create_scroll_view(
        &mut self,
        _horizontal: bool,
        _on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // No scroll in email; a scroll_view is just a container.
        nref(HtmlNode::new("div"))
    }

    fn create_slider(
        &mut self,
        _initial_value: f32,
        _min: f32,
        _max: f32,
        _step: Option<f32>,
        _on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        nref(HtmlNode::new("div"))
    }

    fn create_activity_indicator(
        &mut self,
        _size: runtime_core::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&runtime_core::Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        nref(HtmlNode::new("div"))
    }

    fn create_virtualizer(
        &mut self,
        _callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        _layout: runtime_core::VirtualLayout,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Virtualized lists have no meaning in a static email; emit the
        // container only (authors render real rows with `for` for email).
        nref(HtmlNode::new("div"))
    }

    fn create_graphics(
        &mut self,
        _on_ready: runtime_core::primitives::graphics::OnReady,
        _on_resize: runtime_core::primitives::graphics::OnResize,
        _on_lost: runtime_core::primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // GPU canvas can't render into an email; emit an empty box.
        nref(HtmlNode::new("div"))
    }

    fn create_external(
        &mut self,
        _type_id: std::any::TypeId,
        _type_name: &'static str,
        _payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Third-party externals aren't email-rendered (no registry); emit an
        // empty host box. An email-specific external registry could be wired
        // later if a use case appears.
        nref(HtmlNode::new("div"))
    }

    fn create_portal(
        &mut self,
        _target: runtime_core::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        nref(HtmlNode::new("div"))
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        set_attr(node, "src", src.to_string());
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        node.borrow_mut().text = Some(label.to_string());
    }

    fn set_page_metadata(&mut self, meta: &runtime_core::PageMetadata) {
        self.metadata = meta.clone();
    }

    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        add_class(node, class);
    }

    fn set_app_background(&mut self, color: &runtime_core::Tokenized<runtime_core::Color>) {
        self.app_bg = Some(color.clone());
    }

    fn install_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        self.tokens = tokens.to_vec();
    }

    fn update_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        for incoming in tokens {
            if let Some(slot) = self.tokens.iter_mut().find(|t| t.name == incoming.name) {
                slot.value = incoming.value.clone();
            } else {
                self.tokens.push(incoming.clone());
            }
        }
    }

    fn finish(&mut self, root: Self::Node) {
        self.root = Some(root);
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
    /// [`runtime_core::set_page_metadata`] (title). `None` otherwise — the
    /// caller supplies the subject when sending.
    pub subject: Option<String>,
}

/// Viewport email templates are rendered at. Email is a narrow, fixed-width
/// medium; responsive author code that reads `viewport_size()` gets the
/// mobile-ish layout, which is the right default for a 600px email column.
const EMAIL_VIEWPORT: runtime_core::ViewportSize = runtime_core::ViewportSize::new(600.0, 800.0);

/// Render an idealyst template to an email. Runs the author closure once on
/// the host (native) target against a fresh [`EmailBackend`] and returns the
/// self-contained HTML document (styles inline, tokens resolved) plus a
/// plaintext alternative.
pub fn render_email<F>(app: F) -> RenderedEmail
where
    F: FnOnce() -> runtime_core::Element,
{
    render_email_with(|_| {}, app)
}

/// Like [`render_email`] but runs `setup` against the backend before the
/// build — the hook to install theme tokens / app background for the render
/// (e.g. `setup(|b| my_theme::install(b))`).
pub fn render_email_with<S, F>(setup: S, app: F) -> RenderedEmail
where
    S: FnOnce(&mut EmailBackend),
    F: FnOnce() -> runtime_core::Element,
{
    scheduler::ensure_installed();
    let backend = Rc::new(RefCell::new(EmailBackend::new()));
    setup(&mut backend.borrow_mut());
    let owner = runtime_core::mount(backend.clone(), move || {
        runtime_core::set_viewport_size(EMAIL_VIEWPORT);
        app()
    });
    // Run any deferred reactive builds now that the mount borrow is released.
    scheduler::drain();
    let rendered = {
        let b = backend.borrow();
        RenderedEmail {
            html: email_document(&b.body_html(), b.metadata.title.as_deref(), b.body_bg()),
            text: b.plain_text(),
            subject: b.metadata.title.clone(),
        }
    };
    drop(owner);
    rendered
}

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
    use runtime_core::accessibility::AccessibilityProps;
    use runtime_core::{render, text, view, Color, Length, StyleRules, TokenEntry, TokenValue, Tokenized};

    /// A `view([text])` mounted through the real walker serializes to nested
    /// `<div><span>` markup — proving headless email rendering of the author
    /// tree end to end.
    #[test]
    fn view_with_text_renders_nested_html() {
        let backend = Rc::new(RefCell::new(EmailBackend::new()));
        let _owner = render(backend.clone(), view(vec![text("hi").into()]).into());
        assert_eq!(backend.borrow().body_html(), "<div><span>hi</span></div>");
    }

    /// Author text is HTML-escaped so template strings can't inject markup.
    #[test]
    fn text_content_is_escaped() {
        let backend = Rc::new(RefCell::new(EmailBackend::new()));
        let _owner = render(backend.clone(), text("a<b>&c").into());
        assert_eq!(backend.borrow().body_html(), "<span>a&lt;b&gt;&amp;c</span>");
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
        use runtime_core::{Breakpoint, StateBits};
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
    /// reactive style re-delivery does — old-core state re-fires, new-core
    /// theme-cohort reapply on an in-build token install) must NOT
    /// duplicate the inline CSS. A genuinely changed style still appends
    /// (later wins via CSS source order).
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
    /// plaintext alternative. The subject comes from page metadata if set.
    #[test]
    fn render_email_wraps_full_document() {
        let out = render_email(|| view(vec![text("Hello world").into()]).into());
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
        use runtime_core::primitives::link::LinkConfig;
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
        let out = render_email_with(
            |b| b.set_app_background(&Tokenized::Literal(Color("#0b1020".into()))),
            || view(vec![text("hi").into()]).into(),
        );
        assert!(
            out.html.contains("background:#0b1020"),
            "body background applied, got: {}",
            out.html
        );
    }
}
