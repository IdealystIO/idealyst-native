//! New-core adoption for the email backend (idea-lite migration:
//! one-shot worlds, the SSR shape applied to "SSG for emails").
//!
//! Implements [`runtime_scene::Host`] plus **all 30** capability traits
//! (`runtime_vocabulary::caps`) directly on [`EmailBackend`] — the
//! production shape of the migration (the same choice ssr/web/macOS
//! made): no `LegacyBridge` wrapper in the render path. Every trait
//! method delegates via UFCS (`<EmailBackend as Backend>::method(self,
//! …)`) to the existing `Backend` impl, so the HTML mechanism code
//! (node building, deferred token resolution, inline-style
//! serialization) is REUSED verbatim. Where a `Backend` method is not
//! overridden by `EmailBackend` (handles, batching, wire recording,
//! introspection), the UFCS call resolves to the same trait-default the
//! old walker hits — behavior identical by construction.
//!
//! **30/30 direct, 0 adapted, 0 stubbed.**
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
//!   (`crate::scheduler`, shared with the old render path): a static
//!   email has no animation loop.
//! - **No dispatch-site callback wrapping.** Email swallows every
//!   author callback at the `Backend` layer already (no interaction in
//!   the output), so the caps impls delegate raw — exactly like SSR.
//!
//! # Anchoring
//!
//! [`Host::supports_splice`] delegates to
//! `Backend::supports_child_splice` (the shared `false` default), so
//! every reactive region nests under the same `create_reactive_anchor`
//! `<div>` the old walker emits — anchor placement stays in lockstep
//! with the old core by construction. (Unlike SSR there is no hydration
//! consumer pinning anchors; parity with the old output is the only
//! contract, and delegation preserves it under any future default.)
//!
//! # Output parity with the old-core render
//!
//! The same logical template rendered through [`crate::render_email`]
//! (old core) and [`render_email`] (this module) emits
//! **byte-identical** `html`, `text`, and `subject` — pinned by
//! `tests/newcore_golden.rs` across static styled trees, installed
//! tokens, dropped state/breakpoint overlays, links, dyn branches, and
//! the idea-ui-mail welcome template (old side: the real components;
//! new side: the same template authored against the vocabulary builders
//! — `ui!`'s lowering is a build-graph-wide switch, so one test binary
//! cannot compile the same component body for both cores).

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::accessibility::{AccessibilityProps, AccessibilityTree, LiveRegionPriority, Role};
use runtime_core::animation::AnimProp;
use runtime_core::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
};
use runtime_core::breakpoint::Breakpoint;
use runtime_core::introspect::NativeNode;
use runtime_core::primitives;
use runtime_core::primitives::portal::ViewportRect;
use runtime_core::styled_text::TextRun;
use runtime_core::{
    Action, Backend, BackendBatch, Color, ColorScheme, Easing, FileDropHandler, FontFamily,
    HoverHandler, ImageErrorHandler, ImageLoadHandler, PageMetadata, Platform, SafeAreaSides,
    Screenshot, StateBits, StyleApplication, StyleRules, TokenEntry, Tokenized, TouchHandler,
    TouchId, VirtualizerCallbacks, WheelHandler,
};
use runtime_scene::{realize, Element, Host, Registry};
use runtime_vocabulary::caps;
use runtime_world::World;

use crate::{EmailBackend, RenderedEmail};

// ===========================================================================
// Render entry points
// ===========================================================================

/// Render a new-core idealyst template to an email — the new-core
/// mirror of [`crate::render_email`] (same shape, same
/// [`RenderedEmail`] output: full inline-styled document + plaintext
/// alternative + subject from page-metadata title).
pub fn render_email<F>(build: F) -> RenderedEmail
where
    F: FnOnce() -> Element,
{
    render_email_with(|_| {}, build)
}

