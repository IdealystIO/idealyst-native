//! Native GTK4 backend — scaffold.
//!
//! Implements `runtime_core::Backend` over real GTK4 widgets. Author
//! code that mounts on Linux gets a real `gtk::Box` container (View),
//! `gtk::Label` (Text), and `gtk::Button` widget; every other
//! primitive renders a placeholder label so the missing widget is
//! visible at run-time rather than panicking via the framework's
//! `unimplemented!()` defaults.
//!
//! The placeholder posture matches `backend-cpu` and `backend-windows` —
//! silent no-ops hide the gap, visible labels surface it. See
//! `feedback_cpu_unsupported_placeholders`.
//!
//! ## Threading
//!
//! GTK4 is single-threaded — all widget operations must happen on
//! the main GTK thread. The host shell wraps the backend in a
//! `glib::MainContext` callback path; the backend assumes it's
//! invoked on the right thread and calls GTK inline.
//!
//! ## Build gating
//!
//! The lib body is gated on `cfg(target_os = "linux")`. On macOS /
//! Windows hosts the crate compiles to an empty rlib so workspace
//! builds don't pull `gtk4` (and its `glib-sys` / `cairo-sys`
//! transitive deps) into the dep graph. Don't put cross-platform
//! code here — it belongs in `runtime-core`.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{Action, Backend, Color, ColorScheme, Platform, StyleRules};
use runtime_layout::LayoutTree;

use gtk4::glib;
use gtk4::prelude::*;

// =========================================================================
// Node
// =========================================================================

/// Backend handle for a mounted GTK widget. Holds a strong ref to
/// the widget; cloning shares the underlying GObject reference,
/// matching framework `Clone` semantics.
#[derive(Clone)]
pub struct LinuxNode {
    pub(crate) id: u64,
    pub(crate) widget: gtk4::Widget,
}

impl std::fmt::Debug for LinuxNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxNode")
            .field("id", &self.id)
            .field("widget_type", &self.widget.type_().name())
            .finish()
    }
}

// =========================================================================
// Backend
// =========================================================================

pub struct LinuxBackend {
    /// Top-level window owned by the host shell. The backend
    /// doesn't `show` or `destroy` it — that's the host's job.
    #[allow(dead_code)]
    host_window: gtk4::Window,
    next_id: u64,
    pub(crate) layout: LayoutTree,
    layout_for_id: HashMap<u64, runtime_layout::LayoutNode>,
}

