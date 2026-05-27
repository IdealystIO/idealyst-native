//! Linux (GTK4) implementation of the menu-bar SDK.
//!
//! Builds a `gio::Menu` model from the [`MenuBarSpec`], wraps it in a
//! `GtkPopoverMenuBar`, and packs the bar at the top of the host
//! window's content area (above the framework's root `gtk::Fixed`).
//! Each command is wired through a window-scoped `gio::SimpleAction`
//! whose `activate` signal fires the Rust closure.
//!
//! # Action naming
//!
//! GMenu items reference actions by **scoped name** — `"win.save"`,
//! `"win.quit"`, etc. We use the `win` scope (window-action group)
//! because `LinuxBackend.host_window` is just a `gtk::Window`, not
//! necessarily a `gtk::ApplicationWindow`, so the `app` scope isn't
//! guaranteed to exist. Per-install action names are namespaced with
//! a monotonically increasing index (`win.cmd0`, `win.cmd1`, …) so
//! re-installs don't collide with stale handlers.
//!
//! # Re-installation
//!
//! Calling [`install`] a second time:
//! - Removes the previous PopoverMenuBar from the window's vbox.
//! - Drops the previous `SimpleActionGroup` (the window's
//!   `insert_action_group("win", None)` clears it).
//! - Builds the new menu + actions from the new spec.
//!
//! # Keyboard shortcuts (v1)
//!
//! `GMenuItem.set_attribute("accel", "<Primary>s")` sets the visible
//! accelerator hint in the menu. Actually dispatching the keypress
//! needs `gtk::Application::set_accels_for_action(...)` — which
//! requires the host to use `gtk::Application` + per-action accel
//! mapping. For v1 the SDK only sets the visible hint; full accel
//! dispatch lands when we add a real Linux host crate.

use crate::{Menu, MenuBarSpec, MenuCommand, MenuItem, Modifiers, Shortcut};
use backend_linux::LinuxBackend;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::RefCell;

/// Install (or replace) the menu bar on the host window. Idempotent
/// per call; safe to invoke multiple times to swap the bar.
pub fn install(backend: &mut LinuxBackend, spec: MenuBarSpec) {
    let window = backend.host_window().clone();

    // Build the action group + GMenu model in tandem so the actions
    // each menu item references are present by the time the menubar
    // widget renders. `action_index` ticks once per command so each
    // gets a unique name (`win.cmd0`, `win.cmd1`, …).
    let action_group = gio::SimpleActionGroup::new();
    let menu_model = gio::Menu::new();
    let mut idx: u32 = 0;

    for menu in &spec.menus {
        let submenu = build_submenu(&menu.items, &action_group, &mut idx);
        menu_model.append_submenu(Some(&menu.title), &submenu);
    }

    // Attach the action group under the "win" scope; menu items
    // reference its actions via `win.cmdN`. A second install
    // replaces the previously-attached group on the same name.
    window.insert_action_group("win", Some(&action_group));

    // Build the PopoverMenuBar widget driven by the GMenu model.
    let menubar = gtk4::PopoverMenuBar::from_model(Some(&menu_model));

    // Pack into a vbox above the existing content. LinuxBackend
    // sets `host_window.child = root_fixed` at construction; we
    // swap that for `[menubar, root_fixed]` packed into a vbox so
    // both the menu bar AND the framework's render tree are
    // visible. The vbox is itself stored in a thread-local anchor
    // so the NEXT install can find + reuse it (or rebuild it).
    LAST_MENUBAR.with(|cell| {
        let mut state = cell.borrow_mut();
        // If a previous install already replaced the window's
        // child with a vbox, just swap the menubar inside it.
        if let Some(prev_vbox) = state.vbox.as_ref() {
            // Remove the prior menubar (always the first child).
            if let Some(prev_bar) = state.menubar.take() {
                prev_vbox.remove(&prev_bar);
            }
            prev_vbox.prepend(&menubar);
            state.menubar = Some(menubar.upcast::<gtk4::Widget>());
            return;
        }

        // First install: extract the window's current child (should
        // be the backend's `root_fixed`), build a vbox with
        // [menubar, root_fixed], install vbox as the new child.
        let Some(existing_child) = window.child() else {
            // No existing child — unusual, but defensive. Just
            // install menubar alone; the framework will get its
            // first widget mounted into the vbox via the layout
            // pass once it tries.
            let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            vbox.prepend(&menubar);
            window.set_child(Some(&vbox));
            state.vbox = Some(vbox);
            state.menubar = Some(menubar.upcast::<gtk4::Widget>());
            return;
        };

        // Detach the existing child from the window so we can
        // re-parent it into the vbox.
        window.set_child(None::<&gtk4::Widget>);
        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        vbox.append(&menubar);
        // Allow the framework's content to grow / shrink with the
        // window. `vexpand` on the second child makes the vbox
        // give it whatever space the menubar doesn't claim.
        existing_child.set_vexpand(true);
        vbox.append(&existing_child);
        window.set_child(Some(&vbox));

        state.vbox = Some(vbox);
        state.menubar = Some(menubar.upcast::<gtk4::Widget>());
    });
}

