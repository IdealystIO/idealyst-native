//! OS-level menu-bar SDK for the idealyst framework.
//!
//! Installs the system menu bar — **`NSApplication.mainMenu`** on
//! macOS (the bar at the top of the screen, next to the Apple logo);
//! `HMENU` via `SetMenu(hwnd, hmenu)` on Windows; `GtkPopoverMenuBar`
//! on GTK. On platforms with no menu-bar concept (iOS, Android, web,
//! terminal, wgpu, ESP, CPU), [`install`] is a silent no-op.
//!
//! # Why not a `Element::External`?
//!
//! The system menu bar is a process-level chrome surface — there is
//! exactly one, it lives outside every window's view tree, and macOS
//! and Windows both treat it as application state set once at boot.
//! `Element::External` is the right fit for content that has an
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
//! [`install`] is a one-shot call — call again with a new spec to
//! swap the bar. For specs that depend on signals (Save enabled only
//! when dirty, recent-files submenu populated from a `Signal<Vec>`,
//! checkmarks on view modes), use [`install_reactive`] which takes a
//! closure and re-fires whenever any signal it reads changes. Same
//! shape as the toolbar SDK's reactive `items` closure.

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
    /// The bar's top-level menus, left-to-right. The first entry is
    /// conventionally the application menu on macOS (see the struct
    /// docs).
    pub menus: Vec<Menu>,
}

/// A single dropdown menu in the bar. Title appears in the menu bar
/// (or as a submenu label, when nested). `items` is the dropdown
/// contents.
#[derive(Clone)]
pub struct Menu {
    /// The menu's label — shown in the bar (top-level) or as the
    /// submenu's row text (when nested).
    pub title: String,
    /// The dropdown contents, top-to-bottom.
    pub items: Vec<MenuItem>,
}

impl Menu {
    /// Create an empty menu with the given title. Add rows with
    /// [`items`](Self::items) or by pushing onto [`Menu::items`].
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
    /// A clickable command with an optional shortcut + handler.
    /// Construct via [`MenuItem::command`].
    Command(MenuCommand),
    /// A horizontal divider between command groups.
    Separator,
    /// A nested submenu (the [`Menu`] becomes a fly-out).
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

    /// A divider [`MenuItem`] between command groups.
    pub fn separator() -> Self {
        Self::Separator
    }

    /// Wrap a [`Menu`] as a nested submenu [`MenuItem`].
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
    /// The row's visible text.
    pub label: String,
    /// Handler invoked on the main thread when the command is chosen.
    /// `None` leaves the row inert (but still shown).
    pub on_click: Option<Rc<dyn Fn()>>,
    /// Optional keyboard shortcut shown alongside the label and wired
    /// to the OS's accelerator machinery.
    pub shortcut: Option<Shortcut>,
    /// Whether the command is selectable. `false` greys it out.
    pub enabled: bool,
}

impl MenuCommand {
    /// Set the click handler. Fires on the main thread when the command
    /// is chosen.
    pub fn on_click<F: Fn() + 'static>(mut self, f: F) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// Attach a keyboard [`Shortcut`].
    pub fn shortcut(mut self, s: Shortcut) -> Self {
        self.shortcut = Some(s);
        self
    }

    /// Enable or disable the command (`false` greys it out).
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
    /// The literal key character (e.g. `'n'`, `'s'`). Combined with
    /// [`modifiers`](Self::modifiers) to form the accelerator.
    pub key: char,
    /// The modifier set held with [`key`](Self::key).
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

    /// `Shift+Cmd+<key>` (`Shift+Ctrl+<key>` on Windows/Linux).
    pub fn shift_cmd(key: char) -> Self {
        Self {
            key,
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
        }
    }

    /// `Opt+Cmd+<key>` (`Alt+Ctrl+<key>` on Windows/Linux).
    pub fn opt_cmd(key: char) -> Self {
        Self {
            key,
            modifiers: Modifiers::COMMAND | Modifiers::OPTION,
        }
    }

