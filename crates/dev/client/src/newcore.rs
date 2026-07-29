//! New-core replay target (idea-lite migration — the dev-session wire
//! chain, client half).
//!
//! [`WireBackend`](crate::WireBackend) is the wire replayer: it maps
//! each incoming [`wire::Command`] to one backend call. On the old core
//! those calls go through the `Backend` mega-trait; on the new core a
//! platform's surface is the 30 `runtime_vocabulary::caps` traits (plus
//! [`runtime_scene::Host`] for structure). [`CapsReplay`] is the seam
//! between the two: it wraps any caps-adopted backend and exposes the
//! `Backend` surface the replayer's per-command dispatch drives, with
//! **every** replay-relevant method UFCS-delegating to the matching
//! capability trait (the exact inverse of
//! `runtime_vocabulary::bridge::LegacyBridge`, which adapts a `Backend`
//! to the caps).
//!
//! ```text
//! Commands → WireBackend<CapsReplay<B>> → caps::*Ops on B → native UI
//! ```
//!
//! # Why an adapter instead of re-bounding `WireBackend` on the caps
//!
//! The replayer is consumed by every runtime-server client shell —
//! iOS / Android / macOS hosts, the wgpu sim, the browser transport,
//! `mock-backend`, `headless-screenshot` — all of which today construct
//! it around a plain `Backend` (several share the backend `Rc` with a
//! renderer that reads it as its concrete type). Re-bounding the
//! generic would force every one of those builds to enable its
//! backend's `new-core` feature in the same graph — the exact coupling
//! the dual-core wave keeps apart. The adapter inverts the dependency:
//! the old constructor keeps working unchanged, while a new-core
//! embedding replays through the caps surface only —
//! `tests/newcore_caps_replay.rs` pins that a backend implementing
//! ONLY `Host` + the caps (no `Backend` impl at all) can be driven
//! end-to-end. When the `Backend` trait is deleted, `WireBackend`'s
//! internal dispatch re-bounds onto the caps and this adapter dissolves
//! — a change contained entirely in this crate.
//!
//! # Delegation contract
//!
//! Same rules as `LegacyBridge`, mirrored:
//!
//! - every method delegates with UFCS (`caps::TextOps::update_text(&mut
//!   self.0, …)`) so the wrapped backend's own overrides always win and
//!   no identically-named `Backend` default can shadow a cap;
//! - the two renamed structural ops map `Backend::create_reactive_anchor`
//!   → `Host::create_anchor` and `Backend::supports_child_splice` →
//!   `Host::supports_splice`;
//! - `Backend` methods with no caps counterpart (the walker-only
//!   surface the replayer never calls) keep their trait defaults.

use std::any::{Any, TypeId};
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
use runtime_scene::Host;
use runtime_vocabulary::caps;

use crate::{OutboundSender, WireBackend};

/// Adapts a caps-adopted backend (`Host` + the 30 `caps::*Ops` traits)
/// to the `Backend` surface [`WireBackend`]'s command dispatch drives.
/// See the module docs for the delegation contract.
pub struct CapsReplay<B: caps::AllCaps>(pub B);

/// The new-core replay client: wire commands in, capability calls out.
pub type NewCoreReplayClient<B> = WireBackend<CapsReplay<B>>;

impl<B: caps::AllCaps + 'static> WireBackend<CapsReplay<B>> {
    /// Construct a replay client that drives `backend`'s capability
    /// surface. New-core counterpart of [`WireBackend::new`].
    pub fn new_newcore(backend: B, outbound: impl Into<OutboundSender>) -> Self {
        WireBackend::new(CapsReplay(backend), outbound)
    }
}

