//! Email rendering: the `runtime_scene::Host` + capability-trait surface
//! and the one-shot-world render entries (idea-lite core).
//!
//! [`EmailBackend`] implements [`runtime_scene::Host`] plus **all 30**
//! capability traits (`runtime_vocabulary::caps`) — the production shape
//! of the migration (the same choice ssr/web/macOS made). Every method
//! body in this file IS the email mechanism: it was moved here verbatim
//! from the crate's old `impl Backend for EmailBackend` when the
//! 159-method mega-trait was deleted, so the HTML mechanism (node
//! building, deferred token resolution, inline-style serialization) is
//! unchanged. Capabilities email does not implement are simply absent —
//! the caps-trait DEFAULT bodies serve them, and those defaults were
//! audited byte-for-byte against the `Backend` defaults they replace
//! (`docs/runtime-v2-deletion-baseline.md` S2.1; 116 of email's 152
//! caps methods resolve to a default).
//!
//! **30/30 traits implemented, 0 adapted, 0 stubbed.**
//!
//! # One-shot world per render
//!
//! [`render_email`]/[`render_email_with`] mirror
//! `backend_ssr::newcore::render_path`: a **fresh
//! [`World`] per email** — enter, realize through
//! [`runtime_vocabulary::register_builtins`], flush once, serialize
//! (inline styles, tokens baked to literals via
//! `css::rules_to_css_resolved` at serialize time), drop. Any number of
//! emails can render on one thread with fully independent
//! signals/effects/theme state; dropping the `Realized` runs every
//! cleanup and dropping the `World` removes its TLS registry entry —
//! nothing accumulates across renders.
//!
//! # What email deliberately does NOT install (same list as SSR)
//!
//! - **No flush driver / dispatch hook.** An email renders the
//!   committed initial state; there is no event dispatch, so the single
//!   post-realize `world.flush()` is the whole commit story.
//! - **Frames/timers are dropped** by the crate's queue-only scheduler
//!   (`crate::scheduler`): a static email has no animation loop.
//! - **No dispatch-site callback wrapping.** Email swallows every author
//!   callback (there is no interaction in the output), so no capability
//!   method here wraps a callback — exactly like SSR. That is why every
//!   body below could be moved from the old `Backend` impl unchanged.
//!
//! # Anchoring
//!
//! [`Host::supports_splice`] is `false`, so every reactive region nests
//! under a [`Host::create_anchor`] `<div>`. See the method's comment: the
//! value used to be inherited from a `Backend` trait default and is now
//! stated explicitly, pinned by `newcore_host_is_anchored`.
//!
//! # Output parity with the old core
//!
//! The frozen artifacts in `tests/goldens/` are what the OLD core
//! rendered for the corpus (static styled trees, installed tokens,
//! dropped state/breakpoint overlays, links, dyn branches, and the real
//! idea-ui-mail welcome template). `tests/newcore_golden.rs` compares
//! this module's output against them byte-for-byte, with zero
//! normalization.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{realize, Element, Host, Registry};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::StyleRules;
use runtime_vocabulary::caps;
use runtime_world::World;

use crate::{add_class, nref, push_style_dedup, set_attr, HtmlNode, LINK_RESET_STYLE};
use crate::{EmailBackend, NodeRef, RenderedEmail};

// ===========================================================================
// Render entry points
// ===========================================================================

/// Render an idealyst template to an email: the self-contained HTML
/// document (styles inline, tokens resolved to literals) plus a
/// plaintext alternative and the subject from page metadata.
pub fn render_email<F>(build: F) -> RenderedEmail
where
    F: FnOnce() -> Element,
{
    render_email_with(|_| {}, build)
}