impl LinuxBackend {
    /// Construct a backend rooted at `host_window`. The window must
    /// already be realized by the host before any widget operations
    /// happen — typically the host calls `application.add_window()`
    /// and `window.present()` before handing the window in.
    pub fn new(host_window: gtk4::Window) -> Self {
        Self {
            host_window,
            next_id: 1,
            layout: LayoutTree::new(),
            layout_for_id: HashMap::new(),
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn wrap(&mut self, widget: gtk4::Widget) -> LinuxNode {
        let id = self.alloc_id();
        let layout = self.layout.new_node();
        self.layout_for_id.insert(id, layout);
        LinuxNode { id, widget }
    }

    fn placeholder(&mut self, message: &str) -> LinuxNode {
        let label = gtk4::Label::new(Some(message));
        // Distinguish placeholders visually from real labels by
        // setting a CSS class the host's default theme can pick up.
        label.add_css_class("idealyst-placeholder");
        self.wrap(label.upcast::<gtk4::Widget>())
    }
}

// =========================================================================
// Backend trait
// =========================================================================

impl Backend for LinuxBackend {
    type Node = LinuxNode;

    fn color_scheme(&self) -> ColorScheme {
        // GTK4 exposes the system dark-mode preference via
        // `gtk::Settings::default().gtk_application_prefer_dark_theme`,
        // but the canonical signal is `gtk::StyleContext::settings`'s
        // `prefer_dark_theme` property combined with the system
        // freedesktop color-scheme setting. For the scaffold we
        // return Auto and let the framework's theme APIs decide.
        ColorScheme::Auto
    }

    fn platform(&self) -> Platform {
        Platform::Custom("linux")
    }

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        // gtk::Box with orientation::Vertical — vertical-flex default
        // matches the framework's default Taffy flex_direction. The
        // layout pass will override per-node as styles resolve.
        let widget = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        self.wrap(widget.upcast::<gtk4::Widget>())
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        let label = gtk4::Label::new(Some(content));
        label.set_wrap(true);
        label.set_xalign(0.0);
        self.wrap(label.upcast::<gtk4::Widget>())
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&runtime_core::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_core::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let button = gtk4::Button::with_label(label);
        let fire = on_click.fire.clone();
        button.connect_clicked(move |_| (fire)());
        self.wrap(button.upcast::<gtk4::Widget>())
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // GTK's `GestureClick` controller gives us a press event on
        // any widget. Use it against a transparent gtk::Box so the
        // primitive looks like a plain View externally.
        let widget = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let gesture = gtk4::GestureClick::new();
        let fire = on_click.clone();
        gesture.connect_released(move |_, _, _, _| (fire)());
        widget.add_controller(gesture);
        self.wrap(widget.upcast::<gtk4::Widget>())
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let Some(parent_layout) = self.layout_for_id.get(&parent.id).copied() else {
            return;
        };
        let Some(child_layout) = self.layout_for_id.get(&child.id).copied() else {
            return;
        };
        self.layout.add_child(parent_layout, child_layout);
        // GTK4 dropped `Container::add` — children attach via
        // widget-specific methods. `gtk::Box::append` is the closest
        // analog for vertical-stacked children. For non-Box parents
        // (Button, Label, etc.) this is a no-op; those primitives
        // shouldn't have framework children mounted under them.
        if let Some(box_) = parent.widget.downcast_ref::<gtk4::Box>() {
            box_.append(&child.widget);
        }
    }

    fn clear_children(&mut self, node: &Self::Node) {
        if let Some(box_) = node.widget.downcast_ref::<gtk4::Box>() {
            // Walk children and remove. `gtk::Box::first_child` +
            // `next_sibling` is the GTK4 iteration pattern after
            // `Container` removal.
            let mut child = box_.first_child();
            while let Some(c) = child {
                let next = c.next_sibling();
                box_.remove(&c);
                child = next;
            }
        }
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        if let Some(label) = node.widget.downcast_ref::<gtk4::Label>() {
            label.set_text(content);
        }
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        if let Some(btn) = node.widget.downcast_ref::<gtk4::Button>() {
            btn.set_label(label);
        }
    }

    fn finish(&mut self, _root: Self::Node) {
        // Real impl: compute layout, walk every registered widget,
        // and call `widget.size_allocate(...)` with the computed
        // frame. Skipped in the scaffold — GTK's own size-allocate
        // pass via the container hierarchy still produces a usable
        // initial layout, just not Taffy-driven.
    }

    // ---------------------------------------------------------------------
    // Placeholders. Matching backend-cpu / backend-windows posture.
    // ---------------------------------------------------------------------

    fn create_image(
        &mut self,
        _src: &str,
        _alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Image not yet implemented on Linux backend")
    }

    fn create_icon(
        &mut self,
        _data: &runtime_core::primitives::icon::IconData,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Icon not yet implemented on Linux backend")
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // gtk::Entry is the canonical single-line text editor. Wire
        // initial value here; on_change wiring lands in the follow-up
        // PR alongside placeholder string + key handler routing.
        let entry = gtk4::Entry::new();
        entry.set_text(initial_value);
        self.wrap(entry.upcast::<gtk4::Widget>())
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // gtk::TextView is the multi-line editor. Wrap in a
        // gtk::ScrolledWindow so long content scrolls naturally — a
        // bare TextView has no scrollbar.
        let view = gtk4::TextView::new();
        view.buffer().set_text(initial_value);
        let scrolled = gtk4::ScrolledWindow::new();
        scrolled.set_child(Some(&view));
        self.wrap(scrolled.upcast::<gtk4::Widget>())
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let switch = gtk4::Switch::new();
        switch.set_active(initial_value);
        let fire = on_change.clone();
        switch.connect_state_notify(move |s| (fire)(s.is_active()));
        self.wrap(switch.upcast::<gtk4::Widget>())
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let scale = gtk4::Scale::with_range(
            gtk4::Orientation::Horizontal,
            min as f64,
            max as f64,
            // step_increment — GTK's keyboard step. Drag returns
            // continuous values regardless.
            1.0,
        );
        scale.set_value(initial_value as f64);
        let fire = on_change.clone();
        scale.connect_value_changed(move |s| (fire)(s.value() as f32));
        self.wrap(scale.upcast::<gtk4::Widget>())
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let scrolled = gtk4::ScrolledWindow::new();
        // Disable the axis the author didn't ask for. GTK's default
        // is "show scrollbars on both axes when needed".
        if horizontal {
            scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Never);
        } else {
            scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        }
        // Wrap a gtk::Box as the document so children attach via
        // `insert`.
        let inner = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        scrolled.set_child(Some(&inner));
        // Return the ScrolledWindow as the node; subsequent insert
        // calls go through the gtk::Box downcast path which won't
        // match — pending: track the inner Box separately so
        // insert() routes to it. For the scaffold, children mount
        // directly under the ScrolledWindow's set_child slot via a
        // future refinement.
        self.wrap(scrolled.upcast::<gtk4::Widget>())
    }

    fn create_activity_indicator(
        &mut self,
        _size: runtime_core::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // gtk::Spinner is GTK's spinning loading indicator.
        let spinner = gtk4::Spinner::new();
        spinner.start();
        self.wrap(spinner.upcast::<gtk4::Widget>())
    }

    fn create_virtualizer(
        &mut self,
        _callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        _horizontal: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Virtualizer not yet implemented on Linux backend")
    }

    fn create_graphics(
        &mut self,
        _on_ready: runtime_core::primitives::graphics::OnReady,
        _on_resize: runtime_core::primitives::graphics::OnResize,
        _on_lost: runtime_core::primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Graphics not yet implemented on Linux backend")
    }

    fn create_external(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder(&format!(
            "External \"{type_name}\" not yet implemented on Linux backend"
        ))
    }

    fn create_portal(
        &mut self,
        _target: runtime_core::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Portal not yet implemented on Linux backend")
    }

    fn create_navigator(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _presentation: Rc<dyn std::any::Any>,
        _host: runtime_core::primitives::navigator::NavigatorHost<Self::Node>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder(&format!(
            "Navigator \"{type_name}\" not yet implemented on Linux backend"
        ))
    }

    fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {
        // No-op until we wire Taffy-driven size_allocate in finish().
        // Author code calling apply_style today shouldn't crash; the
        // style is silently dropped.
    }
}

// Keep `glib` import live for the eventual host-thread bridge.
#[allow(dead_code)]
type _KeepGlib = glib::MainContext;
