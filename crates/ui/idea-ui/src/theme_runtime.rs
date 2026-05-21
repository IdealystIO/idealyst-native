//! Theme-as-struct runtime — the previous `framework-theme` crate,
//! folded into idea-ui.
//!
//! `framework-core` cares about **tokens** (named values, plus
//! `Tokenized<T>` references in style rules). It deliberately does
//! not care how the author organizes those tokens. This module
//! provides the optional "theme is a struct that implements
//! [`ThemeTokens`]" pattern that lets author code keep a typed
//! `active_theme()` stash, swap themes at runtime, and drive multi-
//! variant theme selection from a `Signal<String>`.
//!
//! Lives in idea-ui because the concept of "a theme" is a user-side
//! convention — it's idiomatic for app code but not a framework
//! contract. idea-ui's own typed `IdeaTheme` API (`light_theme()`
//! etc.) builds directly on the surface defined here.
//!
//! ```no_run
//! use idea_ui::{install_theme, set_theme, ThemeTokens, TokenEntry, TokenValue};
//!
//! struct MyTheme { accent: framework_core::Color }
//! impl ThemeTokens for MyTheme {
//!     fn tokens(&self) -> Vec<TokenEntry> {
//!         vec![TokenEntry {
//!             name: "accent",
//!             value: TokenValue::Color(self.accent.clone()),
//!         }]
//!     }
//! }
//!
//! install_theme(MyTheme { accent: "#06f".into() });
//! set_theme(MyTheme { accent: "#39f".into() });
//! ```

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use framework_core::{install_tokens, update_tokens, Effect, Signal};

pub use framework_core::{TokenEntry, TokenValue, Tokenized};

/// A theme that exposes its tokens by name and concrete value.
///
/// Implement on your theme struct (whatever shape it has); `tokens()`
/// returns the `(name, value)` pairs that should be installed as
/// runtime variables. The names should match the `name` fields of
/// the `Tokenized::Token { name, .. }` variants the stylesheets
/// construct.
pub trait ThemeTokens: Any {
    fn tokens(&self) -> Vec<TokenEntry>;
}

thread_local! {
    /// The active theme. Wrapped in a `Signal<Rc<dyn Any>>` so
    /// effects subscribe via the existing reactivity system and
    /// re-apply on swap. Only callers who use the theme-as-struct
    /// pattern read this; nothing in framework-core or backends
    /// touches it.
    static ACTIVE_THEME: RefCell<Option<Signal<Rc<dyn Any>>>> = const { RefCell::new(None) };

    /// Keepalive for [`install_themes`]'s internal Effect when
    /// called outside any render scope (e.g. tests, top-level
    /// binaries). In production this is unused — `install_themes`
    /// runs inside the user's `app()` which holds an active scope
    /// and the scope owns the slot.
    static INSTALL_THEMES_KEEPALIVE: RefCell<Vec<Effect>> = const { RefCell::new(Vec::new()) };
}

/// Install the initial active theme. Call once at app startup
/// before rendering. Stashes the theme as `Rc<dyn Any>` in this
/// module's signal and forwards its `tokens()` to
/// [`framework_core::install_tokens`].
pub fn install_theme<T: ThemeTokens + 'static>(theme: T) {
    let tokens = theme.tokens();
    let rc: Rc<dyn Any> = Rc::new(theme);
    let sig = Signal::new(rc);
    ACTIVE_THEME.with(|t| *t.borrow_mut() = Some(sig));
    install_tokens(&tokens);
}

/// Swap the active theme. Forwards the new tokens to
/// [`framework_core::update_tokens`] (which wipes the framework's
/// resolution cache, re-fires every styled effect via the tokens
/// version signal, and pushes deltas to the backend) and re-fires
/// this module's [`active_theme`] signal so author code reading the
/// theme struct directly also re-runs.
pub fn set_theme<T: ThemeTokens + 'static>(theme: T) {
    let tokens = theme.tokens();
    let rc: Rc<dyn Any> = Rc::new(theme);

    ACTIVE_THEME.with(|t| {
        if let Some(sig) = t.borrow().as_ref() {
            sig.set(rc);
        } else {
            // First write — create the signal lazily.
            let new_sig = Signal::new(rc);
            *t.borrow_mut() = Some(new_sig);
        }
    });

    update_tokens(&tokens);
}

/// Install a multi-variant theme system with the active variant
/// driven by a `Signal<String>`. The signal's current value names
/// the initial active theme; an internal Effect watches the signal
/// and calls [`set_theme`] whenever the name changes.
///
/// Variants must include an entry whose name matches the signal's
/// initial value; that variant becomes the initially-active theme.
/// Missing the match panics at install time so misconfiguration
/// surfaces before any rendering.
pub fn install_themes<T: ThemeTokens + Clone + 'static>(
    active: Signal<String>,
    variants: &[(&'static str, T)],
) {
    let initial_name = active.get();
    let initial_theme = variants
        .iter()
        .find(|(name, _)| *name == initial_name.as_str())
        .map(|(_, theme)| theme.clone())
        .unwrap_or_else(|| {
            panic!(
                "install_themes: active signal initial value '{}' has no matching variant; \
                 variants registered: {:?}",
                initial_name,
                variants.iter().map(|(n, _)| *n).collect::<Vec<_>>()
            )
        });
    install_theme(initial_theme);

    let variants_owned: HashMap<String, T> = variants
        .iter()
        .map(|(name, theme)| (name.to_string(), theme.clone()))
        .collect();
    let last_seen: Rc<RefCell<String>> = Rc::new(RefCell::new(initial_name));
    let effect = Effect::new(move || {
        let name = active.get();
        if last_seen.borrow().as_str() == name.as_str() {
            return;
        }
        if let Some(theme) = variants_owned.get(&name) {
            set_theme(theme.clone());
            *last_seen.borrow_mut() = name;
        }
    });
    // If a render scope took ownership of the slot, `effect`'s drop
    // is a no-op and this push just keeps an empty-handle in a vec.
    // Outside a scope (tests, top-level binaries), this is what
    // keeps the effect alive past the function return.
    INSTALL_THEMES_KEEPALIVE.with(|k| k.borrow_mut().push(effect));
}

/// Read the active theme. Subscribes the current effect (if any) to
/// theme changes — that's how reactive style application works for
/// callers that read theme struct fields directly (as opposed to
/// via tokenized stylesheet references).
///
/// Panics if no theme has been installed. Call [`install_theme`]
/// before render.
pub fn active_theme() -> Rc<dyn Any> {
    ACTIVE_THEME.with(|t| {
        t.borrow()
            .as_ref()
            .expect("no theme installed; call idea_ui::install_theme(...) before rendering")
            .get()
    })
}