/// Like [`render_email`] but runs `setup` against the backend before the
/// build — the hook to install theme tokens / app background for the
/// render (e.g. `setup(|b| caps::StyleOps::install_tokens(b, &theme))`).
/// Token installs may equally happen inside the build via
/// `runtime_vocabulary::theme::install_tokens` — resolution is deferred
/// to serialize time either way, so ordering never matters.
pub fn render_email_with<S, F>(setup: S, build: F) -> RenderedEmail
where
    S: FnOnce(&mut EmailBackend),
    F: FnOnce() -> Element,
{
    // Queue-only scheduler: microtasks queue and drain below;
    // frames/timers drop (module docs).
    crate::scheduler::ensure_installed();
    // Viewport seed for author code that reads `viewport_size()`, seeded
    // OUTSIDE the world on purpose — the vocabulary's per-world viewport
    // ctx reads it at creation during realize.
    runtime_shared::set_viewport_size(crate::EMAIL_VIEWPORT);

    let backend = Rc::new(RefCell::new(EmailBackend::new()));
    setup(&mut backend.borrow_mut());
    let mut registry: Registry<EmailBackend> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let registry = Rc::new(registry);

    // THE one-shot world. `World::new` registers it in the thread's
    // world table; the drop at the end of this function removes it —
    // N emails on one thread never accumulate reactive state.
    let world = World::new();
    let realized = world.enter(|| {
        let element = build();
        realize(&backend, &registry, element)
    });

    // Single-root contract: `finish` roots the HTML serialization.
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_email::newcore::render_email: the template root must contribute exactly \
             one top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    caps::LifecycleOps::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount — the render's one and only
    // flush — then run queued microtasks with no backend borrow held.
    world.flush();
    crate::scheduler::drain();

    let rendered = {
        let b = backend.borrow();
        RenderedEmail {
            html: crate::email_document(&b.body_html(), b.metadata.title.as_deref(), b.body_bg()),
            text: b.plain_text(),
            subject: b.metadata.title.clone(),
        }
    };

    // Teardown order matters: the realized tree unmounts (cleanups fire)
    // BEFORE the world — its slots' owner — dies. Then the world's drop
    // removes it from the thread's registry: no TLS accumulation.
    drop(realized);
    drop(world);
    rendered
}

// ===========================================================================
// Host — the structural seam
// ===========================================================================

impl Host for EmailBackend {
    type Node = NodeRef;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        parent.borrow_mut().children.push(child);
    }

    // `insert_many` is deliberately NOT implemented: `Host`'s default is
    // the same N-x-`insert` loop the old `Backend` default ran, so the
    // emitted node order is unchanged (deletion-baseline S2.2 —
    // "byte-identical on `Host`, safe").

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        let mut p = parent.borrow_mut();
        let index = index.min(p.children.len());
        p.children.insert(index, child);
    }

    /// Explicit port of the old `Backend::remove_child` DEFAULT body (a
    /// no-op). `Host` makes it REQUIRED, so the default that used to
    /// supply this body is gone — the body is reproduced verbatim rather
    /// than inherited (deletion-baseline S2.2). Never called here anyway:
    /// [`supports_splice`](Self::supports_splice) is `false`, so every
    /// reactive region tears down through `clear_children` on its own
    /// anchor.
    fn remove_child(&mut self, _parent: &Self::Node, _child: &Self::Node) {
        // default: no-op
    }

    fn clear_children(&mut self, node: &Self::Node) {
        node.borrow_mut().children.clear();
    }

    fn create_anchor(&mut self) -> Self::Node {
        // A reactive `when`/`switch`/`each` placeholder. `display: contents`
        // keeps it layout-transparent (matching web/SSR) — but email clients
        // vary on `display: contents`, so we instead emit a plain wrapper
        // with no box of its own via zero styling; children flow inside it.
        nref(HtmlNode::new("div"))
    }

    /// Explicit `false` — the port of the old `Backend::supports_child_splice`
    /// DEFAULT this backend relied on. `Host` makes it REQUIRED, so the
    /// value is now stated here instead of inherited
    /// (deletion-baseline S2.2). Email has no hydration consumer; ANCHORED
    /// mode is what every frozen golden in `tests/goldens/` recorded from
    /// the old core, and `newcore_host_is_anchored` pins the value.
    fn supports_splice(&self) -> bool {
        false
    }
}

