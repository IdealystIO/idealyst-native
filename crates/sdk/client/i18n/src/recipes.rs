//! Compile-checked usage **recipes** for the i18n SDK — the canonical
//! "declare messages, then switch language" example the MCP catalog serves.
//!
//! Why this exists: i18n's whole value is that translations are authored
//! inline and checked at compile time, and that switching locale re-renders
//! affected text in place with no manual refresh. An agent that doesn't see
//! the loop wires up a hand-rolled string map + a manual re-render. This
//! recipe compiles the real `i18n!` macro against the live generated API
//! (`Locale`, `set_locale`, `current_locale`, one `Reactive<String>` fn per
//! message), so the served example can't rot.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every
//! production build) it expands to nothing. Recipes are self-contained —
//! the message catalog and imports live inside the fn — so the captured
//! source reads as a complete, copy-pasteable example.

use runtime_core::recipe;

recipe!(
    i18n,
    /// A two-locale message catalog plus a button that switches language at
    /// runtime. The `i18n! { … }` macro generates a typed `Locale` enum and
    /// one function per message returning `Reactive<String>`; pass those
    /// straight to any reactive-text prop (`text(content = …)`,
    /// `button(label = …)`) and they re-render IN PLACE when the locale
    /// changes — no manual refresh, same code on every backend.
    ///
    /// Exactly one locale is marked `(default)` (the reference + fallback). A
    /// bundled locale missing a message, or a `{placeholder}` that doesn't
    /// match a declared argument, is a COMPILE error. `set_locale(Locale::Fr)`
    /// flips the global locale signal; `current_locale()` reads it back — here
    /// the button toggles between the two bundled locales.
    pub fn locale_switch() -> ::runtime_core::Element {
        use ::runtime_core::ui;

        // Conventionally the catalog lives in its own `t` module so call
        // sites read `t::greeting(...)`.
        mod t {
            i18n::i18n! {
                // Exactly one locale must be `(default)`.
                locales: { En = "en" (default), Fr = "fr" }

                // A message with a typed interpolation argument.
                greeting(name) {
                    En: "Hello, {name}",
                    Fr: "Bonjour, {name}",
                }

                // A no-argument message — the label of the switch button,
                // itself translated so it flips with the locale.
                switch_label {
                    En: "Français",
                    Fr: "English",
                }
            }
        }

        ui! {
            view {
                // `greeting` returns a `Reactive<String>` — the text updates
                // itself when the locale changes.
                text(content = t::greeting("Ada"))
                // Toggle between the two bundled locales. The label is itself
                // a translated message, so it swaps in place too.
                button(
                    label = t::switch_label(),
                    on_click = || {
                        let next = match t::current_locale() {
                            t::Locale::En => t::Locale::Fr,
                            t::Locale::Fr => t::Locale::En,
                        };
                        t::set_locale(next);
                    },
                )
            }
        }
    }
);
