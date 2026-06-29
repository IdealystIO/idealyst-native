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
    /// We also use it as the size source in `finish()` so Taffy
    /// computes against the window's actual width × height.
    pub(crate) host_window: gtk4::Window,
    /// Root `gtk::Fixed` we install as the window's child once.
    /// All top-level View / Pressable / ScrollView containers
    /// attach as children of this root (re-parented in `insert`
    /// once their framework parent attaches).
    pub(crate) root_fixed: gtk4::Fixed,
    next_id: u64,
    pub(crate) layout: LayoutTree,
    layout_for_id: HashMap<u64, runtime_layout::LayoutNode>,
    /// Every wrapped widget keyed by its node id — `finish()`
    /// walks this to issue `fixed.move_()` + `set_size_request()`
    /// per the Taffy frame. Stored as `Widget` (the GObject base)
    /// because containers and leaves share the same positioning
    /// surface in GTK4.
    widgets: HashMap<u64, gtk4::Widget>,
    /// Third-party `Element::External` registry. Populated by
    /// `register_external::<T>(...)` calls from per-platform leaf
    /// crates. `create_external` looks the handler up by payload
    /// TypeId; unregistered kinds fall through to a "not supported"
    /// placeholder label. Mirrors the iOS / macOS / Windows pattern.
    pub(crate) external_handlers: runtime_core::ExternalRegistry<LinuxBackend>,
}

impl LinuxBackend {
    /// Construct a backend rooted at `host_window`. The window must
    /// already be realized by the host before any widget operations
    /// happen — typically the host calls `application.add_window()`
    /// and `window.present()` before handing the window in.
    pub fn new(host_window: gtk4::Window) -> Self {
        // Install a root `gtk::Fixed` as the window's child. The
        // framework's logical roots ride on top of this — they
        // attach in `insert()` when the framework's root attaches
        // to its parent, but the actual GTK parent for the
        // top-most container is always this root_fixed. Without a
        // single root we'd have no place to set the host window's
        // size constraints from inside `finish()`.
        let root_fixed = gtk4::Fixed::new();
        host_window.set_child(Some(&root_fixed));
        Self {
            host_window,
            root_fixed,
            next_id: 1,
            layout: LayoutTree::new(),
            layout_for_id: HashMap::new(),
            widgets: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
        }
    }

    /// Borrow the host `gtk::Window`. SDK extensions (the `menu`
    /// SDK installing a GMenu via `set_show_menubar` / future
    /// toolbar leaf packing buttons into a `GtkHeaderBar`) reach
    /// the window through this.
    pub fn host_window(&self) -> &gtk4::Window {
        &self.host_window
    }

