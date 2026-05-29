//! Server-side rendering backend.
//!
//! `SsrBackend` is a headless [`Backend`](runtime_core::Backend)
//! implementation: instead of mutating a live DOM (web) or a native
//! view tree (iOS/Android), it accumulates an in-memory HTML node tree
//! and serializes it to a string. It runs on the host (native) target —
//! the same author tree that renders on every other backend is walked
//! once, synchronously, and emitted as HTML + inline CSS for first
//! paint, crawlers, and link unfurlers.
//!
//! The emitted markup is throwaway: the served page still loads the
//! normal WebBackend wasm bundle, which boots and replaces the DOM.
//! Because styling reuses the same [`css::rules_to_css`] the web
//! backend uses, the first-paint output matches what the live app
//! renders — no flash when the bundle takes over.
//!
//! This is the modeled-on-`MockBackend` minimal core (the 8 required
//! `Backend` methods) plus `img`/`icon` coverage. Broader primitive
//! coverage (inputs, links, navigator-at-path, `<head>` metadata) lands
//! in follow-up slices tracked alongside this work.

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{NavigatorHandler, NavigatorHost, RegisterNavigator};
use runtime_core::{Backend, NavigatorRegistry, StyleRules};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[cfg(feature = "serve")]
mod serve;
#[cfg(feature = "serve")]
pub use serve::{serve, ServeConfig};

/// A stashed navigator handler, keyed by its container node's pointer id.
type NavHandler = Rc<RefCell<Box<dyn NavigatorHandler<SsrBackend>>>>;

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
            children: Vec::new(),
        }
    }
}

/// Wrap a freshly-built node in a shareable handle.
fn nref(n: HtmlNode) -> NodeRef {
    Rc::new(RefCell::new(n))
}

/// Stable map key for a navigator container node (pointer identity; the
/// node stays alive in the tree so the address is stable).
fn nav_key(n: &NodeRef) -> usize {
    Rc::as_ptr(n) as usize
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
    metadata: runtime_core::PageMetadata,
    navigator_handlers: NavigatorRegistry<SsrBackend>,
    /// Keyed by container-node pointer id (see [`nav_key`]).
    nav_handler_instances: HashMap<usize, NavHandler>,
    /// Stylesheets registered via [`Backend::register_raw_css`] (e.g. the
    /// navigator layout sheet). Deduped, emitted in the document `<head>`.
    raw_css: Vec<String>,
    /// The active theme's tokens, captured from
    /// [`Backend::install_tokens`]/[`Backend::update_tokens`]. Emitted as
    /// a `:root { … }` block so the SSR first paint resolves
    /// `var(--token, fallback)` to the real theme value (matching the
    /// live web build, which installs the same variables at runtime).
    tokens: Vec<runtime_core::TokenEntry>,
    /// Served URL per registered asset id (fonts/images), from
    /// [`Backend::register_asset`]. Fonts feed the `@font-face` rules
    /// below; image URLs are read at `create_image` time.
    asset_urls: HashMap<runtime_core::assets::AssetId, String>,
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
    /// Third-party `Element::External` handlers (e.g. `idea_codeblock`),
    /// so externals SERVER-RENDER their real DOM (a code block's
    /// `<pre>`+spans) instead of an empty host — matching web so
    /// hydration adopts them.
    external_handlers: runtime_core::ExternalRegistry<SsrBackend>,
}

