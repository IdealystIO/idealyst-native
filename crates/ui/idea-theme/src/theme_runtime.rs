//! Theme-as-struct runtime — the previous `framework-theme` crate,
//! folded into idea-ui.
//!
//! `runtime-core` cares about **tokens** (named values, plus
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
//! use idea_theme::{install_theme, set_theme, ThemeTokens, TokenEntry, TokenValue};
//!
//! struct MyTheme { accent: runtime_core::Color }
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

use runtime_core::{install_tokens, update_tokens, Color, Effect, Signal};

pub use runtime_core::{TokenEntry, TokenValue, Tokenized};

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
    /// pattern read this; nothing in runtime-core or backends
    /// touches it.
    static ACTIVE_THEME: RefCell<Option<Signal<Rc<dyn Any>>>> = const { RefCell::new(None) };

    /// Keepalive for [`install_themes`]'s internal Effect when
    /// called outside any render scope (e.g. tests, top-level
    /// binaries). In production this is unused — `install_themes`
    /// runs inside the user's `app()` which holds an active scope
    /// and the scope owns the slot.
    ///
    /// Single-slot: each [`install_themes`] call replaces the
    /// previous keepalive, dropping its `Effect`. That way a hot-
    /// reload or fixture teardown that re-installs the theme system
    /// doesn't leak one `Effect` per call (and, when outside a scope,
    /// doesn't leave a growing pile of `owns: true` handles for
    /// thread-teardown to trip over). Two concurrent active-theme
    /// signals never make sense — the new install supersedes the
    /// old one.
    static INSTALL_THEMES_KEEPALIVE: RefCell<Option<Effect>> = const { RefCell::new(None) };
}

/// Install the initial active theme. Call once at app startup
/// before rendering. Stashes the theme as `Rc<dyn Any>` in this
/// module's signal and forwards its `tokens()` to
/// [`runtime_core::install_tokens`].
pub fn install_theme<T: ThemeTokens + 'static>(theme: T) {
    let tokens = theme.tokens();
    let rc: Rc<dyn Any> = Rc::new(theme);
    let sig = Signal::new(rc);
    ACTIVE_THEME.with(|t| *t.borrow_mut() = Some(sig));
    install_tokens(&tokens);
    apply_host_surface_from_tokens(&tokens);
}

/// Swap the active theme. Forwards the new tokens to
/// [`runtime_core::update_tokens`] (which wipes the framework's
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
    apply_host_surface_from_tokens(&tokens);
}

/// Token name for the host-surface background — body/UIWindow/clear color.
/// Same name across every idea-theme variant so the web backend's
/// `var(--color-background)` reference auto-resolves on swap.
const HOST_BG_TOKEN: &str = "color-background";

/// Token name for the platform scrollbar thumb. Track is fixed
/// `transparent` (literal) so the underlying surface color shows through;
/// most apps don't want a separately-themed track. Tighter contrast on
/// hover comes from the web backend's `::-webkit-scrollbar-thumb:hover`
/// rule reading `--color-text-muted`, but that's web-only chrome.
const SCROLLBAR_THUMB_TOKEN: &str = "color-border-strong";

/// Look up the host-surface background + scrollbar thumb in `tokens`
/// (by well-known name) and route them through
/// [`runtime_core::set_app_background`] / [`runtime_core::set_scrollbar_theme`].
/// Called from both [`install_theme`] and [`set_theme`] so native
/// backends (which apply the resolved value directly and have no
/// `var(--…)` indirection) repaint on every swap. The web backend uses
/// the token NAME — its rule is `body { background: var(--color-background); }`
/// — and would actually be fine with a single install-time call, but
/// re-calling on swap is cheap (rule delete+reinsert at the same index)
/// and keeps the cross-backend code path uniform.
fn apply_host_surface_from_tokens(tokens: &[TokenEntry]) {
    if let Some(fallback) = lookup_color(tokens, HOST_BG_TOKEN) {
        runtime_core::set_app_background(Tokenized::Token {
            name: HOST_BG_TOKEN,
            fallback,
        });
    }
    if let Some(fallback) = lookup_color(tokens, SCROLLBAR_THUMB_TOKEN) {
        runtime_core::set_scrollbar_theme(
            Tokenized::Token { name: SCROLLBAR_THUMB_TOKEN, fallback },
            Tokenized::Literal(Color("transparent".into())),
        );
    }
}