/// Like [`render_email`] but runs `setup` against the backend before
/// the build — the same hook the old-core [`crate::render_email_with`]
/// exposes to install theme tokens / app background for the render
/// (e.g. `setup(|b| b.install_tokens(&theme))`). Token installs may
/// equally happen inside the build via
/// `runtime_vocabulary::theme::install_tokens` — resolution is deferred
/// to serialize time either way, so ordering never matters.
pub fn render_email_with<S, F>(setup: S, build: F) -> RenderedEmail
where
    S: FnOnce(&mut EmailBackend),
    F: FnOnce() -> Element,
{
    // Queue-only scheduler (shared with the old render path): microtasks
    // queue and drain below; frames/timers drop (module docs).
    crate::scheduler::ensure_installed();
    // Same viewport seed as the old render (old-core thread-level state,
    // seeded OUTSIDE the world on purpose — the vocabulary's per-world
    // viewport ctx reads it at creation during realize).
    runtime_core::set_viewport_size(crate::EMAIL_VIEWPORT);

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

    // Single-root contract, matching the old-core mount: `finish` roots
    // the HTML serialization.
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_email::newcore::render_email: the template root must contribute exactly \
             one top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    Backend::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount — the render's one and only
    // flush — then run queued microtasks with no backend borrow held
    // (parity with the old render path's post-mount drain).
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
    type Node = <EmailBackend as Backend>::Node;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        Backend::insert(self, parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        Backend::insert_many(self, parent, children)
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        Backend::insert_at(self, parent, child, index)
    }

    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        Backend::remove_child(self, parent, child)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        Backend::clear_children(self, node)
    }

    fn create_anchor(&mut self) -> Self::Node {
        Backend::create_reactive_anchor(self)
    }

    /// Delegated (currently the shared `false` default), NOT hard-coded:
    /// email has no hydration consumer — old-output parity is the only
    /// anchor contract, and delegation keeps both cores' anchor
    /// placement in lockstep by construction (module docs, "Anchoring").
    fn supports_splice(&self) -> bool {
        Backend::supports_child_splice(self)
    }
}

// ===========================================================================
// App environment + lifecycle
// ===========================================================================

impl caps::AppEnvOps for EmailBackend {
    fn color_scheme(&self) -> ColorScheme {
        Backend::color_scheme(self)
    }

    fn platform(&self) -> Platform {
        Backend::platform(self)
    }

    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        Backend::url_opener(self)
    }

    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
        Backend::fullscreen_setter(self)
    }

    fn set_page_metadata(&mut self, meta: &PageMetadata) {
        Backend::set_page_metadata(self, meta)
    }

    fn set_app_background(&mut self, color: &Tokenized<Color>) {
        Backend::set_app_background(self, color)
    }

    fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>) {
        Backend::set_scrollbar_theme(self, thumb, track)
    }

    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
        Backend::set_app_key_handler(self, handler)
    }
}

impl caps::LifecycleOps for EmailBackend {
    fn finish(&mut self, root: Self::Node) {
        Backend::finish(self, root)
    }

    fn run_layout(&mut self) {
        Backend::run_layout(self)
    }

    fn schedule_layout_pass() {
        <EmailBackend as Backend>::schedule_layout_pass()
    }

    fn is_hydrating(&self) -> bool {
        Backend::is_hydrating(self)
    }

    fn renders_lazy_chunks(&self) -> bool {
        Backend::renders_lazy_chunks(self)
    }
}

// ===========================================================================
// View + input + pressable
// ===========================================================================

impl caps::ViewOps for EmailBackend {
    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_view(self, a11y)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        Backend::make_view_handle(self, node)
    }
}

impl caps::InputOps for EmailBackend {
    fn install_touch_handler(&mut self, node: &Self::Node, handler: TouchHandler) {
        Backend::install_touch_handler(self, node, handler)
    }

    fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId) {
        Backend::claim_touch(self, node, touch_id)
    }

    fn install_wheel_handler(&mut self, node: &Self::Node, handler: WheelHandler) {
        Backend::install_wheel_handler(self, node, handler)
    }

    fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
        Backend::install_hover_handler(self, node, handler)
    }

    fn mark_preserves_focus(&mut self, node: &Self::Node) {
        Backend::mark_preserves_focus(self, node)
    }

    fn install_file_drop_handler(&mut self, node: &Self::Node, handler: FileDropHandler) {
        Backend::install_file_drop_handler(self, node, handler)
    }
}

impl caps::PressableOps for EmailBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_pressable(self, on_click, a11y)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_core::PressableHandle {
        Backend::make_pressable_handle(self, node)
    }
}

// ===========================================================================
// Text + button
// ===========================================================================

