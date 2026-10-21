//! The active-theme stash: "the theme is a struct" for any component
//! library.
//!
//! The style engine itself only knows **tokens** (named values, plus
//! `Tokenized<T>` references in rules). A component library usually also
//! wants the theme as a typed object — component code reading a font, a
//! hover overlay, a palette entry it does not express as a token — and a
//! way to swap it at runtime. This module is that convention, generic over
//! the theme type: a theme implements [`ThemeTokens`], [`install_theme`]
//! stashes it (and installs its tokens), [`active_theme`] reads it back
//! (`downcast_ref` to the concrete type), [`set_theme`] swaps it, and
//! [`install_themes`] drives the active variant from a `Signal<String>`.
//!
//! ```no_run
//! use runtime_vocabulary::glue::{install_theme, set_theme, ThemeTokens, TokenEntry, TokenValue};
//!
//! struct MyTheme { accent: String }
//! impl ThemeTokens for MyTheme {
//!     fn tokens(&self) -> Vec<TokenEntry> {
//!         vec![TokenEntry {
//!             name: "accent",
//!             value: TokenValue::Color(self.accent.as_str().into()),
//!         }]
//!     }
//! }
//!
//! install_theme(MyTheme { accent: "#06f".to_string() });
//! set_theme(MyTheme { accent: "#39f".to_string() });
//! ```
//!
//! # Host surface
//!
//! [`install_theme`] / [`set_theme`] also paint the host surface (page /
//! window background and the platform scrollbar) from three well-known
//! token names when the theme defines them — [`HOST_BACKGROUND_TOKEN`],
//! [`SCROLLBAR_THUMB_TOKEN`], [`SCROLLBAR_TRACK_TOKEN`] — through
//! [`apply_host_surface_tokens`]. A library that installs its tokens
//! another way (a `#[derive(Tokens)]` set) and stashes its theme object
//! with [`stash_active_theme`] calls that function itself.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_shared::{Color, TokenEntry, TokenValue, Tokenized};
use runtime_world::Signal;

use crate::glue::{watch, Subscription};

/// A theme that exposes its tokens by name and concrete value.
///
/// Implement on your theme struct (whatever shape it has); `tokens()`
/// returns the `(name, value)` pairs installed as runtime variables. The
/// names must match the `Tokenized::Token { name, .. }` references the
/// stylesheets construct.
pub trait ThemeTokens: Any {
    fn tokens(&self) -> Vec<TokenEntry>;
}

/// Cloneable, identity-comparable wrapper for the stashed theme.
/// World signals require `PartialEq` (the kernel's equality cut), which
/// `Rc<dyn Any>` lacks; pointer identity is the honest comparison for
/// "same theme object". Swaps write in place and `touch` (see
/// [`stash_active_theme`]), so the eq impl never suppresses a re-install
/// notification.
#[derive(Clone)]
struct ThemeSlot(Rc<dyn Any>);

impl PartialEq for ThemeSlot {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Storage for the active-theme signal.
///
/// Signals are world-backed and worlds are transient (one per SSR
/// request / mounted app). The handle lives in the WORLD's typed context
/// (`provide`/`inject`), mirroring how the vocabulary's own `ThemeCtx` is
/// stored; each world's first install creates its own slot. A TLS capture
/// of the last-installed handle exists ALONGSIDE the context, consulted
/// only OUTSIDE `World::enter`: platform event handlers run there and the
/// context inject would panic (a theme-toggle button's handler aborting
/// "outside World::enter"). Writes through a handle whose world died are
/// silent kernel no-ops, so the capture can go stale but never dangle.
mod active_slot {
    use super::ThemeSlot;
    use runtime_world::Signal;
    use std::cell::RefCell;

    /// Typed world-context key (contexts are keyed by `TypeId`).
    #[derive(Clone)]
    struct ActiveTheme(Signal<ThemeSlot>);

    runtime_tls::thread_local! {
        /// The last-installed slot's HANDLE — the handler fallback (see
        /// the module docs: capture, don't inject, outside `enter`).
        static LAST_ACTIVE: RefCell<Option<Signal<ThemeSlot>>> = const { RefCell::new(None) };
    }