    /// Register a handler for the third-party external primitive
    /// whose payload type is `T`. Called by per-platform leaf crates
    /// during app bootstrap (`toolbar::register(&mut backend)`).
    /// Mirrors the iOS / macOS / Windows pattern.
    pub fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&Rc<T>, &mut LinuxBackend) -> LinuxNode + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }

    /// `true` if a handler for payload type `T` has been registered.
    pub fn has_external<T: 'static>(&self) -> bool {
        self.external_handlers.has::<T>()
    }

    /// SDK extension helper: register an existing widget with the
    /// backend's layout tree so flex parents can size + position it.
    /// Returns the wrapped LinuxNode. Mirrors
    /// `IosBackend::register_external_view` /
    /// `WindowsBackend::register_external_view`.
    pub fn register_external_view(&mut self, widget: gtk4::Widget) -> LinuxNode {
        self.wrap(widget)
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
        self.widgets.insert(id, widget.clone());
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
        // gtk::Fixed — absolute-positioning container that takes
        // its children's (x, y) from our own `finish()` layout
        // pass. We deliberately don't use `gtk::Box` here because
        // Box's auto-stacking fights Taffy's frame assignments;
        // every container in the framework's flex tree needs to
        // be Fixed so finish() can write the computed position
        // directly via `fixed.move_()`.
        let widget = gtk4::Fixed::new();
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
        // Same container shape as `create_view` — a `gtk::Fixed`
        // so children land at Taffy-computed coordinates — with a
        // `GestureClick` controller mounted on top so the whole
        // surface reports a press. The framework's Pressable is
        // semantically a "transparent View that fires a callback"
        // and that's exactly what this gives us.
        let widget = gtk4::Fixed::new();
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

        // GTK4 attach pattern: `gtk::Fixed::put(child, x, y)` adds
        // a child at absolute (x, y) within the container.
        // Initial coordinates are (0, 0); `finish()` walks every
        // registered widget and calls `fixed.move_()` once Taffy
        // has computed the real frame.
        //
        // For ScrolledWindow parents we route to the inner Fixed
        // installed by `create_scroll_view` — the outer
        // ScrolledWindow takes exactly one child (the scrollable
        // document), and that child is always our Fixed. Author-
        // supplied children mount inside the Fixed, NOT as a
        // sibling document replacing it.
        //
        // Leaf widgets (Button, Label, etc.) aren't containers —
        // author code shouldn't try to mount children inside
        // them; if it does, this call is a no-op rather than a
        // panic.
        if let Some(fixed) = parent.widget.downcast_ref::<gtk4::Fixed>() {
            fixed.put(&child.widget, 0.0, 0.0);
        } else if let Some(scrolled) = parent.widget.downcast_ref::<gtk4::ScrolledWindow>() {
            if let Some(inner) = scrolled
                .child()
                .and_then(|c| c.downcast::<gtk4::Fixed>().ok())
            {
                inner.put(&child.widget, 0.0, 0.0);
            }
        }
    }

    fn clear_children(&mut self, node: &Self::Node) {
        // Walk + remove via the GTK4 `first_child`/`next_sibling`
        // iteration. Works for any `gtk::Widget` that has children;
        // the per-container removal API depends on the concrete
        // type (Fixed::remove, Box::remove, ScrolledWindow::
        // set_child(None)).
        if let Some(fixed) = node.widget.downcast_ref::<gtk4::Fixed>() {
            let mut child = fixed.first_child();
            while let Some(c) = child {
                let next = c.next_sibling();
                fixed.remove(&c);
                child = next;
            }
        } else if let Some(scrolled) = node.widget.downcast_ref::<gtk4::ScrolledWindow>() {
            // The inner document is our own `gtk::Fixed`. Clear
            // its children but keep the Fixed itself — author code
            // can still mount fresh children after a clear, and a
            // ScrolledWindow with no document widget would lose
            // its scrollbar slot machinery.
            if let Some(inner) = scrolled
                .child()
                .and_then(|c| c.downcast::<gtk4::Fixed>().ok())
            {
                let mut child = inner.first_child();
                while let Some(c) = child {
                    let next = c.next_sibling();
                    inner.remove(&c);
                    child = next;
                }
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

    fn finish(&mut self, root: Self::Node) {
        // First mount: attach the framework's root container to our
        // root `gtk::Fixed`. Only the first time — subsequent
        // `finish()` calls (re-render after data changes) keep the
        // same root attached, just re-position.
        if root.widget.parent().is_none() {
            self.root_fixed.put(&root.widget, 0.0, 0.0);
        }

        // Compute against the host window's allocated size. Before
        // the window is realized + presented, `width()`/`height()`
        // return 0; bail in that case so Taffy doesn't compute
        // against a degenerate viewport. The framework will call
        // `finish()` again after the first GTK allocate pass once
        // the window has real bounds.
        let width = self.host_window.width() as f32;
        let height = self.host_window.height() as f32;
        if width <= 0.0 || height <= 0.0 {
            return;
        }

        let Some(root_layout) = self.layout_for_id.get(&root.id).copied() else {
            return;
        };
        self.layout.compute(root_layout, width, height);

        // Walk every registered widget and project its Taffy frame
        // into GTK's positioning surface. `set_size_request`
        // pins the widget's min size so GTK's own allocate pass
        // honors the Taffy width × height. `fixed.move_()` repositions
        // a child that's already attached to a Fixed parent.
        //
        // We split the walk into a collect-then-apply pass so the
        // GTK calls don't alias the borrow on `self.widgets` /
        // `self.layout_for_id`.
        let mut updates: Vec<(gtk4::Widget, f32, f32, i32, i32)> =
            Vec::with_capacity(self.widgets.len());
        for (id, widget) in &self.widgets {
            let Some(layout) = self.layout_for_id.get(id).copied() else {
                continue;
            };
            let frame = self.layout.frame_of(layout);
            updates.push((
                widget.clone(),
                frame.x,
                frame.y,
                frame.width.round() as i32,
                frame.height.round() as i32,
            ));
        }
        for (widget, x, y, w, h) in updates {
            widget.set_size_request(w, h);
            if let Some(parent) = widget.parent() {
                if let Some(fixed) = parent.downcast_ref::<gtk4::Fixed>() {
                    fixed.move_(&widget, x as f64, y as f64);
                }
                // Non-Fixed parents (Buttons accepting a Label
                // child, ScrolledWindow holding our inner Fixed)
                // don't have a coordinate concept — leave their
                // positioning to GTK's own allocate pass.
            }
        }
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
        _on_blur: Option<runtime_core::primitives::text_input::BlurHandler>,
        secure: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // gtk::Entry is the canonical single-line text editor. Wire
        // initial value here; on_change wiring lands in the follow-up
        // PR alongside placeholder string + key handler routing.
        let entry = gtk4::Entry::new();
        entry.set_text(initial_value);
        // Password masking: GTK's Entry hides typed characters (shows
        // the invisible-char bullet) when visibility is off.
        if secure {
            entry.set_visibility(false);
        }
        self.wrap(entry.upcast::<gtk4::Widget>())
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        // GTK masks by hiding the entry's characters; `visibility = !secure`
        // toggles it in place on the same Entry.
        if let Some(entry) = node.widget.downcast_ref::<gtk4::Entry>() {
            entry.set_visibility(!secure);
        }
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _wrap: bool,
        _min_rows: Option<u32>,
        _max_rows: Option<u32>,
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
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
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
        // Inner document = `gtk::Fixed` so children mount via the
        // standard Fixed `put()`/`move_()` path. ScrolledWindow's
        // `set_child` takes exactly one widget; that one widget is
        // our document container. Author-supplied children attach
        // to the framework's logical ScrollView, which the host's
        // `insert` redirects to this inner Fixed via the
        // downcast-to-ScrolledWindow branch.
        let inner = gtk4::Fixed::new();
        scrolled.set_child(Some(&inner));

        // Wire `on_scroll` via the ScrolledWindow's adjustments.
        // GTK4 exposes one `gtk::Adjustment` per axis (`hadjustment` /
        // `vadjustment`); the `value-changed` signal fires whenever
        // the adjustment's `value` (the scroll offset, in widget
        // coordinates) changes \u{2014} touchpad scroll, scroll bar
        // drag, programmatic `set_value`, all of them.
        //
        // We connect to BOTH axes regardless of `horizontal` so the
        // callback observes the disabled axis too (it stays at 0
        // there, matching every other backend). The closure is
        // cloned per signal since GTK's connect API takes `Fn` by
        // ownership and we attach twice.
        if let Some(cb) = on_scroll {
            use gtk4::prelude::*;
            let cb_for_h = cb.clone();
            let scrolled_for_h = scrolled.clone();
            scrolled
                .hadjustment()
                .connect_value_changed(move |adj| {
                    let x = adj.value() as f32;
                    let y = scrolled_for_h.vadjustment().value() as f32;
                    cb_for_h(x, y);
                });
            let scrolled_for_v = scrolled.clone();
            scrolled
                .vadjustment()
                .connect_value_changed(move |adj| {
                    let x = scrolled_for_v.hadjustment().value() as f32;
                    let y = adj.value() as f32;
                    cb(x, y);
                });
        }

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
        _layout: runtime_core::VirtualLayout,
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
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Look up the registered handler for `type_id`; if found,
        // invoke with the typed payload + `&mut self`; otherwise
        // render a labeled placeholder so the missing SDK is visible.
        // Mirrors the iOS / macOS / Windows posture.
        if let Some(handler) = self.external_handlers.get(type_id) {
            return handler(payload, self);
        }
        self.placeholder(&format!(
            "External \"{type_name}\" not registered on Linux backend"
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
