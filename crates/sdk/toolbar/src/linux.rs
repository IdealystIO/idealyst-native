//! Linux (GTK4) implementation of the toolbar SDK.
//!
//! Builds a [`GtkHeaderBar`] populated with buttons matching the
//! reactive [`ToolbarProps::items`] closure, then sets it as the
//! host window's titlebar via `window.set_titlebar()`. HeaderBar is
//! the modern GTK4 replacement for the deprecated `GtkToolbar` and
//! is the closest equivalent to macOS's `NSToolbar` — it replaces
//! the system's default titlebar with a custom widget tree that
//! still gets window-decoration treatment from the compositor
//! (close/minimize/maximize buttons appear at the right by default).
//!
//! # Reactive items
//!
//! Same `Effect::new`-driven rebuild as the macOS impl: on each
//! re-fire we wipe the HeaderBar's packed children and append fresh
//! buttons from the new `items` Vec.
//!
//! # Tree position
//!
//! Unlike the macOS NSToolbar (where the in-tree placeholder is 0×0
//! and invisible), the GTK HeaderBar lives as the window's titlebar
//! — entirely outside the framework's content tree. The framework-
//! returned `LinuxNode` for the Toolbar primitive is therefore a
//! zero-size empty `gtk::Box`. Where you mount it in the `ui!` tree
//! doesn't affect rendering, same model as macOS.

use crate::{ToolbarItem, ToolbarOps, ToolbarProps};
use backend_linux::{LinuxBackend, LinuxNode};
use gtk4::prelude::*;
use runtime_core::Effect;
use std::any::Any;
use std::rc::Rc;

pub(crate) static OPS: &dyn ToolbarOps = &LinuxToolbarOps;

pub fn register(backend: &mut LinuxBackend) {
    backend.register_external::<ToolbarProps, _>(|props, b| build_toolbar(props, b));
}

// =========================================================================
// Build + reactive items
// =========================================================================

fn build_toolbar(props: &Rc<ToolbarProps>, b: &mut LinuxBackend) -> LinuxNode {
    let window = b.host_window().clone();

    // Build the HeaderBar and install as the window's titlebar.
    // `show_title_buttons = true` keeps the standard close /
    // minimize / maximize controls at the right side; our packed
    // buttons land on the left via `pack_start`.
    let headerbar = gtk4::HeaderBar::new();
    headerbar.set_show_title_buttons(true);
    window.set_titlebar(Some(&headerbar));

    if !props.visible {
        headerbar.set_visible(false);
    }

    // Reactive items: every re-fire reads `props.items()` and
    // rebuilds the HeaderBar's packed children.
    let headerbar_for_effect = headerbar.clone();
    let props_for_effect = props.clone();
    let _items_effect = Effect::new(move || {
        let items = (props_for_effect.items)();
        apply_items(&headerbar_for_effect, items);
    });

    // Return a zero-size placeholder widget for the framework's
    // tree. Empty `gtk::Box` with no expand flags — Taffy gives it
    // 0 width/height by default if the author doesn't style it.
    let placeholder: gtk4::Widget =
        gtk4::Box::new(gtk4::Orientation::Horizontal, 0).upcast();
    b.register_external_view(placeholder)
}

/// Wipe the HeaderBar's children and append fresh buttons from a
/// new items vec. Honors button labels + click callbacks + tooltips.
/// Separator / Space / FlexibleSpace items currently render as
/// fixed-width spacers — GTK HeaderBar doesn't have a true
/// separator item, but a `gtk::Box` with `width-request` matches
/// the visual intent.
fn apply_items(headerbar: &gtk4::HeaderBar, items: Vec<ToolbarItem>) {
    // Wipe existing children. HeaderBar lets us iterate via the
    // first_child / next_sibling chain (no direct children() in
    // GTK4 the way GTK3 had).
    let mut child = headerbar.first_child();
    while let Some(c) = child {
        let next = c.next_sibling();
        // `remove` accepts any descendant; safe to call on each
        // direct child regardless of which pack_start/end slot
        // it occupies.
        headerbar.remove(&c);
        child = next;
    }

    for item in items {
        match item {
            ToolbarItem::Button(btn) => {
                let label = btn.label.clone();
                let button = gtk4::Button::with_label(&label);
                if let Some(tooltip) = &btn.tooltip {
                    button.set_tooltip_text(Some(tooltip));
                }
                if let Some(cb) = btn.on_click {
                    button.connect_clicked(move |_| cb());
                }
                // Icon: GTK uses icon-name themed lookup (Freedesktop
                // icon spec). For v1 we leave icons unwired — the
                // SF Symbol names the macOS leaf uses don't translate
                // directly; mapping to standard icon-name strings
                // is a follow-up.
                let _ = btn.icon;
                headerbar.pack_start(&button);
            }
            ToolbarItem::Separator | ToolbarItem::Space => {
                let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
                spacer.set_width_request(8);
                headerbar.pack_start(&spacer);
            }
            ToolbarItem::FlexibleSpace => {
                // HeaderBar packs from start + end; a "flexible
                // space" effectively means "all subsequent items go
                // on the right". The cleanest approximation is to
                // pack a hexpanded empty box as the divider. Items
                // already packed via `pack_start` stay on the left;
                // we can't retroactively flip the next batch to
                // `pack_end` without restructuring. For v1 the
                // flexible space renders as an expanding spacer
                // packed at the start position — visually it pushes
                // following items to the right within the
                // HeaderBar's flex layout.
                let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
                spacer.set_hexpand(true);
                headerbar.pack_start(&spacer);
            }
        }
    }
}

// =========================================================================
// Imperative ops
// =========================================================================

struct LinuxToolbarOps;

impl ToolbarOps for LinuxToolbarOps {
    fn set_visible(&self, _node: &dyn Any, _visible: bool) {
        // The placeholder LinuxNode doesn't carry a HeaderBar ref
        // back here (the HeaderBar lives on the window, not in the
        // tree). Implementing this needs a thread-local anchor
        // (mirror the macOS macOS LAST_TARGET pattern from the menu
        // SDK) tracking the most-recently-installed HeaderBar so
        // set_visible can toggle it. Tracked as a follow-up; v1
        // is a no-op so user code keeps compiling without crashing.
    }
}