// ===========================================================================
// App environment + lifecycle
// ===========================================================================

impl caps::AppEnvOps for EmailBackend {
    fn platform(&self) -> runtime_shared::Platform {
        // Email is closest to the web surface (HTML/CSS output); author code
        // branching on `platform()` treats it like the web target.
        runtime_shared::Platform::Web
    }

    fn set_page_metadata(&mut self, meta: &runtime_shared::PageMetadata) {
        self.metadata = meta.clone();
    }

    fn set_app_background(&mut self, color: &runtime_shared::Tokenized<runtime_shared::Color>) {
        self.app_bg = Some(color.clone());
    }
}

impl caps::LifecycleOps for EmailBackend {
    fn finish(&mut self, root: Self::Node) {
        self.root = Some(root);
    }

    /// Keep the `lazy` primitive at its placeholder — a static email can't paint
    /// lazy/GPU content, and there's no client to load the chunk later.
    fn renders_lazy_chunks(&self) -> bool {
        false
    }
}

// ===========================================================================
// View + input + pressable
// ===========================================================================

impl caps::ViewOps for EmailBackend {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        nref(HtmlNode::new("div"))
    }
}

impl caps::InputOps for EmailBackend {}

impl caps::PressableOps for EmailBackend {
    fn create_pressable(
        &mut self,
        _on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // No interaction in email; a pressable is just a container.
        nref(HtmlNode::new("div"))
    }
}

// ===========================================================================
// Text + button
// ===========================================================================

impl caps::TextOps for EmailBackend {
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        let mut node = HtmlNode::new("span");
        node.text = Some(content.to_string());
        nref(node)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        node.borrow_mut().text = Some(content.to_string());
    }
}

impl caps::ButtonOps for EmailBackend {
    fn create_button(
        &mut self,
        label: &str,
        _on_click: &runtime_shared::Action,
        _leading_icon: Option<&runtime_shared::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_shared::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // No JS in email: a "button" is just a styled inline box. Authors who
        // want a clickable CTA wrap a `link` (idea-ui-mail's Button does).
        let mut node = HtmlNode::new("span");
        node.text = Some(label.to_string());
        nref(node)
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        node.borrow_mut().text = Some(label.to_string());
    }
}

// ===========================================================================
// Image + icon + link
// ===========================================================================

impl caps::ImageOps for EmailBackend {
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

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        set_attr(node, "src", src.to_string());
    }
}

impl caps::IconOps for EmailBackend {
    fn create_icon(
        &mut self,
        data: &runtime_shared::primitives::icon::IconData,
        color: Option<&runtime_shared::Color>,
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
            runtime_shared::primitives::icon::FillRule::NonZero => "nonzero",
            runtime_shared::primitives::icon::FillRule::EvenOdd => "evenodd",
        };
        for path_d in data.paths {
            let mut path = HtmlNode::new("path");
            path.attrs.push(("d", (*path_d).to_string()));
            path.attrs.push(("fill-rule", fill_rule.to_string()));
            svg.children.push(nref(path));
        }
        nref(svg)
    }
}