impl<B> Backend for CapsReplay<B>
where
    B: caps::AllCaps + 'static,
{
    type Node = <B as Host>::Node;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node){
        Host::insert(&mut self.0, parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>){
        Host::insert_many(&mut self.0, parent, children)
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize){
        Host::insert_at(&mut self.0, parent, child, index)
    }

    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node){
        Host::remove_child(&mut self.0, parent, child)
    }

    fn clear_children(&mut self, node: &Self::Node){
        Host::clear_children(&mut self.0, node)
    }

    fn create_reactive_anchor(&mut self) -> Self::Node{
        Host::create_anchor(&mut self.0)
    }

    fn supports_child_splice(&self) -> bool{
        Host::supports_splice(&self.0)
    }

    fn color_scheme(&self) -> ColorScheme{
        caps::AppEnvOps::color_scheme(&self.0)
    }

    fn platform(&self) -> Platform{
        caps::AppEnvOps::platform(&self.0)
    }

    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>>{
        caps::AppEnvOps::url_opener(&self.0)
    }

    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>>{
        caps::AppEnvOps::fullscreen_setter(&self.0)
    }

    fn set_page_metadata(&mut self, meta: &PageMetadata){
        caps::AppEnvOps::set_page_metadata(&mut self.0, meta)
    }

    fn set_app_background(&mut self, color: &Tokenized<Color>){
        caps::AppEnvOps::set_app_background(&mut self.0, color)
    }

    fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>){
        caps::AppEnvOps::set_scrollbar_theme(&mut self.0, thumb, track)
    }

    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>){
        caps::AppEnvOps::set_app_key_handler(&mut self.0, handler)
    }

    fn finish(&mut self, root: Self::Node){
        caps::LifecycleOps::finish(&mut self.0, root)
    }

    fn run_layout(&mut self){
        caps::LifecycleOps::run_layout(&mut self.0)
    }

    fn schedule_layout_pass(){
        <B as caps::LifecycleOps>::schedule_layout_pass()
    }

    fn is_hydrating(&self) -> bool{
        caps::LifecycleOps::is_hydrating(&self.0)
    }

    fn renders_lazy_chunks(&self) -> bool{
        caps::LifecycleOps::renders_lazy_chunks(&self.0)
    }

    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node{
        caps::ViewOps::create_view(&mut self.0, a11y)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle{
        caps::ViewOps::make_view_handle(&self.0, node)
    }

    fn install_touch_handler(&mut self, node: &Self::Node, handler: TouchHandler){
        caps::InputOps::install_touch_handler(&mut self.0, node, handler)
    }

    fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId){
        caps::InputOps::claim_touch(&mut self.0, node, touch_id)
    }

    fn install_wheel_handler(&mut self, node: &Self::Node, handler: WheelHandler){
        caps::InputOps::install_wheel_handler(&mut self.0, node, handler)
    }

    fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler){
        caps::InputOps::install_hover_handler(&mut self.0, node, handler)
    }

    fn mark_preserves_focus(&mut self, node: &Self::Node){
        caps::InputOps::mark_preserves_focus(&mut self.0, node)
    }

    fn install_file_drop_handler(&mut self, node: &Self::Node, handler: FileDropHandler){
        caps::InputOps::install_file_drop_handler(&mut self.0, node, handler)
    }

    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node{
        caps::PressableOps::create_pressable(&mut self.0, on_click, a11y)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_core::PressableHandle{
        caps::PressableOps::make_pressable_handle(&self.0, node)
    }

    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node{
        caps::TextOps::create_text(&mut self.0, content, a11y)
    }

    fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node{
        caps::TextOps::create_styled_text(&mut self.0, runs, a11y)
    }

    fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]){
        caps::TextOps::update_styled_text(&mut self.0, node, runs)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str){
        caps::TextOps::update_text(&mut self.0, node, content)
    }

    fn create_text_with_id(
        &mut self,
        content: &str,
        a11y: &AccessibilityProps,
    ) -> Option<(Self::Node, u32)>{
        caps::TextOps::create_text_with_id(&mut self.0, content, a11y)
    }

    fn update_text_by_id(&mut self, id: u32, content: String){
        caps::TextOps::update_text_by_id(&mut self.0, id, content)
    }

    fn release_text_id(&mut self, id: u32){
        caps::TextOps::release_text_id(&mut self.0, id)
    }

    fn supports_js_text_bindings(&self) -> bool{
        caps::TextOps::supports_js_text_bindings(&self.0)
    }

    fn register_reactive_text_binding(
        &mut self,
        text_id: u32,
        signal_ids: &[u64],
        template_parts: &[&str],
        initial_values: &[&str],
        stringifiers: &[Rc<dyn Fn() -> String>],
    ){
        caps::TextOps::register_reactive_text_binding(&mut self.0, text_id, signal_ids, template_parts, initial_values, stringifiers)
    }

    fn release_reactive_text_binding(&mut self, text_id: u32){
        caps::TextOps::release_reactive_text_binding(&mut self.0, text_id)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle{
        caps::TextOps::make_text_handle(&self.0, node)
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::ButtonOps::create_button(&mut self.0, label, on_click, leading_icon, trailing_icon, a11y)
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str){
        caps::ButtonOps::update_button_label(&mut self.0, node, label)
    }

    fn make_button_handle(&self, node: &Self::Node) -> runtime_core::ButtonHandle{
        caps::ButtonOps::make_button_handle(&self.0, node)
    }

    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> Self::Node{
        caps::ImageOps::create_image(&mut self.0, src, alt, a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str){
        caps::ImageOps::update_image_src(&mut self.0, node, src)
    }

    fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>){
        caps::ImageOps::update_image_alt(&mut self.0, node, alt)
    }

    fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler){
        caps::ImageOps::install_image_load_handler(&mut self.0, node, handler)
    }

    fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler){
        caps::ImageOps::install_image_error_handler(&mut self.0, node, handler)
    }

    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle{
        caps::ImageOps::make_image_handle(&self.0, node)
    }

    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::IconOps::create_icon(&mut self.0, data, color, a11y)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color){
        caps::IconOps::update_icon_color(&mut self.0, node, color)
    }

    fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData){
        caps::IconOps::update_icon_data(&mut self.0, node, data)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32){
        caps::IconOps::update_icon_stroke(&mut self.0, node, progress)
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
    ){
        caps::IconOps::animate_icon_stroke(&mut self.0, node, from, to, duration_ms, easing, infinite, autoreverses)
    }

    fn make_icon_handle(&self, node: &Self::Node) -> primitives::icon::IconHandle{
        caps::IconOps::make_icon_handle(&self.0, node)
    }

    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::LinkOps::create_link(&mut self.0, config, a11y)
    }

    fn update_link_url(&mut self, node: &Self::Node, url: &str){
        caps::LinkOps::update_link_url(&mut self.0, node, url)
    }

    fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle{
        caps::LinkOps::make_link_handle(&self.0, node)
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        secure: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::TextInputOps::create_text_input(&mut self.0, initial_value, placeholder, on_change, on_key_down, on_blur, secure, a11y)
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str){
        caps::TextInputOps::update_text_input_value(&mut self.0, node, value)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool){
        caps::TextInputOps::update_text_input_secure(&mut self.0, node, secure)
    }

    fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>){
        caps::TextInputOps::set_text_input_focus_handler(&mut self.0, node, handler)
    }

    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>){
        caps::TextInputOps::update_text_input_placeholder(&mut self.0, node, placeholder)
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
    ) -> Self::Node{
        caps::TextInputOps::create_text_area(&mut self.0, initial_value, placeholder, wrap, min_rows, max_rows, on_change, on_key_down, a11y)
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str){
        caps::TextInputOps::update_text_area_value(&mut self.0, node, value)
    }

    fn make_text_input_handle(&self, node: &Self::Node) -> primitives::text_input::TextInputHandle{
        caps::TextInputOps::make_text_input_handle(&self.0, node)
    }

    fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle{
        caps::TextInputOps::make_text_area_handle(&self.0, node)
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::ToggleOps::create_toggle(&mut self.0, initial_value, on_change, a11y)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool){
        caps::ToggleOps::update_toggle_value(&mut self.0, node, value)
    }

    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle{
        caps::ToggleOps::make_toggle_handle(&self.0, node)
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::SliderOps::create_slider(&mut self.0, initial_value, min, max, step, on_change, a11y)
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32){
        caps::SliderOps::update_slider_value(&mut self.0, node, value)
    }

    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle{
        caps::SliderOps::make_slider_handle(&self.0, node)
    }

    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::ActivityIndicatorOps::create_activity_indicator(&mut self.0, size, color, a11y)
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &Self::Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ){
        caps::ActivityIndicatorOps::update_activity_indicator_size(&mut self.0, node, size)
    }

    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle{
        caps::ActivityIndicatorOps::make_activity_indicator_handle(&self.0, node)
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::ScrollOps::create_scroll_view(&mut self.0, horizontal, on_scroll, a11y)
    }

    fn node_scroll(&self, node: &Self::Node) -> (f32, f32){
        caps::ScrollOps::node_scroll(&self.0, node)
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32){
        caps::ScrollOps::set_node_scroll(&mut self.0, node, x, y)
    }

    fn make_scroll_view_handle(&self, node: &Self::Node) -> primitives::scroll_view::ScrollViewHandle{
        caps::ScrollOps::make_scroll_view_handle(&self.0, node)
    }

    fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides){
        caps::SafeAreaOps::apply_safe_area_padding(&mut self.0, node, sides)
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides){
        caps::SafeAreaOps::apply_scroll_view_safe_area_inset(&mut self.0, node, sides)
    }

    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::VirtualizerOps::create_virtualizer(&mut self.0, callbacks, overscan, layout, a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node){
        caps::VirtualizerOps::virtualizer_data_changed(&mut self.0, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node){
        caps::VirtualizerOps::release_virtualizer(&mut self.0, node)
    }

    fn make_virtualizer_handle(&self, node: &Self::Node) -> primitives::virtualizer::VirtualizerHandle{
        caps::VirtualizerOps::make_virtualizer_handle(&self.0, node)
    }

    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::GraphicsOps::create_graphics(&mut self.0, on_ready, on_resize, on_lost, a11y)
    }

    fn release_graphics(&mut self, node: &Self::Node){
        caps::GraphicsOps::release_graphics(&mut self.0, node)
    }

    fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle{
        caps::GraphicsOps::make_graphics_handle(&self.0, node)
    }

    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::PortalOps::create_portal(&mut self.0, target, on_dismiss, trap_focus, a11y)
    }

    fn release_portal(&mut self, node: &Self::Node){
        caps::PortalOps::release_portal(&mut self.0, node)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool){
        caps::PortalOps::set_portal_hidden(&mut self.0, node, hidden)
    }

    fn make_portal_handle(&self, node: &Self::Node) -> primitives::portal::PortalHandle{
        caps::PortalOps::make_portal_handle(&self.0, node)
    }

    fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node{
        caps::PresenceOps::create_presence_placeholder(&mut self.0, a11y)
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ){
        caps::PresenceOps::apply_presence(&mut self.0, node, state, transition)
    }

    fn make_presence_handle(&self, node: &Self::Node) -> primitives::presence::PresenceHandle{
        caps::PresenceOps::make_presence_handle(&self.0, node)
    }

    fn create_navigator(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        presentation: Rc<dyn Any>,
        host: primitives::navigator::NavigatorHost<Self::Node>,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::NavigatorOps::create_navigator(&mut self.0, type_id, type_name, presentation, host, a11y)
    }

    fn release_navigator(&mut self, node: &Self::Node){
        caps::NavigatorOps::release_navigator(&mut self.0, node)
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ){
        caps::NavigatorOps::apply_navigator_slot_style(&mut self.0, node, slot, style)
    }

    fn make_navigator_handle(&self, node: &Self::Node) -> primitives::navigator::NavigatorHandle{
        caps::NavigatorOps::make_navigator_handle(&self.0, node)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn Any>,
    ){
        caps::NavigatorOps::navigator_attach_initial(&mut self.0, navigator, screen, scope_id, options)
    }

    fn create_external(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
        a11y: &AccessibilityProps,
    ) -> Self::Node{
        caps::ExternalOps::create_external(&mut self.0, type_id, type_name, payload, a11y)
    }

    fn release_external(&mut self, node: &Self::Node){
        caps::ExternalOps::release_external(&mut self.0, node)
    }

    fn missing_primitive_placeholder(&mut self, label: &'static str) -> Self::Node{
        caps::ExternalOps::missing_primitive_placeholder(&mut self.0, label)
    }

    fn create_element(&mut self, tag: &str) -> Self::Node{
        caps::DocumentOps::create_element(&mut self.0, tag)
    }

    fn attach_html_id(&self, node: &Self::Node, id: &str){
        caps::DocumentOps::attach_html_id(&self.0, node, id)
    }

    fn attach_html_class(&self, node: &Self::Node, class: &str){
        caps::DocumentOps::attach_html_class(&self.0, node, class)
    }

    fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str){
        caps::DocumentOps::attach_html_style(&self.0, node, prop, value)
    }

    fn register_raw_css(&mut self, css: &str){
        caps::DocumentOps::register_raw_css(&mut self.0, css)
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>){
        caps::StyleOps::apply_style(&mut self.0, node, style)
    }

    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String>{
        caps::StyleOps::mint_style_class(&mut self.0, style)
    }

    fn mint_class_for_app(&mut self, app: &StyleApplication) -> Option<String>{
        caps::StyleOps::mint_class_for_app(&mut self.0, app)
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(StateBits, Rc<StyleRules>)],
    ){
        caps::StyleOps::apply_styled_states(&mut self.0, node, base, overlays)
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(Breakpoint, Rc<StyleRules>)],
        container_overlays: &[(f32, Rc<StyleRules>)],
    ){
        caps::StyleOps::apply_styled_variants(&mut self.0, node, base, state_overlays, breakpoint_overlays, container_overlays)
    }

    fn mark_container(&mut self, node: &Self::Node){
        caps::StyleOps::mark_container(&mut self.0, node)
    }

    fn handles_states_natively(&self) -> bool{
        caps::StyleOps::handles_states_natively(&self.0)
    }

    fn token_updates_propagate_via_cascade(&self) -> bool{
        caps::StyleOps::token_updates_propagate_via_cascade(&self.0)
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]){
        caps::StyleOps::register_stylesheet(&mut self.0, rules)
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]){
        caps::StyleOps::unregister_stylesheet(&mut self.0, rules)
    }

    fn install_tokens(&mut self, tokens: &[TokenEntry]){
        caps::StyleOps::install_tokens(&mut self.0, tokens)
    }

    fn update_tokens(&mut self, tokens: &[TokenEntry]){
        caps::StyleOps::update_tokens(&mut self.0, tokens)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node){
        caps::StyleOps::on_node_unstyled(&mut self.0, node)
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>){
        caps::StyleOps::attach_states(&mut self.0, node, setter)
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool){
        caps::StyleOps::set_disabled(&mut self.0, node, disabled)
    }

    fn supports_preminted_styles(&self) -> bool{
        caps::StyleOps::supports_preminted_styles(&self.0)
    }

    fn apply_default_text_font(&mut self, font: Option<&FontFamily>){
        caps::StyleOps::apply_default_text_font(&mut self.0, font)
    }

    fn supports_js_class_bindings(&self) -> bool{
        caps::StyleOps::supports_js_class_bindings(&self.0)
    }

    fn register_reactive_class_binding(
        &mut self,
        node: &Self::Node,
        signal_id: u64,
        values: &[u32],
        classes: &[&str],
        value_reader: Rc<dyn Fn() -> u32>,
    ) -> u32{
        caps::StyleOps::register_reactive_class_binding(&mut self.0, node, signal_id, values, classes, value_reader)
    }

    fn release_reactive_class_binding(&mut self, binding_id: u32){
        caps::StyleOps::release_reactive_class_binding(&mut self.0, binding_id)
    }

    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource){
        caps::AssetOps::register_asset(&mut self.0, id, kind, source)
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag){
        caps::AssetOps::unregister_asset(&mut self.0, id, kind)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ){
        caps::AssetOps::register_typeface(&mut self.0, id, family_name, faces, fallback)
    }

    fn unregister_typeface(&mut self, id: TypefaceId){
        caps::AssetOps::unregister_typeface(&mut self.0, id)
    }

    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &AccessibilityProps,
        inferred_role: Option<Role>,
    ){
        caps::A11yOps::update_accessibility(&mut self.0, node, a11y, inferred_role)
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority){
        caps::A11yOps::announce_for_accessibility(&mut self.0, msg, priority)
    }

    fn dump_accessibility_tree(&self) -> Option<AccessibilityTree>{
        caps::A11yOps::dump_accessibility_tree(&self.0)
    }

    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32){
        caps::AnimationOps::set_animated_f32(&mut self.0, node, prop, value)
    }

    fn frame(&self, node: &Self::Node) -> Option<ViewportRect>{
        caps::IntrospectionOps::frame(&self.0, node)
    }

    fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect>{
        caps::IntrospectionOps::absolute_frame(&self.0, node)
    }

    fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect>{
        caps::IntrospectionOps::device_frame(&self.0, node)
    }

    fn supports_native_introspection(&self) -> bool{
        caps::IntrospectionOps::supports_native_introspection(&self.0)
    }

    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode>{
        caps::IntrospectionOps::introspect_native(&self.0, node)
    }

    fn note_introspection_root(&self, node: &Self::Node){
        caps::IntrospectionOps::note_introspection_root(&self.0, node)
    }

    fn supports_screenshot(&self) -> bool{
        caps::IntrospectionOps::supports_screenshot(&self.0)
    }

    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>){
        caps::IntrospectionOps::capture_screenshot(&self.0, done)
    }

    fn supports_batched_repeat(&self) -> bool{
        caps::BatchOps::supports_batched_repeat(&self.0)
    }

    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node>{
        caps::BatchOps::execute_batch(&mut self.0, batch)
    }

    fn execute_batch_with_attach(
        &mut self,
        batch: BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node>{
        caps::BatchOps::execute_batch_with_attach(&mut self.0, batch, parent, attach_locals)
    }

    fn note_text_binding(&mut self, node: &Self::Node, signal_ids: &[u64], method: &'static str){
        caps::WireBindingOps::note_text_binding(&mut self.0, node, signal_ids, method)
    }

    fn note_signal_initial(&mut self, signal_id: u64, value: &runtime_core::__serde_json::Value){
        caps::WireBindingOps::note_signal_initial(&mut self.0, signal_id, value)
    }

    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ){
        caps::WireBindingOps::note_when_binding(&mut self.0, anchor, signal_ids, cond_method, then_node, otherwise_node)
    }

    fn note_switch_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        arms: &[(runtime_core::__serde_json::Value, Self::Node)],
        default_node: &Self::Node,
    ){
        caps::WireBindingOps::note_switch_binding(&mut self.0, anchor, signal_ids, cond_method, arms, default_node)
    }

    fn note_repeat_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
    ){
        caps::WireBindingOps::note_repeat_binding(&mut self.0, anchor, signal_ids, count_method, row_template, row_index_signal_id)
    }

    fn note_virtualizer_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
        horizontal: bool,
    ){
        caps::WireBindingOps::note_virtualizer_binding(&mut self.0, anchor, signal_ids, count_method, row_template, row_index_signal_id, horizontal)
    }

    fn supports_lazy_slot_capture(&self) -> bool{
        caps::WireBindingOps::supports_lazy_slot_capture(&self.0)
    }

    fn begin_slot_capture(&mut self){
        caps::WireBindingOps::begin_slot_capture(&mut self.0)
    }

    fn end_slot_capture(&mut self, slot_root: &Self::Node){
        caps::WireBindingOps::end_slot_capture(&mut self.0, slot_root)
    }
}