    pub(super) fn current() -> Option<Signal<ThemeSlot>> {
        if runtime_world::is_entered() {
            // Ambient path: per-world isolation (an SSR request world
            // never sees the app world's theme, and vice versa).
            crate::glue::inject::<ActiveTheme>().map(|a| a.0)
        } else {
            LAST_ACTIVE.with(|t| *t.borrow())
        }
    }

    pub(super) fn store(sig: Signal<ThemeSlot>) {
        // `unscope` the PROVISION too, not just the signal: a context
        // entry is owned by the scope that made it, and `store` runs from
        // whichever render scope first installed a theme. Scope-owned, the
        // slot would stop being injectable on that subtree's unmount and
        // the next `current()` would fall back to a fresh theme.
        crate::glue::unscope(|| crate::glue::provide(ActiveTheme(sig)));
        LAST_ACTIVE.with(|t| *t.borrow_mut() = Some(sig));
    }
}

runtime_tls::thread_local! {
    /// Owns [`install_themes`]'s theme-variant `watch`. Single-slot: each
    /// call replaces the previous keepalive, dropping its `Subscription`
    /// and disposing the prior effect, so a hot reload or fixture teardown
    /// that re-installs never leaks one subscription per call. (Dropping a
    /// stale-world subscription is a guarded no-op.)
    static INSTALL_THEMES_KEEPALIVE: RefCell<Option<Subscription>> = const { RefCell::new(None) };
}

/// Install the initial active theme. Call once at app startup before
/// rendering: stashes the theme (what [`active_theme`] returns), installs
/// its `tokens()` and paints the host surface from them.
pub fn install_theme<T: ThemeTokens + 'static>(theme: T) {
    let tokens = theme.tokens();
    stash_active_theme(Rc::new(theme));
    crate::glue::install_tokens(&tokens);
    apply_host_surface_tokens(&tokens);
}

/// Swap the active theme: re-stash it (waking every [`active_theme`]
/// reader) and push its tokens through `update_tokens`, which re-resolves
/// every token reference in place.
pub fn set_theme<T: ThemeTokens + 'static>(theme: T) {
    let tokens = theme.tokens();
    stash_active_theme(Rc::new(theme));
    crate::glue::update_tokens(&tokens);
    apply_host_surface_tokens(&tokens);
}

/// Store `theme` as the active theme object without touching tokens — for a
/// library that installs its tokens through its own token set and only
/// needs the object stashed (idea-theme's `install_idea_theme`).
///
/// Reuses the existing signal slot when present, otherwise creates it
/// **outside any render scope**: the slot is owned by the world context
/// plus the handler capture, not by a reactive scope. An embedded sub-app
/// re-installing the theme from inside a navigator screen's transient
/// scope would otherwise have that scope own the slot — popping the screen
/// freed it while the stored handle kept pointing at it, and the next read
/// from a still-mounted sibling aborted "signal used after its scope was
/// dropped". Reusing the slot also keeps one signal id across re-installs.
pub fn stash_active_theme(theme: Rc<dyn Any>) {
    if let Some(sig) = active_slot::current() {
        // Written in place, then `touch`ed — NOT staged with `set_always`.
        // A staged write stays invisible until the owning world flushes,
        // and the theme is read right after it is (re)installed: a hot
        // patch's rebuild re-runs the app entry, re-installs the theme and
        // builds every component body against it in the same turn, and
        // those bodies read untracked — a staged write would hand them the
        // previous theme for good. `touch` still wakes every subscriber at
        // the next flush, even when the same `Rc` is re-stored.
        sig.set_untracked(ThemeSlot(theme));
        sig.touch();
        return;
    }
    let sig = crate::glue::unscope(|| runtime_world::signal(ThemeSlot(theme)));
    active_slot::store(sig);
}

