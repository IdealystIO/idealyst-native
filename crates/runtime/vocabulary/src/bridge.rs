//! [`LegacyBridge`] — adapts any existing `runtime_core::Backend` to
//! the split capability surface.
//!
//! Implements [`runtime_scene::Host`] plus **every** `caps::*Ops` trait
//! by delegating each method to the wrapped Backend with UFCS
//! (`<B as Backend>::method(&mut self.0, …)`), so:
//!
//! - the wrapped backend's own overrides always win (a bridge method
//!   never falls back to a caps-trait default), and
//! - there is no way to recurse into the bridge's identically-named
//!   trait methods by accident.
//!
//! Because every Ops trait is implemented explicitly, `LegacyBridge<B>`
//! satisfies the blanket [`AllCaps`](crate::caps::AllCaps) impl for any
//! `B: Backend` — the compile-time proof that the signature freeze is
//! faithful and that every existing backend gets the full capability
//! surface with zero edits to backend crates.
//!
//! Host mapping (names differ between the P1 seam and the mega-trait):
//! `create_anchor` → `Backend::create_reactive_anchor`, and
//! `supports_splice` → `Backend::supports_child_splice`. The `'static`
//! bounds come from `Host`'s requirements (structural regions retain
//! node handles across effect fires); every real backend satisfies them.

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

use crate::caps;

/// Wraps a legacy `Backend` impl and exposes it through the capability
/// traits. See the module docs for the delegation contract.
pub struct LegacyBridge<B: Backend>(pub B);

// ---------------------------------------------------------------------------
// Host — the P1 structural seam
// ---------------------------------------------------------------------------

impl<B> Host for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    type Node = B::Node;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        <B as Backend>::insert(&mut self.0, parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        <B as Backend>::insert_many(&mut self.0, parent, children)
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        <B as Backend>::insert_at(&mut self.0, parent, child, index)
    }

    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        <B as Backend>::remove_child(&mut self.0, parent, child)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        <B as Backend>::clear_children(&mut self.0, node)
    }

    fn create_anchor(&mut self) -> Self::Node {
        <B as Backend>::create_reactive_anchor(&mut self.0)
    }

    fn supports_splice(&self) -> bool {
        <B as Backend>::supports_child_splice(&self.0)
    }
}

// ---------------------------------------------------------------------------
// App environment + lifecycle
// ---------------------------------------------------------------------------

impl<B> caps::AppEnvOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn color_scheme(&self) -> ColorScheme {
        <B as Backend>::color_scheme(&self.0)
    }

    fn platform(&self) -> Platform {
        <B as Backend>::platform(&self.0)
    }

    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        <B as Backend>::url_opener(&self.0)
    }

    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
        <B as Backend>::fullscreen_setter(&self.0)
    }

    fn set_page_metadata(&mut self, meta: &PageMetadata) {
        <B as Backend>::set_page_metadata(&mut self.0, meta)
    }

    fn set_app_background(&mut self, color: &Tokenized<Color>) {
        <B as Backend>::set_app_background(&mut self.0, color)
    }

    fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>) {
        <B as Backend>::set_scrollbar_theme(&mut self.0, thumb, track)
    }

    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
        <B as Backend>::set_app_key_handler(&mut self.0, handler)
    }
}

impl<B> caps::LifecycleOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn finish(&mut self, root: Self::Node) {
        <B as Backend>::finish(&mut self.0, root)
    }

    fn run_layout(&mut self) {
        <B as Backend>::run_layout(&mut self.0)
    }

    fn schedule_layout_pass() {
        <B as Backend>::schedule_layout_pass()
    }

    fn is_hydrating(&self) -> bool {
        <B as Backend>::is_hydrating(&self.0)
    }

    fn renders_lazy_chunks(&self) -> bool {
        <B as Backend>::renders_lazy_chunks(&self.0)
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl<B> caps::ViewOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        <B as Backend>::create_view(&mut self.0, a11y)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        <B as Backend>::make_view_handle(&self.0, node)
    }
}

