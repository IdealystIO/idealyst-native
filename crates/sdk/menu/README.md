# `menu`

Installs the OS-level **system menu bar** — `NSApplication.mainMenu` on
macOS (the bar at the top of the screen), `HMENU` via `SetMenu` on
Windows, `GtkPopoverMenuBar` on GTK. On platforms with no menu-bar
concept (iOS, Android, web, terminal, wgpu, ESP, CPU), [`install`] is a
silent no-op.

Unlike most SDKs in this directory, `menu` is **not** an
`Element::External` primitive — it's a plain Rust capability API. The
system menu bar is process-level chrome: there's exactly one, it lives
outside every window's view tree, and the OS treats it as application
state set once at boot. It has no in-tree size/layout/parent, so a
primitive would be the wrong model.

```rust
host_appkit::run_with(
    app,
    host_appkit::RunOptions::default(),
    |backend| {
        menu::install(backend, menu::MenuBarSpec {
            menus: vec![
                menu::Menu::new("File").items(vec![
                    menu::MenuItem::command("New")
                        .shortcut(menu::Shortcut::cmd('n'))
                        .on_click(|| log::info!("new!")),
                    menu::MenuItem::separator(),
                    menu::MenuItem::command("Quit")
                        .shortcut(menu::Shortcut::cmd('q'))
                        .on_click(|| std::process::exit(0)),
                ]),
                menu::Menu::new("Edit").items(vec![
                    menu::MenuItem::command("Undo")
                        .shortcut(menu::Shortcut::cmd('z'))
                        .on_click(|| log::info!("undo")),
                ]),
            ],
        });
    },
)?;
```

## Per-platform behavior

| Target | Mechanism |
| --- | --- |
| macOS | `NSApplication.mainMenu`. Shortcuts map to `NSMenuItem.keyEquivalent` + `keyEquivalentModifierMask`. [`install_reactive`] is fully reactive — it wraps the spec closure in an `Effect` and re-installs the bar when a read signal changes (requires `backend_macos::install_global_self`, which `host_appkit::run_with` does for you). |
| Windows | `HMENU` via `SetMenu(hwnd, hmenu)`. [`install_reactive`] is **best-effort**: calls the closure once and installs the result; true re-install-on-change needs a global-backend hook (follow-up). |
| Linux (GTK) | `GtkPopoverMenuBar`; accelerators via `gtk_application_set_accels_for_action`. [`install_reactive`] is best-effort (same as Windows). |
| iOS | UIKit menu builder. `install` applies once and forces a UIKit rebuild; [`install_reactive`] reads current signal values at that moment but does not re-run on later changes (follow-up). |
| Android / web / terminal / wgpu / ESP / CPU | No-op — no menu bar exists. |

## Menu structure

- [`MenuBarSpec`] — the top-level `Vec<Menu>`. On macOS the **first**
  menu is conventionally the application menu (About / Preferences /
  Quit); most apps pass `Menu::new("")` first so macOS substitutes the
  process name.
- [`Menu`] — one dropdown. [`Menu::new`] + [`Menu::items`].
- [`MenuItem`] — a [`Command`](MenuItem::Command), a
  [`Separator`](MenuItem::Separator), or a nested
  [`Submenu`](MenuItem::Submenu). Build commands with
  [`MenuItem::command`] → [`MenuCommand`], chaining
  [`.shortcut(...)`](MenuCommand::shortcut),
  [`.on_click(...)`](MenuCommand::on_click),
  [`.enabled(...)`](MenuCommand::enabled).

## Shortcuts

[`Shortcut`] pairs a key char with a [`Modifiers`] bitmask. The
constructors keep author code cross-platform: [`Shortcut::cmd`] is ⌘ on
macOS and `Ctrl` on Windows/Linux (the platform's primary modifier), so
the same declaration ports without forking. [`Shortcut::ctrl`] is the
literal Control key everywhere. Compose extra modifiers with
[`Shortcut::with`] or the `|` operator on [`Modifiers`].

[`install`]: src/lib.rs
[`install_reactive`]: src/lib.rs
[`MenuBarSpec`]: src/lib.rs
[`Menu`]: src/lib.rs
[`Menu::new`]: src/lib.rs
[`Menu::items`]: src/lib.rs
[`MenuItem`]: src/lib.rs
[`MenuItem::command`]: src/lib.rs
[`MenuCommand`]: src/lib.rs
[`MenuCommand::shortcut`]: src/lib.rs
[`MenuCommand::on_click`]: src/lib.rs
[`MenuCommand::enabled`]: src/lib.rs
[`Shortcut`]: src/lib.rs
[`Shortcut::cmd`]: src/lib.rs
[`Shortcut::ctrl`]: src/lib.rs
[`Shortcut::with`]: src/lib.rs
[`Modifiers`]: src/lib.rs