/// Token naming the host-surface background (body / UIWindow / clear
/// color).
pub const HOST_BACKGROUND_TOKEN: &str = "color-background";
/// Token naming the platform scrollbar thumb.
pub const SCROLLBAR_THUMB_TOKEN: &str = "color-border-strong";
/// Token naming the scrollbar track (gutter). An OPAQUE surface is
/// expected: the web `::-webkit-scrollbar` rule is global and several
/// scroll surfaces have a transparent background, so a transparent track
/// let a floating scrollbar reveal the content behind the scroller.
pub const SCROLLBAR_TRACK_TOKEN: &str = "color-surface-alt";

/// Route the host-surface tokens in `tokens` (looked up by the well-known
/// names above) to `set_app_background` / `set_scrollbar_theme`. Called on
/// install AND on swap: native backends apply the resolved value directly
/// and have no `var(--…)` indirection, so they repaint only when told.
pub fn apply_host_surface_tokens(tokens: &[TokenEntry]) {
    if let Some(fallback) = lookup_color(tokens, HOST_BACKGROUND_TOKEN) {
        crate::glue::set_app_background(Tokenized::Token { name: HOST_BACKGROUND_TOKEN, fallback });
    }
    if let Some(fallback) = lookup_color(tokens, SCROLLBAR_THUMB_TOKEN) {
        crate::glue::set_scrollbar_theme(
            Tokenized::Token { name: SCROLLBAR_THUMB_TOKEN, fallback },
            scrollbar_track_ref(tokens),
        );
    }
}

/// The scrollbar track reference: the themed opaque
/// [`SCROLLBAR_TRACK_TOKEN`] surface, or `transparent` only when the theme
/// defines no such token.
fn scrollbar_track_ref(tokens: &[TokenEntry]) -> Tokenized<Color> {
    match lookup_color(tokens, SCROLLBAR_TRACK_TOKEN) {
        Some(fallback) => Tokenized::Token { name: SCROLLBAR_TRACK_TOKEN, fallback },
        None => Tokenized::Literal(Color("transparent".into())),
    }
}

fn lookup_color(tokens: &[TokenEntry], name: &str) -> Option<Color> {
    tokens.iter().find(|t| t.name == name).and_then(|t| match &t.value {
        TokenValue::Color(c) => Some(c.clone()),
        _ => None,
    })
}

/// Install a multi-variant theme system whose active variant is named by
/// a `Signal<String>`. The signal's current value picks the initial theme
/// (no match panics at install, before anything renders); an internal
/// effect calls [`set_theme`] whenever the name changes.
pub fn install_themes<T: ThemeTokens + Clone + 'static>(active: Signal<String>, variants: &[(&'static str, T)]) {
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

    let variants_owned: HashMap<String, T> =
        variants.iter().map(|(name, theme)| (name.to_string(), theme.clone())).collect();
    let last_seen: Rc<RefCell<String>> = Rc::new(RefCell::new(initial_name));
    let sub = watch(move || {
        let name = active.get();
        if last_seen.borrow().as_str() == name.as_str() {
            return;
        }
        if let Some(theme) = variants_owned.get(&name) {
            set_theme(theme.clone());
            *last_seen.borrow_mut() = name;
        }
    });
    INSTALL_THEMES_KEEPALIVE.with(|k| *k.borrow_mut() = Some(sub));
}

/// Read the active theme. Subscribes the current effect (if any) to theme
/// changes. `downcast_ref` to your theme's concrete type.
///
/// Panics if no theme has been installed — install one before rendering.
pub fn active_theme() -> Rc<dyn Any> {
    active_slot::current()
        .expect("no theme installed; call install_theme(...) (or your library's theme install) before rendering")
        .get()
        .0
}

/// Whether a theme has been installed yet — a non-panicking peek at
/// [`active_theme`]. Lets an embeddable sub-app skip its own install when a
/// host already installed one.
pub fn theme_installed() -> bool {
    active_slot::current().is_some()
}