impl<B> caps::InputOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn install_touch_handler(&mut self, node: &Self::Node, handler: TouchHandler) {
        <B as Backend>::install_touch_handler(&mut self.0, node, handler)
    }

    fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId) {
        <B as Backend>::claim_touch(&mut self.0, node, touch_id)
    }

    fn install_wheel_handler(&mut self, node: &Self::Node, handler: WheelHandler) {
        <B as Backend>::install_wheel_handler(&mut self.0, node, handler)
    }

    fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
        <B as Backend>::install_hover_handler(&mut self.0, node, handler)
    }

    fn mark_preserves_focus(&mut self, node: &Self::Node) {
        <B as Backend>::mark_preserves_focus(&mut self.0, node)
    }

    fn install_file_drop_handler(&mut self, node: &Self::Node, handler: FileDropHandler) {
        <B as Backend>::install_file_drop_handler(&mut self.0, node, handler)
    }
}

impl<B> caps::PressableOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        <B as Backend>::create_pressable(&mut self.0, on_click, a11y)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_core::PressableHandle {
        <B as Backend>::make_pressable_handle(&self.0, node)
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl<B> caps::TextOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
        <B as Backend>::create_text(&mut self.0, content, a11y)
    }

    fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
        <B as Backend>::create_styled_text(&mut self.0, runs, a11y)
    }

    fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
        <B as Backend>::update_styled_text(&mut self.0, node, runs)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        <B as Backend>::update_text(&mut self.0, node, content)
    }

    fn create_text_with_id(
        &mut self,
        content: &str,
        a11y: &AccessibilityProps,
    ) -> Option<(Self::Node, u32)> {
        <B as Backend>::create_text_with_id(&mut self.0, content, a11y)
    }

    fn update_text_by_id(&mut self, id: u32, content: String) {
        <B as Backend>::update_text_by_id(&mut self.0, id, content)
    }

    fn release_text_id(&mut self, id: u32) {
        <B as Backend>::release_text_id(&mut self.0, id)
    }

    fn supports_js_text_bindings(&self) -> bool {
        <B as Backend>::supports_js_text_bindings(&self.0)
    }

    fn register_reactive_text_binding(
        &mut self,
        text_id: u32,
        signal_ids: &[u64],
        template_parts: &[&str],
        initial_values: &[&str],
        stringifiers: &[Rc<dyn Fn() -> String>],
    ) {
        <B as Backend>::register_reactive_text_binding(
            &mut self.0,
            text_id,
            signal_ids,
            template_parts,
            initial_values,
            stringifiers,
        )
    }

    fn release_reactive_text_binding(&mut self, text_id: u32) {
        <B as Backend>::release_reactive_text_binding(&mut self.0, text_id)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        <B as Backend>::make_text_handle(&self.0, node)
    }
}

impl<B> caps::ButtonOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_button(&mut self.0, label, on_click, leading_icon, trailing_icon, a11y)
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        <B as Backend>::update_button_label(&mut self.0, node, label)
    }

    fn make_button_handle(&self, node: &Self::Node) -> runtime_core::ButtonHandle {
        <B as Backend>::make_button_handle(&self.0, node)
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl<B> caps::ImageOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> Self::Node {
        <B as Backend>::create_image(&mut self.0, src, alt, a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        <B as Backend>::update_image_src(&mut self.0, node, src)
    }

    fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
        <B as Backend>::update_image_alt(&mut self.0, node, alt)
    }

    fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
        <B as Backend>::install_image_load_handler(&mut self.0, node, handler)
    }

    fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
        <B as Backend>::install_image_error_handler(&mut self.0, node, handler)
    }

    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
        <B as Backend>::make_image_handle(&self.0, node)
    }
}

impl<B> caps::IconOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_icon(&mut self.0, data, color, a11y)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        <B as Backend>::update_icon_color(&mut self.0, node, color)
    }

    fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData) {
        <B as Backend>::update_icon_data(&mut self.0, node, data)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        <B as Backend>::update_icon_stroke(&mut self.0, node, progress)
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
        <B as Backend>::animate_icon_stroke(
            &mut self.0,
            node,
            from,
            to,
            duration_ms,
            easing,
            infinite,
            autoreverses,
        )
    }

    fn make_icon_handle(&self, node: &Self::Node) -> primitives::icon::IconHandle {
        <B as Backend>::make_icon_handle(&self.0, node)
    }
}