struct InstallState {
    /// The vbox we installed as the window's child. Holds
    /// [menubar, framework_root] vertically. None until the first
    /// install runs.
    vbox: Option<gtk4::Box>,
    /// The currently-installed menubar widget (first child of the
    /// vbox). Tracked separately so re-install can swap it.
    menubar: Option<gtk4::Widget>,
}

thread_local! {
    static LAST_MENUBAR: RefCell<InstallState> = RefCell::new(InstallState {
        vbox: None,
        menubar: None,
    });
}

// =========================================================================
// GMenu construction
// =========================================================================

fn build_submenu(
    items: &[MenuItem],
    actions: &gio::SimpleActionGroup,
    idx: &mut u32,
) -> gio::Menu {
    let menu = gio::Menu::new();

    // Group runs of non-separator items into a `gio::MenuItem` list,
    // then commit each group as a section so the separators GTK
    // draws come from section boundaries (the GMenu-idiomatic
    // approach, vs. inserting fake separator items).
    let mut section = gio::Menu::new();

    for item in items {
        match item {
            MenuItem::Command(cmd) => {
                let action_name = format!("cmd{}", *idx);
                *idx += 1;
                install_action(actions, &action_name, cmd);

                let menu_item = gio::MenuItem::new(Some(&cmd.label), Some(&format!("win.{action_name}")));
                if let Some(s) = &cmd.shortcut {
                    // GMenu's accel attribute renders the
                    // accelerator hint at the right side of the
                    // menu item. Dispatch still requires the host
                    // app to call `set_accels_for_action`.
                    menu_item.set_attribute_value(
                        "accel",
                        Some(&glib::Variant::from(gtk_accel_string(s))),
                    );
                }
                section.append_item(&menu_item);
            }
            MenuItem::Separator => {
                // Commit the section accumulated so far, start a
                // fresh one. GTK draws a horizontal rule between
                // sections automatically.
                if section.n_items() > 0 {
                    menu.append_section(None, &section);
                    section = gio::Menu::new();
                }
            }
            MenuItem::Submenu(sub) => {
                let nested = build_submenu(&sub.items, actions, idx);
                section.append_submenu(Some(&sub.title), &nested);
            }
        }
    }

    // Trailing section (if any).
    if section.n_items() > 0 {
        menu.append_section(None, &section);
    }

    menu
}

fn install_action(
    actions: &gio::SimpleActionGroup,
    name: &str,
    cmd: &MenuCommand,
) {
    let action = gio::SimpleAction::new(name, None);
    action.set_enabled(cmd.enabled);
    if let Some(cb) = cmd.on_click.clone() {
        action.connect_activate(move |_, _| cb());
    }
    actions.add_action(&action);
}

/// Format a `Shortcut` as the accelerator string GTK expects, e.g.
/// `"<Primary>s"`, `"<Primary><Shift>z"`, `"<Alt>F4"`. `<Primary>` is
/// GTK's portable name for Ctrl-on-Linux / Cmd-on-macOS — our
/// `Modifiers::COMMAND` maps to it cleanly.
fn gtk_accel_string(s: &Shortcut) -> String {
    let mut out = String::new();
    if s.modifiers.contains(Modifiers::CONTROL) || s.modifiers.contains(Modifiers::COMMAND) {
        out.push_str("<Primary>");
    }
    if s.modifiers.contains(Modifiers::SHIFT) {
        out.push_str("<Shift>");
    }
    if s.modifiers.contains(Modifiers::OPTION) {
        // macOS Option → Linux Alt. Same physical key on most
        // cross-platform keyboards.
        out.push_str("<Alt>");
    }
    out.push(s.key.to_ascii_lowercase());
    out
}

// Re-export `Menu` so the cfg-gated build picks it up — `clippy`
// flags unused imports otherwise on the `Menu` type which only
// appears in the public API surface, not in this module's body.
#[allow(dead_code)]
fn _menu_marker() -> Menu {
    Menu::new("")
}