/// [`active_theme`] without subscribing the current effect — for reads
/// that discard the value or must not re-run on a swap.
pub fn active_theme_untracked() -> Rc<dyn Any> {
    runtime_world::untrack(active_theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct TestTheme {
        name: &'static str,
    }
    impl ThemeTokens for TestTheme {
        fn tokens(&self) -> Vec<TokenEntry> {
            vec![TokenEntry { name: "test.value", value: TokenValue::Number(1.0) }]
        }
    }

    fn keepalive_set() -> bool {
        INSTALL_THEMES_KEEPALIVE.with(|k| k.borrow().is_some())
    }

    /// Regression: the scrollbar track was hardcoded `transparent`, so a
    /// floating scrollbar revealed the content behind any scroll surface
    /// with a transparent background. The track must resolve to the opaque
    /// surface token when the theme has one.
    #[test]
    fn scrollbar_track_is_opaque_surface_not_transparent() {
        let tokens = vec![
            TokenEntry { name: "color-surface-alt", value: TokenValue::Color(Color("#f1f5f9".into())) },
            TokenEntry { name: "color-border-strong", value: TokenValue::Color(Color("#94a3b8".into())) },
        ];
        match scrollbar_track_ref(&tokens) {
            Tokenized::Token { name, fallback } => {
                assert_eq!(name, SCROLLBAR_TRACK_TOKEN);
                assert_ne!(fallback.0.to_ascii_lowercase(), "transparent");
            }
            Tokenized::Literal(_) => panic!("expected a themed track token, got a literal"),
        }
        let bare = vec![TokenEntry { name: "color-border-strong", value: TokenValue::Color(Color("#94a3b8".into())) }];
        match scrollbar_track_ref(&bare) {
            Tokenized::Literal(c) => assert_eq!(c.0.to_ascii_lowercase(), "transparent"),
            Tokenized::Token { .. } => panic!("no surface-alt token must fall back to transparent"),
        }
    }

    /// Regression: a theme-toggle handler runs OUTSIDE `World::enter`, and
    /// the swap surface used to be ambient-only, so the first swap aborted
    /// "outside World::enter". The swap rides the install-time captures
    /// and commits on the owning world's next flush.
    #[test]
    fn regression_set_theme_from_handler_outside_enter_commits_on_flush() {
        let world = runtime_world::World::new();
        world.enter(|| install_theme(TestTheme { name: "light" }));
        world.flush();
        set_theme(TestTheme { name: "dark" });
        world.flush();
        let name = world.enter(|| active_theme().downcast_ref::<TestTheme>().expect("concrete type").name);
        assert_eq!(name, "dark");
    }

    /// Regression (hot-patch rebuild): a re-install used to STAGE the new
    /// theme, so a read in the same turn got the previous one and kept it.
    /// The re-installed theme must be what the next read returns, and a
    /// subscriber must still wake at the flush.
    #[test]
    fn regression_reinstall_is_visible_to_a_read_in_the_same_turn() {
        use std::cell::Cell;
        let world = runtime_world::World::new();
        world.enter(|| install_theme(TestTheme { name: "old" }));
        world.flush();
        let runs = Rc::new(Cell::new(0u32));
        let runs_in = runs.clone();
        let _sub = world.enter(|| {
            watch(move || {
                let _ = active_theme();
                runs_in.set(runs_in.get() + 1);
            })
        });
        let before = runs.get();
        let name = world.enter(|| {
            install_theme(TestTheme { name: "new" });
            active_theme().downcast_ref::<TestTheme>().expect("concrete type").name
        });
        assert_eq!(name, "new");
        world.flush();
        assert_eq!(runs.get(), before + 1);
    }

    /// Repeated `install_themes` calls must not grow the keepalive.
    #[test]
    fn install_themes_keepalive_is_bounded_across_repeated_calls() {
        let world = runtime_world::World::new();
        world.enter(|| {
            let variants: [(&'static str, TestTheme); 2] =
                [("light", TestTheme { name: "light" }), ("dark", TestTheme { name: "dark" })];
            for _ in 0..16 {
                let active = runtime_world::signal("light".to_string());
                install_themes(active, &variants);
            }
            assert!(keepalive_set());
            INSTALL_THEMES_KEEPALIVE.with(|k| *k.borrow_mut() = None);
        });
    }
}