fn lookup_color(tokens: &[TokenEntry], name: &str) -> Option<Color> {
    tokens.iter().find(|t| t.name == name).and_then(|t| match &t.value {
        TokenValue::Color(c) => Some(c.clone()),
        _ => None,
    })
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
    // is a no-op and the keepalive just stores an empty handle.
    // Outside a scope (tests, top-level binaries), this is what
    // keeps the effect alive past the function return.
    //
    // Single-slot replacement: dropping the previous `Option`'s
    // `Effect` here frees the prior install's slot (or no-ops if
    // a scope owned it), preventing unbounded growth across
    // repeated calls.
    INSTALL_THEMES_KEEPALIVE.with(|k| *k.borrow_mut() = Some(effect));
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

/// Read the active theme *without* subscribing the current effect to theme
/// changes.
///
/// Use this for install/type assertions inside reactive style closures that
/// discard the value (`let _ = active_theme_untracked()...`). The active
/// theme is a hot, rarely-written signal; subscribing to it from a
/// per-instance style closure that doesn't actually consume the value leaves
/// a dead subscriber behind on every mount/unmount cycle (pruned only on the
/// next, rare, `set_theme`). Components that genuinely react to theme changes
/// should still use [`active_theme`].
///
/// Panics if no theme has been installed, exactly like [`active_theme`].
pub fn active_theme_untracked() -> Rc<dyn Any> {
    runtime_core::untrack(active_theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::Signal;

    #[derive(Clone)]
    struct TestTheme {
        name: &'static str,
    }
    impl ThemeTokens for TestTheme {
        fn tokens(&self) -> Vec<TokenEntry> {
            // Numeric token avoids requiring a Color/Length parser dep in tests.
            let _ = self.name;
            vec![TokenEntry {
                name: "test.value",
                value: TokenValue::Number(1.0),
            }]
        }
    }

    fn keepalive_len() -> usize {
        INSTALL_THEMES_KEEPALIVE.with(|k| if k.borrow().is_some() { 1 } else { 0 })
    }

    /// Regression test for the `INSTALL_THEMES_KEEPALIVE` Vec growth audit
    /// finding. Repeated calls to `install_themes` (hot-reload, fixture
    /// teardown, tests) must not append to the keepalive indefinitely.
    /// The keepalive should hold at most one current effect; older
    /// installs are superseded and dropped cleanly.
    #[test]
    fn install_themes_keepalive_is_bounded_across_repeated_calls() {
        let baseline = keepalive_len();
        let variants: [(&'static str, TestTheme); 2] = [
            ("light", TestTheme { name: "light" }),
            ("dark", TestTheme { name: "dark" }),
        ];
        for _ in 0..16 {
            let active = Signal::new("light".to_string());
            install_themes(active, &variants);
        }
        let len_after = keepalive_len();
        let leak = len_after.saturating_sub(baseline);
        // Drop the keepalive before test return so the Effect's arena slot
        // is freed while ARENA's thread-local is still alive (thread-teardown
        // ordering would otherwise panic when dropping an `owns:true` effect).
        INSTALL_THEMES_KEEPALIVE.with(|k| *k.borrow_mut() = None);
        assert!(
            leak <= 1,
            "INSTALL_THEMES_KEEPALIVE grew by {leak} entries across 16 calls; \
             expected at most 1 (each install supersedes the previous)",
        );
    }
}