impl caps::TextOps for EmailBackend {
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_text(self, content, a11y)
    }

    fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_styled_text(self, runs, a11y)
    }

    fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
        Backend::update_styled_text(self, node, runs)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        Backend::update_text(self, node, content)
    }

    fn create_text_with_id(
        &mut self,
        content: &str,
        a11y: &AccessibilityProps,
    ) -> Option<(Self::Node, u32)> {
        Backend::create_text_with_id(self, content, a11y)
    }

    fn update_text_by_id(&mut self, id: u32, content: String) {
        Backend::update_text_by_id(self, id, content)
    }

    fn release_text_id(&mut self, id: u32) {
        Backend::release_text_id(self, id)
    }

    fn supports_js_text_bindings(&self) -> bool {
        Backend::supports_js_text_bindings(self)
    }

    fn register_reactive_text_binding(
        &mut self,
        text_id: u32,
        signal_ids: &[u64],
        template_parts: &[&str],
        initial_values: &[&str],
        stringifiers: &[Rc<dyn Fn() -> String>],
    ) {
        Backend::register_reactive_text_binding(
            self,
            text_id,
            signal_ids,
            template_parts,
            initial_values,
            stringifiers,
        )
    }

    fn release_reactive_text_binding(&mut self, text_id: u32) {
        Backend::release_reactive_text_binding(self, text_id)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        Backend::make_text_handle(self, node)
    }
}

impl caps::ButtonOps for EmailBackend {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_button(self, label, on_click, leading_icon, trailing_icon, a11y)
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        Backend::update_button_label(self, node, label)
    }

    fn make_button_handle(&self, node: &Self::Node) -> runtime_core::ButtonHandle {
        Backend::make_button_handle(self, node)
    }
}

// ===========================================================================
// Image + icon + link
// ===========================================================================

impl caps::ImageOps for EmailBackend {
    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_image(self, src, alt, a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        Backend::update_image_src(self, node, src)
    }

    fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
        Backend::update_image_alt(self, node, alt)
    }

    fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
        Backend::install_image_load_handler(self, node, handler)
    }

    fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
        Backend::install_image_error_handler(self, node, handler)
    }

    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
        Backend::make_image_handle(self, node)
    }
}

impl caps::IconOps for EmailBackend {
    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_icon(self, data, color, a11y)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        Backend::update_icon_color(self, node, color)
    }

    fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData) {
        Backend::update_icon_data(self, node, data)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        Backend::update_icon_stroke(self, node, progress)
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        Backend::animate_icon_stroke(self, node, from, to, duration_ms, easing, infinite, autoreverses)
    }

    fn make_icon_handle(&self, node: &Self::Node) -> primitives::icon::IconHandle {
        Backend::make_icon_handle(self, node)
    }
}

impl caps::LinkOps for EmailBackend {
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_link(self, config, a11y)
    }

    fn update_link_url(&mut self, node: &Self::Node, url: &str) {
        Backend::update_link_url(self, node, url)
    }

    fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
        Backend::make_link_handle(self, node)
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
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        secure: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_text_input(
            self,
            initial_value,
            placeholder,
            on_change,
            on_key_down,
            on_blur,
            secure,
            a11y,
        )
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        Backend::update_text_input_value(self, node, value)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        Backend::update_text_input_secure(self, node, secure)
    }

    fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {
        Backend::set_text_input_focus_handler(self, node, handler)
    }

    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
        Backend::update_text_input_placeholder(self, node, placeholder)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_text_area(
            self,
            initial_value,
            placeholder,
            wrap,
            min_rows,
            max_rows,
            on_change,
            on_key_down,
            a11y,
        )
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        Backend::update_text_area_value(self, node, value)
    }

    fn make_text_input_handle(&self, node: &Self::Node) -> primitives::text_input::TextInputHandle {
        Backend::make_text_input_handle(self, node)
    }

    fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
        Backend::make_text_area_handle(self, node)
    }
}

impl caps::ToggleOps for EmailBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_toggle(self, initial_value, on_change, a11y)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        Backend::update_toggle_value(self, node, value)
    }

    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
        Backend::make_toggle_handle(self, node)
    }
}

impl caps::SliderOps for EmailBackend {
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_slider(self, initial_value, min, max, step, on_change, a11y)
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        Backend::update_slider_value(self, node, value)
    }

    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
        Backend::make_slider_handle(self, node)
    }
}

impl caps::ActivityIndicatorOps for EmailBackend {
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_activity_indicator(self, size, color, a11y)
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &Self::Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        Backend::update_activity_indicator_size(self, node, size)
    }

    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle {
        Backend::make_activity_indicator_handle(self, node)
    }
}

// ===========================================================================
// Scroll + safe area + virtualizer
// ===========================================================================

impl caps::ScrollOps for EmailBackend {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_scroll_view(self, horizontal, on_scroll, a11y)
    }

    fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
        Backend::node_scroll(self, node)
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
        Backend::set_node_scroll(self, node, x, y)
    }

    fn make_scroll_view_handle(&self, node: &Self::Node) -> primitives::scroll_view::ScrollViewHandle {
        Backend::make_scroll_view_handle(self, node)
    }
}

