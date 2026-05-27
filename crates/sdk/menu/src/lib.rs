//! OS-level menu-bar SDK for the idealyst framework.
//!
//! Installs the system menu bar — **`NSApplication.mainMenu`** on
//! macOS (the bar at the top of the screen, next to the Apple logo);
//! `HMENU` via `SetMenu(hwnd, hmenu)` on Windows; `GtkPopoverMenuBar`
//! on GTK. On platforms with no menu-bar concept (iOS, Android, web,
//! terminal, wgpu, ESP, CPU), [`install`] is a silent no-op.
//!
//! # Why not a `Primitive::External`?
//!
//! The system menu bar is a process-level chrome surface — there is
//! exactly one, it lives outside every window's view tree, and macOS
//! and Windows both treat it as application state set once at boot.
//! `Primitive::External` is the right fit for content that has an
//! in-tree position (size, layout, parent); the menu bar has none of
//! those. Modeling it as a primitive would force authors to mount it
//! "somewhere" arbitrary in the tree (where exactly? the navigator?
//! the root?), and the lifetime of that "somewhere" doesn't match
//! the lifetime of the menu bar.
//!
//! Instead, the API is a direct call against the backend:
//!
//! ```ignore
//! host_appkit::run_with(
//!     app,
//!     host_appkit::RunOptions::default(),
//!     |backend| {
//!         menu::install(backend, menu::MenuBarSpec {
//!             menus: vec![
//!                 menu::Menu::new("File").items(vec![
//!                     menu::MenuItem::command("New")
//!                         .shortcut(menu::Shortcut::cmd('n'))
//!                         .on_click(|| log::info!("new!")),
//!                     menu::MenuItem::separator(),
//!                     menu::MenuItem::command("Quit")
//!                         .shortcut(menu::Shortcut::cmd('q'))
//!                         .on_click(|| std::process::exit(0)),
//!                 ]),
//!                 menu::Menu::new("Edit").items(vec![
//!                     menu::MenuItem::command("Undo")
//!                         .shortcut(menu::Shortcut::cmd('z'))
//!                         .on_click(|| log::info!("undo")),
//!                 ]),
//!             ],
//!         });
//!     },
//! )?;
//! ```
//!
//! # Reactive updates
//!
//! V1 is install-once. Authors who need dynamic menus (e.g. recently-
//! opened files) should call [`install`] again with the new spec —
//! the macOS impl detaches the previous main menu and replaces it.
//! A future revision can offer a reactive variant that subscribes to
//! signals via `Effect::new`, mirroring the toolbar SDK's reactive
//! `items` closure shape.

use std::rc::Rc;

// ============================================================================
// Public API surface
// ============================================================================

/// Top-level menu bar spec. A `Vec<Menu>` of dropdown menus shown
/// across the system menu bar (or the in-window menu bar on Windows /
/// Linux).
///
/// On macOS, convention is that the **first** menu in the list is the
/// "application menu" — the one named after the app, containing
/// About / Preferences / Hide / Quit. The macOS impl doesn't enforce
/// this — if the first menu's title is non-empty, the system displays
/// the supplied title; macOS reserves a hard-coded first slot only when
/// the title is empty (then it substitutes the app's process name).
/// Most apps want `Menu::new("")` as the first entry with the standard
/// app-menu items inside it.
pub struct MenuBarSpec {
    pub menus: Vec<Menu>,
}

/// A single dropdown menu in the bar. Title appears in the menu bar
/// (or as a submenu label, when nested). `items` is the dropdown
/// contents.
#[derive(Clone)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
}

impl Menu {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
        }
    }

    /// Builder-style setter — replaces the items list. Use when
    /// constructing inline; for assembling incrementally, mutate
    /// `Menu::items` directly.
    pub fn items(mut self, items: Vec<MenuItem>) -> Self {
        self.items = items;
        self
    }
}

/// One row of a dropdown menu. Either a command (the user's
/// callback), a horizontal separator, or a nested submenu.
#[derive(Clone)]
pub enum MenuItem {
    Command(MenuCommand),
    Separator,
    Submenu(Menu),
}

