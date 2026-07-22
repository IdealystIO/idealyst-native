//! Compile-checked usage **recipes** for the theme runtime — the
//! canonical "how do I wire light/dark mode" example the MCP catalog
//! serves.
//!
//! Why this exists: the arena's themed-settings feedback report
//! (run-0, 2026-07) showed an agent hand-rolling a palette struct with
//! per-node `if dark {...}` color switching because `list_recipes`
//! offered no theming recipe and `install_theme` wasn't discoverable
//! from `runtime-core` alone. This recipe compiles against the live
//! `install_theme` / `set_theme` / `stylesheet!` APIs, so the served
//! example can't rot.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every
//! production build) it expands to nothing — no `#[cfg]` needed here
//! and no cost in shipped apps. Recipes are self-contained (imports
//! live inside the fn) so the captured source reads as a complete,
//! copy-pasteable example.

use runtime_core::recipe;

recipe!(
    install_theme,
    /// Light/dark mode, end to end: ONE theme struct implementing
    /// `ThemeTokens`, two consts (`LIGHT` / `DARK`), a `stylesheet!`
    /// whose colors reference the theme's tokens BY NAME, and a
    /// `signal(false)`-driven toggle that swaps the whole app with a
    /// single `set_theme` call. Every styled primitive referencing a
    /// token re-resolves automatically on swap — no per-node `if dark`
    /// branching, no hand-rolled palette plumbing. `install_theme`
    /// runs once at startup, before the first render. (For a fully
    /// signal-driven variant selection see `install_themes`, which
    /// watches a `Signal<String>`; idea-ui apps use
    /// `install_idea_theme` / `install_idea_theme_reactive` instead.)
    pub fn dark_mode_toggle() -> ::runtime_core::Element {
        use crate::{install_theme, set_theme, ThemeTokens, TokenEntry, TokenValue};
        use ::runtime_core::{signal, stylesheet, ui, Color, Tokenized};

        // The theme is one plain struct — fields are whatever the
        // design needs. `tokens()` maps them to named token values.
        #[derive(Clone, Copy)]
        struct AppTheme {
            surface: &'static str,
            text: &'static str,
        }
        impl ThemeTokens for AppTheme {
            fn tokens(&self) -> Vec<TokenEntry> {
                vec![
                    TokenEntry { name: "app-surface", value: TokenValue::Color(self.surface.into()) },
                    TokenEntry { name: "app-text", value: TokenValue::Color(self.text.into()) },
                ]
            }
        }
        const LIGHT: AppTheme = AppTheme { surface: "#ffffff", text: "#111827" };
        const DARK: AppTheme = AppTheme { surface: "#0b1220", text: "#e5e7eb" };

        // Styles consume tokens BY NAME; the fallback literal only
        // applies when no token with that name is installed.
        stylesheet! {
            Panel<()> {
                base(_t) {
                    background: Tokenized::token("app-surface", Color("#ffffff".into())),
                    color: Tokenized::token("app-text", Color("#111827".into())),
                    padding: 16,
                }
            }
        }

        // Install once, BEFORE the first render.
        install_theme(LIGHT);

        let dark = signal(false);
        ui! {
            view(style = Panel()) {
                text { "Appearance" }
                toggle(
                    value = dark,
                    on_change = move |v| {
                        dark.set(v);
                        // One call re-themes every styled node.
                        set_theme(if v { DARK } else { LIGHT });
                    },
                )
            }
        }
    }
);
