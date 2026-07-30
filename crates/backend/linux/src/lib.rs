//! Native GTK4 backend — scaffold.
//!
//! Implements `runtime_shared::Backend` over real GTK4 widgets. Author
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

use runtime_layout::LayoutTree;

use gtk4::glib;
use gtk4::prelude::*;

// Post-dispatch flush-hook slot (new-core flush driver). Unconditional
// — the fire sites live in the out-of-repo host shell, which cannot
// see this crate's features; no-op default so the old core never pays.
pub mod dispatch_hook;

/// `runtime_scene::Host` + the 30 capability traits on [`LinuxBackend`],
/// plus the boot entry and flush driver.
pub mod newcore;

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
        }
    }

    /// Borrow the host `gtk::Window`. SDK extensions (the `menu`
    /// SDK installing a GMenu via `set_show_menubar` / future
    /// toolbar leaf packing buttons into a `GtkHeaderBar`) reach
    /// the window through this.
    pub fn host_window(&self) -> &gtk4::Window {
        &self.host_window
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


// Keep `glib` import live for the eventual host-thread bridge.
#[allow(dead_code)]
type _KeepGlib = glib::MainContext;