impl<B> caps::LinkOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_link(&mut self.0, config, a11y)
    }

    fn update_link_url(&mut self, node: &Self::Node, url: &str) {
        <B as Backend>::update_link_url(&mut self.0, node, url)
    }

    fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
        <B as Backend>::make_link_handle(&self.0, node)
    }
}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl<B> caps::TextInputOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
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
        <B as Backend>::create_text_input(
            &mut self.0,
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
        <B as Backend>::update_text_input_value(&mut self.0, node, value)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        <B as Backend>::update_text_input_secure(&mut self.0, node, secure)
    }

    fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {
        <B as Backend>::set_text_input_focus_handler(&mut self.0, node, handler)
    }

    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
        <B as Backend>::update_text_input_placeholder(&mut self.0, node, placeholder)
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
        <B as Backend>::create_text_area(
            &mut self.0,
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
        <B as Backend>::update_text_area_value(&mut self.0, node, value)
    }

    fn make_text_input_handle(&self, node: &Self::Node) -> primitives::text_input::TextInputHandle {
        <B as Backend>::make_text_input_handle(&self.0, node)
    }

    fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
        <B as Backend>::make_text_area_handle(&self.0, node)
    }
}

impl<B> caps::ToggleOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_toggle(&mut self.0, initial_value, on_change, a11y)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        <B as Backend>::update_toggle_value(&mut self.0, node, value)
    }

    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
        <B as Backend>::make_toggle_handle(&self.0, node)
    }
}

impl<B> caps::SliderOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_slider(&mut self.0, initial_value, min, max, step, on_change, a11y)
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        <B as Backend>::update_slider_value(&mut self.0, node, value)
    }

    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
        <B as Backend>::make_slider_handle(&self.0, node)
    }
}

impl<B> caps::ActivityIndicatorOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_activity_indicator(&mut self.0, size, color, a11y)
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &Self::Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        <B as Backend>::update_activity_indicator_size(&mut self.0, node, size)
    }

    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle {
        <B as Backend>::make_activity_indicator_handle(&self.0, node)
    }
}

// ---------------------------------------------------------------------------
// Scroll + safe area + virtualizer
// ---------------------------------------------------------------------------

impl<B> caps::ScrollOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_scroll_view(&mut self.0, horizontal, on_scroll, a11y)
    }

    fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
        <B as Backend>::node_scroll(&self.0, node)
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
        <B as Backend>::set_node_scroll(&mut self.0, node, x, y)
    }

    fn make_scroll_view_handle(&self, node: &Self::Node) -> primitives::scroll_view::ScrollViewHandle {
        <B as Backend>::make_scroll_view_handle(&self.0, node)
    }
}

impl<B> caps::SafeAreaOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        <B as Backend>::apply_safe_area_padding(&mut self.0, node, sides)
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        <B as Backend>::apply_scroll_view_safe_area_inset(&mut self.0, node, sides)
    }
}

impl<B> caps::VirtualizerOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_virtualizer(&mut self.0, callbacks, overscan, layout, a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        <B as Backend>::virtualizer_data_changed(&mut self.0, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        <B as Backend>::release_virtualizer(&mut self.0, node)
    }

    fn make_virtualizer_handle(&self, node: &Self::Node) -> primitives::virtualizer::VirtualizerHandle {
        <B as Backend>::make_virtualizer_handle(&self.0, node)
    }
}

// ---------------------------------------------------------------------------
// Graphics + portal + presence + navigator
// ---------------------------------------------------------------------------

impl<B> caps::GraphicsOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_graphics(&mut self.0, on_ready, on_resize, on_lost, a11y)
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        <B as Backend>::release_graphics(&mut self.0, node)
    }

    fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle {
        <B as Backend>::make_graphics_handle(&self.0, node)
    }
}

impl<B> caps::PortalOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_portal(&mut self.0, target, on_dismiss, trap_focus, a11y)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        <B as Backend>::release_portal(&mut self.0, node)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        <B as Backend>::set_portal_hidden(&mut self.0, node, hidden)
    }

    fn make_portal_handle(&self, node: &Self::Node) -> primitives::portal::PortalHandle {
        <B as Backend>::make_portal_handle(&self.0, node)
    }
}