impl caps::LinkOps for EmailBackend {
    fn create_link(
        &mut self,
        config: runtime_shared::primitives::link::LinkConfig,
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
}

// ===========================================================================
// Form widgets
// ===========================================================================

impl caps::TextInputOps for EmailBackend {
    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        _on_blur: Option<runtime_shared::primitives::text_input::BlurHandler>,
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
        _on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
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
}

impl caps::ToggleOps for EmailBackend {
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
}

impl caps::SliderOps for EmailBackend {
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
}

impl caps::ActivityIndicatorOps for EmailBackend {
    fn create_activity_indicator(
        &mut self,
        _size: runtime_shared::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&runtime_shared::Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        nref(HtmlNode::new("div"))
    }
}

// ===========================================================================
// Scroll + safe area + virtualizer
// ===========================================================================

impl caps::ScrollOps for EmailBackend {
    fn create_scroll_view(
        &mut self,
        _horizontal: bool,
        _on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // No scroll in email; a scroll_view is just a container.
        nref(HtmlNode::new("div"))
    }
}

impl caps::SafeAreaOps for EmailBackend {}

// No two-axis grid engine on this backend yet; every `GridOps`
// method defaults, so `virtual_grid` reports itself as an
// unsupported primitive instead of silently rendering nothing.
impl caps::GridOps for EmailBackend {}

impl caps::VirtualizerOps for EmailBackend {
    fn create_virtualizer(
        &mut self,
        _callbacks: runtime_shared::VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        _layout: runtime_shared::VirtualLayout,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Virtualized lists have no meaning in a static email; emit the
        // container only (authors render real rows with `for` for email).
        nref(HtmlNode::new("div"))
    }
}

// ===========================================================================
// Graphics + portal + presence + navigator
// ===========================================================================

impl caps::GraphicsOps for EmailBackend {
    fn create_graphics(
        &mut self,
        _on_ready: runtime_shared::primitives::graphics::OnReady,
        _on_resize: runtime_shared::primitives::graphics::OnResize,
        _on_lost: runtime_shared::primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // GPU canvas can't render into an email; emit an empty box.
        nref(HtmlNode::new("div"))
    }
}

impl caps::PortalOps for EmailBackend {
    fn create_portal(
        &mut self,
        _target: runtime_shared::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        nref(HtmlNode::new("div"))
    }
}

impl caps::PresenceOps for EmailBackend {}

impl caps::NavigatorOps for EmailBackend {}

// ===========================================================================
// External + document
// ===========================================================================

impl caps::ExternalOps for EmailBackend {
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
}

impl caps::DocumentOps for EmailBackend {
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

    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        add_class(node, class);
    }
}

// ===========================================================================
// Style + assets
// ===========================================================================

impl caps::StyleOps for EmailBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        // Store the resolved base style; flattened to inline CSS (tokens baked
        // to literals) at serialize time. NO class, NO head stylesheet.
        push_style_dedup(node, style);
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        _overlays: &[(runtime_shared::StateBits, Rc<StyleRules>)],
    ) {
        push_style_dedup(node, base);
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        _overlays: &[(runtime_shared::StateBits, Rc<StyleRules>)],
        _breakpoint_overlays: &[(runtime_shared::Breakpoint, Rc<StyleRules>)],
        _container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        // Email has no interaction and unreliable `@media`/`@container`
        // support — emit only the resolved base, drop every overlay.
        push_style_dedup(node, base);
    }

    // Email opts into the "native state" model only so the walker hands us the
    // base + overlays in one call (`apply_styled_states`) — we keep the base
    // and DROP the overlays. There is no `:hover`/`:active`/`:focus` in email.
    fn handles_states_natively(&self) -> bool {
        true
    }

    fn install_tokens(&mut self, tokens: &[runtime_shared::TokenEntry]) {
        self.tokens = tokens.to_vec();
    }

    fn update_tokens(&mut self, tokens: &[runtime_shared::TokenEntry]) {
        for incoming in tokens {
            if let Some(slot) = self.tokens.iter_mut().find(|t| t.name == incoming.name) {
                slot.value = incoming.value.clone();
            } else {
                self.tokens.push(incoming.clone());
            }
        }
    }
}

impl caps::AssetOps for EmailBackend {}

// ===========================================================================
// A11y + animation + introspection
// ===========================================================================

impl caps::A11yOps for EmailBackend {}

impl caps::AnimationOps for EmailBackend {}

impl caps::IntrospectionOps for EmailBackend {}

// ===========================================================================
// Batch + wire bindings
// ===========================================================================

impl caps::BatchOps for EmailBackend {}

impl caps::WireBindingOps for EmailBackend {}