impl SsrBackend {
    pub fn new() -> Self {
        Self::default()
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
    /// 4. registered stylesheets (navigator layout, etc.);
    /// 5. the content-keyed per-node style classes (`apply_style`);
    /// 6. responsive `@media (min-width: …)` breakpoint overlays — LAST,
    ///    so a matching media query overrides the base class rule above it.
    pub fn head_css(&self) -> String {
        let mut out = css::base_reset_css();
        out.push_str(&self.font_faces.concat());
        out.push_str(&css::tokens_to_root_css(&self.tokens));
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

impl RegisterNavigator for SsrBackend {
    fn register_navigator<P, F>(&mut self, factory: F)
    where
        P: 'static,
        F: Fn() -> Box<dyn NavigatorHandler<SsrBackend>> + 'static,
    {
        self.navigator_handlers.register::<P, _>(factory);
    }
}

impl runtime_core::RegisterExternal for SsrBackend {
    fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&Rc<T>, &mut SsrBackend) -> Self::Node + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
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
        if !runtime_core::scheduling::is_scheduler_installed() {
            runtime_core::scheduling::install_scheduler(Box::new(SsrScheduler));
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

impl Backend for SsrBackend {
    type Node = NodeRef;

    fn platform(&self) -> runtime_core::Platform {
        runtime_core::Platform::Web
    }

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        nref(HtmlNode::new("div"))
    }

    fn create_element(&mut self, tag: &str) -> Self::Node {
        // `HtmlNode.tag` is `&'static str`; intern the structural tags an
        // External handler might emit to a static (no allocation/leak).
        // Unknown tags fall back to `div`.
        let tag: &'static str = match tag {
            "pre" => "pre",
            "code" => "code",
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
        let mut node = HtmlNode::new("button");
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
        nref(node)
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        // `display: contents` (matching web) keeps the `when`/`switch`/
        // `each` placeholder layout-transparent: the branch's children
        // inherit the surrounding flex/sizing context and a
        // `position: sticky` child gets the real parent as its containing
        // block (without this, the opaque anchor is a short containing
        // block and sticky stops sticking — e.g. the docs "On this page"
        // rail).
        let mut node = HtmlNode::new("div");
        node.style = Some(css::REACTIVE_ANCHOR_STYLE.to_string());
        nref(node)
    }

    fn create_pressable(
        &mut self,
        _on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // A bare clickable `<div>`, matching the web pressable: a hand
        // cursor + button a11y. The click handler is the live bundle's
        // job on hydration; the static first paint just needs to look
        // and read like a button.
        let mut node = HtmlNode::new("div");
        node.style = Some(css::PRESSABLE_CURSOR_STYLE.to_string());
        node.attrs.push(("role", "button".to_string()));
        node.attrs.push(("tabindex", "0".to_string()));
        nref(node)
    }

    fn create_icon(
        &mut self,
        _data: &runtime_core::primitives::icon::IconData,
        _color: Option<&runtime_core::Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // First paint doesn't need the vector paths drawn; emit a
        // placeholder span so layout reserves the slot. The live bundle
        // renders the real inline <svg> on hydration. Same inline default
        // as the web icon so it sits inline with surrounding text.
        let mut node = HtmlNode::new("span");
        node.style = Some(css::ICON_INLINE_STYLE.to_string());
        nref(node)
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
        // Match the web backend's structure: each resolved style becomes a
        // content-keyed class (`ui-<hash>`) plus one shared rule in the
        // document stylesheet — NOT an inline `style="…"`. Same
        // `hash_class_name` + `rules_to_css` as web, so a given style gets
        // the same class name and declarations on both. Dedupe by class so
        // N nodes sharing a style emit one rule (as web's `pregen` does).
        let class = css::hash_class_name(&style.content_key());
        if !self.style_rules.contains_key(&class) {
            self.style_rules.insert(class.clone(), css::rules_to_css(style));
        }
        add_class(node, &class);
    }

    // SSR opts into the web's declarative state model: interaction-state
    // overlays (`state hovered`, etc.) become CSS pseudo-class rules, so
    // hover/press/focus styling works on the static first paint with no
    // JS — same as the live web build (which the bundle takes over on
    // hydration). The event-driven `attach_states` path needs a runtime.
    fn handles_states_natively(&self) -> bool {
        true
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(runtime_core::StateBits, Rc<StyleRules>)],
    ) {
        // States-only entry; delegate to the superset with no breakpoint
        // overlays so the combined-key + emission logic lives in one place.
        self.apply_styled_variants(node, base, overlays, &[]);
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(runtime_core::StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(runtime_core::Breakpoint, Rc<StyleRules>)],
    ) {
        // Key the class by base + every state overlay + every breakpoint
        // overlay, so a base shared with different hover/responsive styling
        // still gets distinct classes (mirrors the web backend's combined-key
        // scheme, including the `@bp_*` tags).
        let mut combined = base.content_key();
        for (state, overlay) in overlays {
            combined.push('|');
            combined.push_str(&state.0.to_string());
            combined.push(':');
            combined.push_str(&overlay.content_key());
        }
        for (bp, overlay) in breakpoint_overlays {
            combined.push(';');
            combined.push('@');
            combined.push_str(bp.axis_name().unwrap_or("__bp_xs"));
            combined.push(':');
            combined.push_str(&overlay.content_key());
        }
        let class = css::hash_class_name(&combined);
        self.style_rules
            .entry(class.clone())
            .or_insert_with(|| css::rules_to_css(base));
        for (state, overlay) in overlays {
            if let Some(pseudo) = css::state_pseudo(*state) {
                // Key carries the pseudo so head_css emits
                // `.ui-<hash>:hover{ … }` (the node still wears `ui-<hash>`).
                self.style_rules
                    .entry(format!("{class}{pseudo}"))
                    .or_insert_with(|| css::rules_to_css(overlay));
            }
        }
        // Breakpoint overlays → `@media (min-width: …) { .ui-<hash> { … } }`.
        // Keyed by `{class}@{rank}` so `head_css`'s BTreeMap iteration emits
        // them ascending by rank (mobile-first cascade). `None` only for Xs,
        // which the walker never sends as an overlay.
        for (bp, overlay) in breakpoint_overlays {
            let body = css::rules_to_css(overlay);
            if let Some(rule) = css::breakpoint_media_rule(&class, *bp, &body) {
                self.media_rules
                    .entry(format!("{class}@{}", bp.rank()))
                    .or_insert(rule);
            }
        }
        add_class(node, &class);
    }

    fn create_link(
        &mut self,
        config: runtime_core::primitives::link::LinkConfig,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("a");
        // Same de-defaulting reset as the web link primitive (strip the
        // browser's blue/underlined anchor styling).
        node.style = Some(css::LINK_RESET_STYLE.to_string());
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
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("input");
        node.attrs.push(("value", initial_value.to_string()));
        if let Some(p) = placeholder {
            node.attrs.push(("placeholder", p.to_string()));
        }
        nref(node)
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        set_attr(node,"value", value.to_string());
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("textarea");
        node.text = Some(initial_value.to_string());
        if let Some(p) = placeholder {
            node.attrs.push(("placeholder", p.to_string()));
        }
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
        let mut node = HtmlNode::new("input");
        node.attrs.push(("type", "checkbox".to_string()));
        if initial_value {
            node.attrs.push(("checked", String::new()));
        }
        nref(node)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        if value {
            set_attr(node,"checked", String::new());
        } else {
            remove_attr(node, "checked");
        }
    }

    fn create_scroll_view(
        &mut self,
        _horizontal: bool,
        _on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("div");
        node.scroll = true;
        nref(node)
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        _on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("input");
        node.attrs.push(("type", "range".to_string()));
        node.attrs.push(("min", min.to_string()));
        node.attrs.push(("max", max.to_string()));
        if let Some(s) = step {
            node.attrs.push(("step", s.to_string()));
        }
        node.attrs.push(("value", initial_value.to_string()));
        nref(node)
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        set_attr(node,"value", value.to_string());
    }

    fn create_activity_indicator(
        &mut self,
        _size: runtime_core::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&runtime_core::Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Spinner animation is the live bundle's job; reserve a slot.
        nref(HtmlNode::new("div"))
    }

    fn create_virtualizer(
        &mut self,
        _callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        _horizontal: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // First paint emits the scroll container only; the live bundle
        // mounts visible rows on boot. (Row pre-rendering for SEO of
        // virtualized content is a later enhancement.)
        nref(HtmlNode::new("div"))
    }

    fn create_graphics(
        &mut self,
        _on_ready: runtime_core::primitives::graphics::OnReady,
        _on_resize: runtime_core::primitives::graphics::OnResize,
        _on_lost: runtime_core::primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        nref(HtmlNode::new("canvas"))
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        _type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Server-render the external via its registered handler (e.g.
        // `idea_codeblock` → a real `<pre>` + spans), so SSR output
        // matches the web build and hydration adopts it. Falls back to an
        // empty host `<div>` only when no handler is registered (the
        // client bundle then fills it).
        if let Some(handler) = self.external_handlers.get(type_id) {
            handler(payload, self)
        } else {
            nref(HtmlNode::new("div"))
        }
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
        set_attr(node,"src", src.to_string());
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        node.borrow_mut().text = Some(label.to_string());
    }

    fn create_navigator(
        &mut self,
        type_id: std::any::TypeId,
        _type_name: &'static str,
        presentation: Rc<dyn std::any::Any>,
        host: NavigatorHost<Self::Node>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch to a registered SSR handler (which builds the kind's
        // chrome as primitives). With no handler registered, fall back to
        // a bare container; the walker still mounts the path-matched
        // screen into it via `navigator_attach_initial`.
        if let Some(factory) = self.navigator_handlers.get(type_id) {
            let mut handler = factory();
            let node = handler.init(self, host, presentation);
            self.nav_handler_instances
                .insert(nav_key(&node), Rc::new(RefCell::new(handler)));
            node
        } else {
            nref(HtmlNode::new("div"))
        }
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        if let Some(handler) = self.nav_handler_instances.get(&nav_key(navigator)).cloned() {
            handler.borrow_mut().attach_initial(self, screen, scope_id, options);
        } else {
            navigator.borrow_mut().children.push(screen);
        }
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        if let Some(handler) = self.nav_handler_instances.remove(&nav_key(node)) {
            handler.borrow_mut().release(self);
        }
    }

    fn set_page_metadata(&mut self, meta: &runtime_core::PageMetadata) {
        self.metadata = meta.clone();
    }

    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        add_class(node, class);
    }

    fn register_raw_css(&mut self, css: &str) {
        // Dedupe: navigator chrome registers the same layout sheet on
        // every navigator instance.
        if !self.raw_css.iter().any(|c| c == css) {
            self.raw_css.push(css.to_string());
        }
    }

    fn register_asset(
        &mut self,
        id: runtime_core::assets::AssetId,
        kind: runtime_core::assets::AssetTag,
        source: &runtime_core::assets::AssetSource,
    ) {
        if self.asset_urls.contains_key(&id) {
            return;
        }
        // `Embedded` sources have no served URL on a headless server
        // (they'd need a runtime blob, which is web-only) — skip them.
        if let Some(url) = css::asset_url(kind, source) {
            self.asset_urls.insert(id, url);
        }
    }

    fn register_typeface(
        &mut self,
        _id: runtime_core::assets::TypefaceId,
        family_name: &str,
        faces: &[runtime_core::assets::TypefaceFace],
        _fallback: runtime_core::assets::SystemFallback,
    ) {
        for face in faces {
            if let Some(url) = self.asset_urls.get(&face.asset) {
                let rule = css::font_face_css(family_name, face, url);
                if !self.font_faces.contains(&rule) {
                    self.font_faces.push(rule);
                }
            }
        }
    }

    fn install_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        self.tokens = tokens.to_vec();
    }

    fn update_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        // Merge: `update_tokens` may carry only the changed tokens.
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

/// A rendered page: the body HTML (styles inline) plus the `<head>`
/// metadata an author screen declared via
/// [`runtime_core::set_page_metadata`]. Slice that wires the page shell
/// turns `metadata` into `<title>` / `<meta>` / Open Graph tags.
pub struct RenderedPage {
    pub html: String,
    pub metadata: runtime_core::PageMetadata,
    /// Stylesheets the render registered via
    /// [`Backend::register_raw_css`] (navigator layout sheet, etc.).
    /// [`render_document`] emits these in `<head>` so the server's first
    /// paint matches the live web layout.
    pub head_css: String,
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
pub fn render_document(page: &RenderedPage, bundle_module: Option<&str>) -> String {
    let m = &page.metadata;
    let mut doc = String::from("<!DOCTYPE html>\n<html lang=\"en\">\n<head>");
    doc.push_str("<meta charset=\"utf-8\">");
    doc.push_str(
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
    );
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

/// Render an app headlessly at a given URL path. Seeds the navigator's
/// initial-path override so the root navigator opens to the screen
/// matching `path`, walks the tree once against a fresh `SsrBackend`,
/// and returns the body HTML (styles inline) plus the screen's `<head>`
/// metadata.
///
/// `app` is the same entry closure the web bundle mounts — SSR runs it on
/// the host (native) target. The returned markup is throwaway first-paint
/// HTML; the served page still loads the real WebBackend bundle, which
/// boots and replaces the DOM.
pub fn render_path<F>(path: &str, app: F) -> RenderedPage
where
    F: FnOnce() -> runtime_core::Element,
{
    render_path_with(path, |_| {}, app)
}

/// Default viewport SSR assumes (desktop-ish). Responsive author code
/// reads `viewport_size()`; a server has no real window, so we pick a
/// sensible wide default for first paint.
const SSR_VIEWPORT: runtime_core::ViewportSize = runtime_core::ViewportSize::new(1280.0, 800.0);

/// Like [`render_path`] but runs `setup` against the backend before the
/// build — the hook where navigator SDKs register their chrome handlers
/// (`stack_navigator::chrome::register(backend)`, etc.) so chrome renders.
pub fn render_path_with<S, F>(path: &str, setup: S, app: F) -> RenderedPage
where
    S: FnOnce(&mut SsrBackend),
    F: FnOnce() -> runtime_core::Element,
{
    scheduler::ensure_installed();
    runtime_core::primitives::navigator::set_initial_path(Some(path.to_string()));
    let backend = Rc::new(RefCell::new(SsrBackend::new()));
    setup(&mut backend.borrow_mut());
    let owner = runtime_core::mount(backend.clone(), move || {
        // Seed the viewport FIRST, inside the root scope, so the
        // framework's lazy `viewport_size` signal (and any responsive
        // author code reading it) is created in this stable scope —
        // not a transient deferred-build scope, where the cached signal
        // id would dangle and later type-mismatch on recycle.
        runtime_core::set_viewport_size(SSR_VIEWPORT);
        app()
    });
    // Clear in case the tree had no navigator to consume it.
    runtime_core::primitives::navigator::set_initial_path(None);
    // Run deferred chrome builds (e.g. a drawer sidebar built via
    // `build_node`) now that the mount borrow is released.
    scheduler::drain();
    let page = {
        let b = backend.borrow();
        RenderedPage {
            html: b.into_html(),
            metadata: b.metadata.clone(),
            head_css: b.head_css(),
        }
    };
    drop(owner);
    page
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::accessibility::AccessibilityProps;
    use runtime_core::{render, text, view, Color, Tokenized};
    use std::cell::RefCell;

    /// A `view([text])` mounted through the real walker serializes to
    /// nested `<div><span>` markup — proving headless server execution
    /// of the author tree produces HTML end to end.
    #[test]
    fn view_with_text_renders_nested_html() {
        let backend = Rc::new(RefCell::new(SsrBackend::new()));
        let _owner = render(backend.clone(), view(vec![text("hi").into()]).into());
        assert_eq!(backend.borrow().into_html(), "<div><span>hi</span></div>");
    }

    /// Text content is HTML-escaped so author strings can't inject
    /// markup into the server-rendered page.
    #[test]
    fn text_content_is_escaped() {
        let backend = Rc::new(RefCell::new(SsrBackend::new()));
        let _owner = render(backend.clone(), text("a<b>&c").into());
        assert_eq!(backend.borrow().into_html(), "<span>a&lt;b&gt;&amp;c</span>");
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

    /// `apply_styled_states` emits the base rule plus a `:hover` pseudo
    /// rule, so hover styling works on the static first paint with no JS.
    #[test]
    fn apply_styled_states_emits_hover_rule() {
        use runtime_core::StateBits;
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

    /// `apply_styled_variants` emits the base rule plus an
    /// `@media (min-width: …)` rule per breakpoint overlay — so the SSR
    /// first paint already respects size boundaries (the whole point:
    /// a mobile request gets the mobile layout in static HTML). This is
    /// the SSR fix for the "sidebar pinned on initial mobile render" bug.
    #[test]
    fn apply_styled_variants_emits_breakpoint_media_rule() {
        use runtime_core::{Breakpoint, Length};
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

    /// `create_pressable` is a clickable `<div>` with a hand cursor +
    /// button a11y, matching the web pressable.
    #[test]
    fn create_pressable_has_cursor_and_role() {
        let mut b = SsrBackend::new();
        let node = b.create_pressable(Rc::new(|| {}), &AccessibilityProps::default());
        let html = { let mut s = String::new(); serialize(&node, &mut s); s };
        assert!(html.contains("cursor: pointer"), "hand cursor, got: {html}");
        assert!(html.contains(r#"role="button""#), "button role, got: {html}");
        assert!(html.contains(r#"tabindex="0""#), "focusable, got: {html}");
    }

    /// A reactive `when`/`switch`/`each` anchor is `display: contents`
    /// (layout-transparent), matching web — so a `position: sticky` child
    /// keeps the real parent as its containing block and keeps sticking.
    #[test]
    fn reactive_anchor_is_display_contents() {
        let mut b = SsrBackend::new();
        let node = b.create_reactive_anchor();
        let html = { let mut s = String::new(); serialize(&node, &mut s); s };
        assert_eq!(html, r#"<div style="display: contents"></div>"#);
    }

    /// `create_link` resets the browser's anchor defaults (so links don't
    /// render blue/underlined) — same inline reset the web link uses.
    #[test]
    fn create_link_applies_anchor_reset() {
        use runtime_core::primitives::link::LinkConfig;
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
            metadata: runtime_core::PageMetadata {
                title: Some("Home".into()),
                description: Some("welcome".into()),
                og_image: Some("/og.png".into()),
                ..Default::default()
            },
            head_css: ".x{color:red}".into(),
        };
        let doc = render_document(&page, Some("/pkg/app.js"));

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
        let doc = render_document(&page, None);
        assert!(doc.contains(r#"<div id="app" data-ssr-viewport="1280x800"><div>hi</div></div>"#));
        assert!(!doc.contains("<script"), "no bundle => no script, got: {doc}");
    }

    /// Theme tokens delivered via `install_tokens` are emitted as a
    /// `:root { … }` block in `head_css`, so the SSR first paint resolves
    /// `var(--token, fallback)` to the real theme value (matching the
    /// live web build). `update_tokens` merges (changed tokens only).
    #[test]
    fn install_tokens_emits_root_variables() {
        use runtime_core::{Length, TokenEntry, TokenValue};
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

    /// A registered typeface emits `@font-face` rules in `head_css`,
    /// linking the served font URL (matching the live web build) — so the
    /// SSR first paint uses the real font, not a fallback. `register_asset`
    /// must run first to supply the per-face served URL.
    #[test]
    fn register_typeface_emits_font_face_rules() {
        use runtime_core::assets::{AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId};
        use runtime_core::{FontStyle, FontWeight};
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