impl caps::SafeAreaOps for EmailBackend {
    fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        Backend::apply_safe_area_padding(self, node, sides)
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        Backend::apply_scroll_view_safe_area_inset(self, node, sides)
    }
}

impl caps::VirtualizerOps for EmailBackend {
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_virtualizer(self, callbacks, overscan, layout, a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        Backend::virtualizer_data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        Backend::release_virtualizer(self, node)
    }

    fn make_virtualizer_handle(&self, node: &Self::Node) -> primitives::virtualizer::VirtualizerHandle {
        Backend::make_virtualizer_handle(self, node)
    }
}

// ===========================================================================
// Graphics + portal + presence + navigator
// ===========================================================================

impl caps::GraphicsOps for EmailBackend {
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_graphics(self, on_ready, on_resize, on_lost, a11y)
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        Backend::release_graphics(self, node)
    }

    fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle {
        Backend::make_graphics_handle(self, node)
    }
}

impl caps::PortalOps for EmailBackend {
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_portal(self, target, on_dismiss, trap_focus, a11y)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        Backend::release_portal(self, node)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        Backend::set_portal_hidden(self, node, hidden)
    }

    fn make_portal_handle(&self, node: &Self::Node) -> primitives::portal::PortalHandle {
        Backend::make_portal_handle(self, node)
    }
}

impl caps::PresenceOps for EmailBackend {
    fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        Backend::create_presence_placeholder(self, a11y)
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        Backend::apply_presence(self, node, state, transition)
    }

    fn make_presence_handle(&self, node: &Self::Node) -> primitives::presence::PresenceHandle {
        Backend::make_presence_handle(self, node)
    }
}

impl caps::NavigatorOps for EmailBackend {
    fn create_navigator(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        presentation: Rc<dyn Any>,
        host: primitives::navigator::NavigatorHost<Self::Node>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_navigator(self, type_id, type_name, presentation, host, a11y)
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        Backend::release_navigator(self, node)
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ) {
        Backend::apply_navigator_slot_style(self, node, slot, style)
    }

    fn make_navigator_handle(&self, node: &Self::Node) -> primitives::navigator::NavigatorHandle {
        Backend::make_navigator_handle(self, node)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        Backend::navigator_attach_initial(self, navigator, screen, scope_id, options)
    }
}

// ===========================================================================
// External + document
// ===========================================================================

impl caps::ExternalOps for EmailBackend {
    fn create_external(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        Backend::create_external(self, type_id, type_name, payload, a11y)
    }

    fn release_external(&mut self, node: &Self::Node) {
        Backend::release_external(self, node)
    }

    fn missing_primitive_placeholder(&mut self, label: &'static str) -> Self::Node {
        Backend::missing_primitive_placeholder(self, label)
    }
}

impl caps::DocumentOps for EmailBackend {
    fn create_element(&mut self, tag: &str) -> Self::Node {
        Backend::create_element(self, tag)
    }

    fn attach_html_id(&self, node: &Self::Node, id: &str) {
        Backend::attach_html_id(self, node, id)
    }

    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        Backend::attach_html_class(self, node, class)
    }

    fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str) {
        Backend::attach_html_style(self, node, prop, value)
    }

    fn register_raw_css(&mut self, css: &str) {
        Backend::register_raw_css(self, css)
    }
}

// ===========================================================================
// Style + assets
// ===========================================================================

impl caps::StyleOps for EmailBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        Backend::apply_style(self, node, style)
    }

    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        Backend::mint_style_class(self, style)
    }

    fn mint_class_for_app(&mut self, app: &StyleApplication) -> Option<String> {
        Backend::mint_class_for_app(self, app)
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        Backend::apply_styled_states(self, node, base, overlays)
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(Breakpoint, Rc<StyleRules>)],
        container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        Backend::apply_styled_variants(
            self,
            node,
            base,
            state_overlays,
            breakpoint_overlays,
            container_overlays,
        )
    }

    fn mark_container(&mut self, node: &Self::Node) {
        Backend::mark_container(self, node)
    }

    fn handles_states_natively(&self) -> bool {
        Backend::handles_states_natively(self)
    }

    fn token_updates_propagate_via_cascade(&self) -> bool {
        Backend::token_updates_propagate_via_cascade(self)
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        Backend::register_stylesheet(self, rules)
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        Backend::unregister_stylesheet(self, rules)
    }

    fn install_tokens(&mut self, tokens: &[TokenEntry]) {
        Backend::install_tokens(self, tokens)
    }

    fn update_tokens(&mut self, tokens: &[TokenEntry]) {
        Backend::update_tokens(self, tokens)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        Backend::on_node_unstyled(self, node)
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        Backend::attach_states(self, node, setter)
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        Backend::set_disabled(self, node, disabled)
    }

    fn supports_preminted_styles(&self) -> bool {
        Backend::supports_preminted_styles(self)
    }

    fn apply_default_text_font(&mut self, font: Option<&FontFamily>) {
        Backend::apply_default_text_font(self, font)
    }

    fn supports_js_class_bindings(&self) -> bool {
        Backend::supports_js_class_bindings(self)
    }

    fn register_reactive_class_binding(
        &mut self,
        node: &Self::Node,
        signal_id: u64,
        values: &[u32],
        classes: &[&str],
        value_reader: Rc<dyn Fn() -> u32>,
    ) -> u32 {
        Backend::register_reactive_class_binding(self, node, signal_id, values, classes, value_reader)
    }

    fn release_reactive_class_binding(&mut self, binding_id: u32) {
        Backend::release_reactive_class_binding(self, binding_id)
    }
}