impl<B> caps::PresenceOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        <B as Backend>::create_presence_placeholder(&mut self.0, a11y)
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        <B as Backend>::apply_presence(&mut self.0, node, state, transition)
    }

    fn make_presence_handle(&self, node: &Self::Node) -> primitives::presence::PresenceHandle {
        <B as Backend>::make_presence_handle(&self.0, node)
    }
}

impl<B> caps::NavigatorOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_navigator(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        presentation: Rc<dyn Any>,
        host: primitives::navigator::NavigatorHost<Self::Node>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_navigator(&mut self.0, type_id, type_name, presentation, host, a11y)
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        <B as Backend>::release_navigator(&mut self.0, node)
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ) {
        <B as Backend>::apply_navigator_slot_style(&mut self.0, node, slot, style)
    }

    fn make_navigator_handle(&self, node: &Self::Node) -> primitives::navigator::NavigatorHandle {
        <B as Backend>::make_navigator_handle(&self.0, node)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        <B as Backend>::navigator_attach_initial(&mut self.0, navigator, screen, scope_id, options)
    }
}

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl<B> caps::ExternalOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_external(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <B as Backend>::create_external(&mut self.0, type_id, type_name, payload, a11y)
    }

    fn release_external(&mut self, node: &Self::Node) {
        <B as Backend>::release_external(&mut self.0, node)
    }

    fn missing_primitive_placeholder(&mut self, label: &'static str) -> Self::Node {
        <B as Backend>::missing_primitive_placeholder(&mut self.0, label)
    }
}

impl<B> caps::DocumentOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn create_element(&mut self, tag: &str) -> Self::Node {
        <B as Backend>::create_element(&mut self.0, tag)
    }

    fn attach_html_id(&self, node: &Self::Node, id: &str) {
        <B as Backend>::attach_html_id(&self.0, node, id)
    }

    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        <B as Backend>::attach_html_class(&self.0, node, class)
    }

    fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str) {
        <B as Backend>::attach_html_style(&self.0, node, prop, value)
    }

    fn register_raw_css(&mut self, css: &str) {
        <B as Backend>::register_raw_css(&mut self.0, css)
    }
}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl<B> caps::StyleOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        <B as Backend>::apply_style(&mut self.0, node, style)
    }

    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        <B as Backend>::mint_style_class(&mut self.0, style)
    }

    fn mint_class_for_app(&mut self, app: &StyleApplication) -> Option<String> {
        <B as Backend>::mint_class_for_app(&mut self.0, app)
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        <B as Backend>::apply_styled_states(&mut self.0, node, base, overlays)
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(Breakpoint, Rc<StyleRules>)],
        container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        <B as Backend>::apply_styled_variants(
            &mut self.0,
            node,
            base,
            state_overlays,
            breakpoint_overlays,
            container_overlays,
        )
    }

    fn mark_container(&mut self, node: &Self::Node) {
        <B as Backend>::mark_container(&mut self.0, node)
    }

    fn handles_states_natively(&self) -> bool {
        <B as Backend>::handles_states_natively(&self.0)
    }

    fn token_updates_propagate_via_cascade(&self) -> bool {
        <B as Backend>::token_updates_propagate_via_cascade(&self.0)
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        <B as Backend>::register_stylesheet(&mut self.0, rules)
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        <B as Backend>::unregister_stylesheet(&mut self.0, rules)
    }

    fn install_tokens(&mut self, tokens: &[TokenEntry]) {
        <B as Backend>::install_tokens(&mut self.0, tokens)
    }

    fn update_tokens(&mut self, tokens: &[TokenEntry]) {
        <B as Backend>::update_tokens(&mut self.0, tokens)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        <B as Backend>::on_node_unstyled(&mut self.0, node)
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        <B as Backend>::attach_states(&mut self.0, node, setter)
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        <B as Backend>::set_disabled(&mut self.0, node, disabled)
    }

    fn supports_preminted_styles(&self) -> bool {
        <B as Backend>::supports_preminted_styles(&self.0)
    }

    fn apply_default_text_font(&mut self, font: Option<&FontFamily>) {
        <B as Backend>::apply_default_text_font(&mut self.0, font)
    }

    fn supports_js_class_bindings(&self) -> bool {
        <B as Backend>::supports_js_class_bindings(&self.0)
    }

    fn register_reactive_class_binding(
        &mut self,
        node: &Self::Node,
        signal_id: u64,
        values: &[u32],
        classes: &[&str],
        value_reader: Rc<dyn Fn() -> u32>,
    ) -> u32 {
        <B as Backend>::register_reactive_class_binding(
            &mut self.0,
            node,
            signal_id,
            values,
            classes,
            value_reader,
        )
    }

    fn release_reactive_class_binding(&mut self, binding_id: u32) {
        <B as Backend>::release_reactive_class_binding(&mut self.0, binding_id)
    }
}

