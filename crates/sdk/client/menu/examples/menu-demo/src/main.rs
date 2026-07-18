//! Smoke-test binary for the `menu` SDK.
//!
//! Boots a host-appkit window with a system menu bar attached to
//! `NSApplication.mainMenu` — File / Edit / View, with separators,
//! submenus, and keyboard shortcuts. Click items or use shortcuts to
//! see the per-item callbacks fire (logged to stderr).
//!
//! Run with:
//!
//! ```sh
//! cargo run -p menu-demo
//! ```

#[cfg(target_os = "macos")]
fn main() {
    use menu::{Menu, MenuBarSpec, MenuItem, Shortcut};
    use runtime_core::{install_tokens, view, Element};

    let app = || -> Element {
        install_tokens(&[]);
        // Empty view — the menu bar lives outside the view tree
        // entirely. Window body is just blank.
        view(vec![]).into()
    };

    let opts = host_appkit::RunOptions {
        title: "Menu SDK Smoke Test".to_string(),
        width: 600.0,
        height: 300.0,
    };

    if let Err(e) = host_appkit::run_with(app, opts, |backend| {
        // The first menu's title is the app menu by convention.
        // Leaving it empty here lets the system display whatever it
        // chooses (typically the process name); a real app would
        // populate it with About / Preferences / Quit.
        let bar = MenuBarSpec {
            menus: vec![
                Menu::new("App").items(vec![
                    MenuItem::command("About menu-demo")
                        .on_click(|| eprintln!("[menu-demo] About clicked"))
                        .into(),
                    MenuItem::separator(),
                    MenuItem::command("Quit")
                        .shortcut(Shortcut::cmd('q'))
                        .on_click(|| {
                            eprintln!("[menu-demo] Quit clicked");
                            std::process::exit(0);
                        })
                        .into(),
                ]),
                Menu::new("File").items(vec![
                    MenuItem::command("New")
                        .shortcut(Shortcut::cmd('n'))
                        .on_click(|| eprintln!("[menu-demo] File → New"))
                        .into(),
                    MenuItem::command("Open…")
                        .shortcut(Shortcut::cmd('o'))
                        .on_click(|| eprintln!("[menu-demo] File → Open…"))
                        .into(),
                    MenuItem::separator(),
                    MenuItem::command("Close Window")
                        .shortcut(Shortcut::cmd('w'))
                        .on_click(|| eprintln!("[menu-demo] File → Close"))
                        .into(),
                ]),
                Menu::new("Edit").items(vec![
                    MenuItem::command("Undo")
                        .shortcut(Shortcut::cmd('z'))
                        .on_click(|| eprintln!("[menu-demo] Edit → Undo"))
                        .into(),
                    MenuItem::command("Redo")
                        .shortcut(Shortcut::shift_cmd('z'))
                        .on_click(|| eprintln!("[menu-demo] Edit → Redo"))
                        .into(),
                    MenuItem::separator(),
                    MenuItem::submenu(Menu::new("Find").items(vec![
                        MenuItem::command("Find…")
                            .shortcut(Shortcut::cmd('f'))
                            .on_click(|| eprintln!("[menu-demo] Find → Find…"))
                            .into(),
                        MenuItem::command("Find Next")
                            .shortcut(Shortcut::cmd('g'))
                            .on_click(|| eprintln!("[menu-demo] Find → Next"))
                            .into(),
                    ])),
                ]),
                Menu::new("View").items(vec![
                    MenuItem::command("Zoom In")
                        .shortcut(Shortcut::cmd('='))
                        .on_click(|| eprintln!("[menu-demo] View → Zoom In"))
                        .into(),
                    MenuItem::command("Zoom Out")
                        .shortcut(Shortcut::cmd('-'))
                        .on_click(|| eprintln!("[menu-demo] View → Zoom Out"))
                        .into(),
                ]),
                // Unmistakably-custom menu so you can visually confirm
                // the bar is the one this SDK installed (not just the
                // system default). Labels are intentionally weird —
                // no real app would ship "Wave at the Universe" or
                // "Print Squirrels to Stderr".
                Menu::new("🦀 Idealyst").items(vec![
                    MenuItem::command("Wave at the Universe 👋")
                        .shortcut(Shortcut::shift_cmd('w'))
                        .on_click(|| eprintln!("[menu-demo] 👋 hello, universe"))
                        .into(),
                    MenuItem::command("Print Squirrels to Stderr 🐿️")
                        .shortcut(Shortcut::opt_cmd('s'))
                        .on_click(|| {
                            eprintln!("[menu-demo] 🐿️🐿️🐿️🐿️🐿️");
                            eprintln!("[menu-demo]  squirrels delivered");
                        })
                        .into(),
                    MenuItem::separator(),
                    MenuItem::command("This Item Is Disabled")
                        .enabled(false)
                        .on_click(|| eprintln!("[menu-demo] should never fire"))
                        .into(),
                    MenuItem::separator(),
                    MenuItem::submenu(Menu::new("Nested Weirdness").items(vec![
                        MenuItem::command("Level 2 → Beep")
                            .on_click(|| {
                                // ASCII BEL — terminal bell, hard to
                                // miss when fired from a menu click.
                                eprintln!("[menu-demo] \x07 beep");
                            })
                            .into(),
                        MenuItem::submenu(Menu::new("Even Deeper").items(vec![
                            MenuItem::command("Level 3 → Boop")
                                .on_click(|| eprintln!("[menu-demo] boop"))
                                .into(),
                        ])),
                    ])),
                    MenuItem::separator(),
                    MenuItem::command("Show ⌃⌥⌘K Combo Shortcut")
                        .shortcut(
                            Shortcut::cmd('k')
                                .with(menu::Modifiers::CONTROL)
                                .with(menu::Modifiers::OPTION),
                        )
                        .on_click(|| eprintln!("[menu-demo] ⌃⌥⌘K — four-modifier combo"))
                        .into(),
                ]),
            ],
        };
        menu::install(backend, bar);
        eprintln!("[menu-demo] menu bar installed");
    }) {
        eprintln!("menu-demo: runtime error: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!(
        "menu-demo: this smoke test only runs on macOS. The menu \
         SDK's `install` is a no-op on other platforms."
    );
}