impl MenuItem {
    /// Builder for a clickable command. Chain `.shortcut(...)`,
    /// `.on_click(...)`, `.enabled(false)` to fill in details.
    pub fn command(label: impl Into<String>) -> MenuCommand {
        MenuCommand {
            label: label.into(),
            on_click: None,
            shortcut: None,
            enabled: true,
        }
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    pub fn submenu(menu: Menu) -> Self {
        Self::Submenu(menu)
    }
}

impl From<MenuCommand> for MenuItem {
    fn from(c: MenuCommand) -> Self {
        MenuItem::Command(c)
    }
}

/// Builder for a clickable command item. Constructed via
/// [`MenuItem::command`]; use `.into()` (auto via `From`) to lift into
/// a `MenuItem` for inclusion in a `Vec<MenuItem>`.
#[derive(Clone)]
pub struct MenuCommand {
    pub label: String,
    pub on_click: Option<Rc<dyn Fn()>>,
    pub shortcut: Option<Shortcut>,
    pub enabled: bool,
}

impl MenuCommand {
    pub fn on_click<F: Fn() + 'static>(mut self, f: F) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    pub fn shortcut(mut self, s: Shortcut) -> Self {
        self.shortcut = Some(s);
        self
    }

    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
}

/// Keyboard shortcut for a menu command. On macOS this maps to
/// `NSMenuItem.keyEquivalent` (the character) +
/// `NSMenuItem.keyEquivalentModifierMask` (the modifier bitmask).
/// On Windows the equivalent is a `WM_COMMAND` accelerator table; on
/// GTK it's `gtk_application_set_accels_for_action`. The Rust shape
/// abstracts over those backends.
#[derive(Clone, Copy, Debug)]
pub struct Shortcut {
    pub key: char,
    pub modifiers: Modifiers,
}

impl Shortcut {
    /// `Cmd+<key>` — the default shortcut shape on macOS. On Windows
    /// / Linux this maps to `Ctrl+<key>` (the platform-native primary
    /// modifier), so author code stays cross-platform readable.
    pub fn cmd(key: char) -> Self {
        Self {
            key,
            modifiers: Modifiers::COMMAND,
        }
    }

    pub fn shift_cmd(key: char) -> Self {
        Self {
            key,
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
        }
    }

    pub fn opt_cmd(key: char) -> Self {
        Self {
            key,
            modifiers: Modifiers::COMMAND | Modifiers::OPTION,
        }
    }

    pub fn ctrl(key: char) -> Self {
        Self {
            key,
            modifiers: Modifiers::CONTROL,
        }
    }

    pub fn with(mut self, m: Modifiers) -> Self {
        self.modifiers = self.modifiers | m;
        self
    }
}

/// Modifier bitflags. Hand-rolled (no `bitflags` crate dep) — the set
/// is tiny and the surface stable.
///
/// On macOS, `Command` maps to the ⌘ key; on Windows / Linux it maps
/// to `Ctrl` (the platform's primary modifier) so cross-platform
/// shortcut declarations port without per-backend forking.
/// `Control` is the literal Ctrl key on every platform; on macOS it
/// stacks on top of `Command` for less-common shortcuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modifiers(pub u32);

impl Modifiers {
    pub const NONE: Self = Self(0);
    pub const COMMAND: Self = Self(1 << 0);
    pub const SHIFT: Self = Self(1 << 1);
    pub const OPTION: Self = Self(1 << 2);
    pub const CONTROL: Self = Self(1 << 3);

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

// ============================================================================
// Backend selector
// ============================================================================

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::install;

#[cfg(not(target_os = "macos"))]
mod fallback {
    use super::MenuBarSpec;
    use runtime_core::Backend;

    /// No-op `install` for targets without a menu-bar backend yet.
    /// User code calls this unconditionally; on iOS / Android / web /
    /// etc. it's the right thing (no menu bar exists), on Windows /
    /// Linux it's a temporary no-op until those backends land.
    pub fn install<B: Backend>(_backend: &mut B, _spec: MenuBarSpec) {}
}

#[cfg(not(target_os = "macos"))]
pub use fallback::install;

// ============================================================================
// Prelude
// ============================================================================

/// `use menu::prelude::*;` brings the common types into scope.
pub mod prelude {
    pub use super::{
        install, Menu, MenuBarSpec, MenuCommand, MenuItem, Modifiers, Shortcut,
    };
}