impl<B> caps::AssetOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        <B as Backend>::register_asset(&mut self.0, id, kind, source)
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        <B as Backend>::unregister_asset(&mut self.0, id, kind)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        <B as Backend>::register_typeface(&mut self.0, id, family_name, faces, fallback)
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        <B as Backend>::unregister_typeface(&mut self.0, id)
    }
}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl<B> caps::A11yOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &AccessibilityProps,
        inferred_role: Option<Role>,
    ) {
        <B as Backend>::update_accessibility(&mut self.0, node, a11y, inferred_role)
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        <B as Backend>::announce_for_accessibility(&mut self.0, msg, priority)
    }

    fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
        <B as Backend>::dump_accessibility_tree(&self.0)
    }
}

impl<B> caps::AnimationOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        <B as Backend>::set_animated_f32(&mut self.0, node, prop, value)
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        <B as Backend>::set_animated_color(&mut self.0, node, prop, value)
    }
}

impl<B> caps::IntrospectionOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <B as Backend>::frame(&self.0, node)
    }

    fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <B as Backend>::absolute_frame(&self.0, node)
    }

    fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <B as Backend>::device_frame(&self.0, node)
    }

    fn supports_native_introspection(&self) -> bool {
        <B as Backend>::supports_native_introspection(&self.0)
    }

    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
        <B as Backend>::introspect_native(&self.0, node)
    }

    fn note_introspection_root(&self, node: &Self::Node) {
        <B as Backend>::note_introspection_root(&self.0, node)
    }

    fn supports_screenshot(&self) -> bool {
        <B as Backend>::supports_screenshot(&self.0)
    }

    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        <B as Backend>::capture_screenshot(&self.0, done)
    }
}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl<B> caps::BatchOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn supports_batched_repeat(&self) -> bool {
        <B as Backend>::supports_batched_repeat(&self.0)
    }

    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
        <B as Backend>::execute_batch(&mut self.0, batch)
    }

    fn execute_batch_with_attach(
        &mut self,
        batch: BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node> {
        <B as Backend>::execute_batch_with_attach(&mut self.0, batch, parent, attach_locals)
    }
}

impl<B> caps::WireBindingOps for LegacyBridge<B>
where
    B: Backend + 'static,
    B::Node: 'static,
{
    fn note_text_binding(&mut self, node: &Self::Node, signal_ids: &[u64], method: &'static str) {
        <B as Backend>::note_text_binding(&mut self.0, node, signal_ids, method)
    }

    fn note_signal_initial(&mut self, signal_id: u64, value: &runtime_core::__serde_json::Value) {
        <B as Backend>::note_signal_initial(&mut self.0, signal_id, value)
    }

    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        <B as Backend>::note_when_binding(
            &mut self.0,
            anchor,
            signal_ids,
            cond_method,
            then_node,
            otherwise_node,
        )
    }

    fn note_switch_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        arms: &[(runtime_core::__serde_json::Value, Self::Node)],
        default_node: &Self::Node,
    ) {
        <B as Backend>::note_switch_binding(
            &mut self.0,
            anchor,
            signal_ids,
            cond_method,
            arms,
            default_node,
        )
    }

    fn note_repeat_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
    ) {
        <B as Backend>::note_repeat_binding(
            &mut self.0,
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
        <B as Backend>::note_virtualizer_binding(
            &mut self.0,
            anchor,
            signal_ids,
            count_method,
            row_template,
            row_index_signal_id,
            horizontal,
        )
    }

    fn supports_lazy_slot_capture(&self) -> bool {
        <B as Backend>::supports_lazy_slot_capture(&self.0)
    }

    fn begin_slot_capture(&mut self) {
        <B as Backend>::begin_slot_capture(&mut self.0)
    }

    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        <B as Backend>::end_slot_capture(&mut self.0, slot_root)
    }
}