impl caps::AssetOps for EmailBackend {
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        Backend::register_asset(self, id, kind, source)
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        Backend::unregister_asset(self, id, kind)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        Backend::register_typeface(self, id, family_name, faces, fallback)
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        Backend::unregister_typeface(self, id)
    }
}

// ===========================================================================
// A11y + animation + introspection
// ===========================================================================

impl caps::A11yOps for EmailBackend {
    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &AccessibilityProps,
        inferred_role: Option<Role>,
    ) {
        Backend::update_accessibility(self, node, a11y, inferred_role)
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        Backend::announce_for_accessibility(self, msg, priority)
    }

    fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
        Backend::dump_accessibility_tree(self)
    }
}

impl caps::AnimationOps for EmailBackend {
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        Backend::set_animated_f32(self, node, prop, value)
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        Backend::set_animated_color(self, node, prop, value)
    }
}

impl caps::IntrospectionOps for EmailBackend {
    fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        Backend::frame(self, node)
    }

    fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        Backend::absolute_frame(self, node)
    }

    fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        Backend::device_frame(self, node)
    }

    fn supports_native_introspection(&self) -> bool {
        Backend::supports_native_introspection(self)
    }

    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
        Backend::introspect_native(self, node)
    }

    fn note_introspection_root(&self, node: &Self::Node) {
        Backend::note_introspection_root(self, node)
    }

    fn supports_screenshot(&self) -> bool {
        Backend::supports_screenshot(self)
    }

    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        Backend::capture_screenshot(self, done)
    }
}

// ===========================================================================
// Batch + wire bindings
// ===========================================================================

impl caps::BatchOps for EmailBackend {
    fn supports_batched_repeat(&self) -> bool {
        Backend::supports_batched_repeat(self)
    }

    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
        Backend::execute_batch(self, batch)
    }

    fn execute_batch_with_attach(
        &mut self,
        batch: BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node> {
        Backend::execute_batch_with_attach(self, batch, parent, attach_locals)
    }
}

impl caps::WireBindingOps for EmailBackend {
    fn note_text_binding(&mut self, node: &Self::Node, signal_ids: &[u64], method: &'static str) {
        Backend::note_text_binding(self, node, signal_ids, method)
    }

    fn note_signal_initial(&mut self, signal_id: u64, value: &runtime_core::__serde_json::Value) {
        Backend::note_signal_initial(self, signal_id, value)
    }

    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        Backend::note_when_binding(self, anchor, signal_ids, cond_method, then_node, otherwise_node)
    }

    fn note_switch_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        arms: &[(runtime_core::__serde_json::Value, Self::Node)],
        default_node: &Self::Node,
    ) {
        Backend::note_switch_binding(self, anchor, signal_ids, cond_method, arms, default_node)
    }

    fn note_repeat_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
    ) {
        Backend::note_repeat_binding(
            self,
            anchor,
            signal_ids,
            count_method,
            row_template,
            row_index_signal_id,
        )
    }

    fn note_virtualizer_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
        horizontal: bool,
    ) {
        Backend::note_virtualizer_binding(
            self,
            anchor,
            signal_ids,
            count_method,
            row_template,
            row_index_signal_id,
            horizontal,
        )
    }

    fn supports_lazy_slot_capture(&self) -> bool {
        Backend::supports_lazy_slot_capture(self)
    }

    fn begin_slot_capture(&mut self) {
        Backend::begin_slot_capture(self)
    }

    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        Backend::end_slot_capture(self, slot_root)
    }
}