    /// `Ctrl+<key>` — the literal Ctrl key on every platform (distinct
    /// from [`cmd`](Self::cmd), which maps to ⌘ on macOS).
    pub fn ctrl(key: char) -> Self {
        Self {
            key,
            modifiers: Modifiers::CONTROL,
        }
    }

    /// Add extra modifiers to an existing shortcut (OR-combines).
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
pub struct Modifiers(
    /// The raw bitmask. Prefer the named constants
    /// ([`COMMAND`](Self::COMMAND), …) and `|` over constructing this
    /// directly.
    pub u32,
);

impl Modifiers {
    /// No modifiers.
    pub const NONE: Self = Self(0);
    /// The primary command modifier — ⌘ on macOS, `Ctrl` on
    /// Windows/Linux.
    pub const COMMAND: Self = Self(1 << 0);
    /// The Shift key.
    pub const SHIFT: Self = Self(1 << 1);
    /// The Option / Alt key.
    pub const OPTION: Self = Self(1 << 2);
    /// The literal Control key (distinct from [`COMMAND`](Self::COMMAND)
    /// on macOS).
    pub const CONTROL: Self = Self(1 << 3);

    /// `true` if every bit in `other` is set in `self`.
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
pub use macos::{install, install_reactive};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::install;

#[cfg(target_os = "windows")]
/// On Windows, `install_reactive` is best-effort: it calls `spec_fn`
/// once and installs the result. True reactivity (re-install on
/// signal change) needs a global-backend hook on `WindowsBackend`
/// — tracked as a follow-up. The one-shot behavior matches what
/// `install(backend, spec_fn())` would do.
pub fn install_reactive<F>(backend: &mut backend_windows::WindowsBackend, spec_fn: F)
where
    F: Fn() -> MenuBarSpec + 'static,
{
    install(backend, spec_fn());
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::install;

#[cfg(target_os = "linux")]
/// On Linux, `install_reactive` is best-effort: it calls `spec_fn`
/// once and installs the result. True reactivity (re-install on
/// signal change) needs a global-backend hook on `LinuxBackend` —
/// tracked as a follow-up.
pub fn install_reactive<F>(backend: &mut backend_linux::LinuxBackend, spec_fn: F)
where
    F: Fn() -> MenuBarSpec + 'static,
{
    install(backend, spec_fn());
}

#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "ios")]
pub use ios::{apply_to_builder, force_rebuild, idealyst_menu_apply_to_builder, install};

#[cfg(target_os = "ios")]
/// On iOS, `install_reactive` is best-effort: it calls `spec_fn`
/// once and installs the result, then forces UIKit to rebuild. The
/// rebuild reads the stored spec via `apply_to_builder`, so any
/// signals the closure read will reflect their **current** values
/// at that moment — but the closure isn't re-run on subsequent
/// signal changes. True reactivity needs `Effect::new` + a
/// re-trigger of `force_rebuild` on signal change, which is a
/// follow-up.
pub fn install_reactive<F>(backend: &mut backend_ios::IosBackend, spec_fn: F)
where
    F: Fn() -> MenuBarSpec + 'static,
{
    install(backend, spec_fn());
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "ios",
)))]
mod fallback {
    use super::MenuBarSpec;
    use runtime_core::Backend;

    /// No-op `install` for targets with no menu-bar concept. User
    /// code calls this unconditionally; on iOS / Android / web /
    /// etc. it's the right thing (no menu bar exists).
    pub fn install<B: Backend>(_backend: &mut B, _spec: MenuBarSpec) {}

    /// No-op reactive `install` for the same targets. Drops the
    /// closure without calling it; user code that mounts shortcuts
    /// inside the closure gets nothing on these platforms.
    pub fn install_reactive<B: Backend, F: Fn() -> MenuBarSpec + 'static>(
        _backend: &mut B,
        _spec_fn: F,
    ) {
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "ios",
)))]
pub use fallback::{install, install_reactive};

// ============================================================================
// Prelude
// ============================================================================

/// `use menu::prelude::*;` brings the common types into scope.
pub mod prelude {
    pub use super::{
        install, install_reactive, Menu, MenuBarSpec, MenuCommand, MenuItem, Modifiers,
        Shortcut,
    };
}
