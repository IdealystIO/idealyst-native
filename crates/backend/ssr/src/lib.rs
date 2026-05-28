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
    children: Vec<NodeRef>,
}

impl HtmlNode {
    fn new(tag: &'static str) -> Self {
        Self { tag, text: None, style: None, attrs: Vec::new(), children: Vec::new() }
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

fn serialize(node: &NodeRef, out: &mut String) {
    let n = node.borrow();
    out.push('<');
    out.push_str(n.tag);
    if let Some(style) = &n.style {
        if !style.is_empty() {
            out.push_str(" style=\"");
            escape_attr(style, out);
            out.push('"');
        }
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

    fn create_icon(
        &mut self,
        _data: &runtime_core::primitives::icon::IconData,
        _color: Option<&runtime_core::Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // First paint doesn't need the vector paths drawn; emit a
        // placeholder span so layout reserves the slot. The live bundle
        // renders the real inline <svg> on hydration.
        nref(HtmlNode::new("span"))
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        parent.borrow_mut().children.push(child);
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        node.borrow_mut().text = Some(content.to_string());
    }

    fn clear_children(&mut self, node: &Self::Node) {
        node.borrow_mut().children.clear();
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let body = css::rules_to_css(style);
        node.borrow_mut().style = Some(body);
    }

    fn create_link(
        &mut self,
        config: runtime_core::primitives::link::LinkConfig,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let mut node = HtmlNode::new("a");
        node.attrs.push(("href", config.url.clone()));
        if config.external {
            node.attrs.push(("target", "_blank".to_string()));
            node.attrs.push(("rel", "noopener".to_string()));
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
        nref(HtmlNode::new("div"))
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
        _type_id: std::any::TypeId,
        _type_name: &'static str,
        _payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Third-party External widgets render via their own runtime on
        // the client; emit a host container for the bundle to fill.
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
/// server-rendered first-paint markup followed by a module script that
/// loads the real WebBackend bundle. The bundle boots and replaces
/// `#app`'s contents (see the web backend's `finish`).
///
/// `bundle_module` is the path to the bundle's JS entry (e.g.
/// `/pkg/website.js`); it's developer-provided config, not user input.
pub fn render_document(page: &RenderedPage, bundle_module: &str) -> String {
    let m = &page.metadata;
    let mut doc = String::from("<!DOCTYPE html>\n<html lang=\"en\">\n<head>");
    doc.push_str("<meta charset=\"utf-8\">");
    doc.push_str(
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
    );
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
    doc.push_str("</head>\n<body><div id=\"app\">");
    doc.push_str(&page.html);
    doc.push_str("</div><script type=\"module\">import init from \"");
    doc.push_str(bundle_module);
    doc.push_str("\";init();</script></body>\n</html>");
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

/// Like [`render_path`] but runs `setup` against the backend before the
/// build — the hook where navigator SDKs register their SSR handlers
/// (`stack_navigator::ssr::register(backend)`, etc.) so chrome renders.
pub fn render_path_with<S, F>(path: &str, setup: S, app: F) -> RenderedPage
where
    S: FnOnce(&mut SsrBackend),
    F: FnOnce() -> runtime_core::Element,
{
    scheduler::ensure_installed();
    runtime_core::primitives::navigator::set_initial_path(Some(path.to_string()));
    let backend = Rc::new(RefCell::new(SsrBackend::new()));
    setup(&mut backend.borrow_mut());
    let owner = runtime_core::mount(backend.clone(), app);
    // Clear in case the tree had no navigator to consume it.
    runtime_core::primitives::navigator::set_initial_path(None);
    // Run deferred chrome builds (e.g. a drawer sidebar built via
    // `build_node`) now that the mount borrow is released.
    scheduler::drain();
    let page = {
        let b = backend.borrow();
        RenderedPage { html: b.into_html(), metadata: b.metadata.clone() }
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

    /// `apply_style` runs the resolved `StyleRules` through the shared
    /// `css` crate and emits the result as an inline `style` attribute —
    /// the same declarations the web backend would put in a class body.
    #[test]
    fn apply_style_emits_inline_css() {
        let mut b = SsrBackend::new();
        let v = b.create_view(&AccessibilityProps::default());
        let mut rules = StyleRules::default();
        rules.background = Some(Tokenized::Literal(Color("#ff0000".into())));
        b.apply_style(&v, &Rc::new(rules));
        b.finish(v);
        assert_eq!(b.into_html(), r#"<div style="background: #ff0000"></div>"#);
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
        };
        let doc = render_document(&page, "/pkg/app.js");

        assert!(doc.starts_with("<!DOCTYPE html>"));
        assert!(doc.contains("<title>Home</title>"));
        assert!(doc.contains(r#"<meta property="og:title" content="Home">"#));
        assert!(doc.contains(r#"<meta name="description" content="welcome">"#));
        assert!(doc.contains(r#"<meta property="og:image" content="/og.png">"#));
        assert!(doc.contains(r#"<div id="app"><div>hi</div></div>"#));
        assert!(doc.contains(r#"import init from "/pkg/app.js";init();"#));
    }
}
