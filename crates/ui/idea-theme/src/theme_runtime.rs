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

use runtime_core::{install_tokens, update_tokens, watch, Color, Signal, Subscription};

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

/// Cloneable, identity-comparable wrapper for the stashed theme.
/// The new core's world signals require `PartialEq` (the kernel's
/// equality cut), which `Rc<dyn Any>` lacks; pointer identity is the
/// honest comparison for "same theme object". Swaps go through
/// `set_always` on both cores, so the eq impl never suppresses a
/// re-install notification.
#[derive(Clone)]
pub(crate) struct ThemeSlot(pub(crate) Rc<dyn Any>);

impl PartialEq for ThemeSlot {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Storage for the active-theme signal — the ONE per-core fork in this
/// crate, because the state MODEL (not an API name) differs:
///
/// - **Old core**: a thread-lifetime TLS singleton. Signals live in the
///   thread-local arena forever (the documented `unscope` contract), so
///   a TLS handle is always valid.
/// - **New core**: signals are world-backed and worlds are transient
///   (one per SSR request / mounted app). The handle lives in the
///   WORLD's typed context (`provide`/`inject`), mirroring how the
///   vocabulary's own ThemeCtx is stored; each world's first
///   `install_theme` creates its own slot. A TLS capture of the
///   last-installed handle exists ALONGSIDE the context, consulted only
///   OUTSIDE `World::enter`: platform event handlers run there and the
///   context inject would panic (the docs-shell dark-button abort).
///   Writes through a handle whose world died are silent kernel no-ops,
///   so the capture can go stale but never dangle.
#[cfg(not(feature = "new-core"))]
mod active_slot {
    use super::ThemeSlot;
    use runtime_core::Signal;
    use std::cell::RefCell;

    thread_local! {
        static ACTIVE_THEME: RefCell<Option<Signal<ThemeSlot>>> = const { RefCell::new(None) };
    }

    pub(super) fn current() -> Option<Signal<ThemeSlot>> {
        ACTIVE_THEME.with(|t| *t.borrow())
    }

    pub(super) fn store(sig: Signal<ThemeSlot>) {
        ACTIVE_THEME.with(|t| *t.borrow_mut() = Some(sig));
    }
}

#[cfg(feature = "new-core")]
mod active_slot {
    use super::ThemeSlot;
    use runtime_core::Signal;
    use std::cell::RefCell;

    /// Typed world-context key (contexts are keyed by `TypeId`).
    #[derive(Clone)]
    struct ActiveTheme(Signal<ThemeSlot>);

    thread_local! {
        /// The last-installed slot's HANDLE — the handler fallback.
        /// Platform event handlers run OUTSIDE `World::enter`, where the
        /// context `inject` panics — so `set_theme` from a button
        /// handler must reach the slot through a handle captured at
        /// install time: capture, don't inject. (Found live: the
        /// idea-ui-docs dark-theme button aborted "outside
        /// World::enter" through this module's inject.) The handle is
        /// `Copy` and routes to its OWN world; a write after that world
        /// died is a silent no-op, and any live world's fresh install
        /// re-captures — so staleness is inert, never dangling.
        static LAST_ACTIVE: RefCell<Option<Signal<ThemeSlot>>> = const { RefCell::new(None) };
    }

    pub(super) fn current() -> Option<Signal<ThemeSlot>> {
        if runtime_core::__world_is_entered() {
            // Ambient path: per-world isolation (an SSR request world
            // never sees the app world's theme, and vice versa).
            runtime_core::inject::<ActiveTheme>().map(|a| a.0)
        } else {
            // Handler path: the last ambient install's handle.
            LAST_ACTIVE.with(|t| *t.borrow())
        }
    }

    pub(super) fn store(sig: Signal<ThemeSlot>) {
        runtime_core::provide(ActiveTheme(sig));
        LAST_ACTIVE.with(|t| *t.borrow_mut() = Some(sig));
    }
}

thread_local! {
    /// Owns [`install_themes`]'s theme-variant `watch` for the process
    /// lifetime. `install_themes` runs at app boot, outside any render
    /// scope, so the subscription is caller-owned and this is the caller.
    ///
    /// Single-slot: each [`install_themes`] call replaces the previous
    /// keepalive, dropping its `Subscription` and disposing the prior
    /// effect. That way a hot-reload or fixture teardown that re-installs
    /// the theme system doesn't leak one subscription per call. Two
    /// concurrent active-theme signals never make sense — the new install
    /// supersedes the old one. (Dropping a stale-world subscription on the
    /// new core is a guarded no-op — `Owned::drop` skips dead arenas.)
    static INSTALL_THEMES_KEEPALIVE: RefCell<Option<Subscription>> = const { RefCell::new(None) };
}

/// Install the initial active theme. Call once at app startup
/// before rendering. Stashes the theme as `Rc<dyn Any>` in this
/// module's signal and forwards its `tokens()` to
/// [`runtime_core::install_tokens`].
pub fn install_theme<T: ThemeTokens + 'static>(theme: T) {
    let tokens = theme.tokens();
    let rc: Rc<dyn Any> = Rc::new(theme);
    store_active_theme(rc);
    install_tokens(&tokens);
    apply_host_surface_from_tokens(&tokens);
}

/// Store `rc` as the active theme: reuse the existing thread-local signal
/// slot when present, otherwise create it **outside any render scope**.
///
/// `ACTIVE_THEME` is a thread-lifetime singleton owned by this module's
/// thread-local — NOT by a reactive scope. Its backing signal must therefore
/// be created via [`runtime_core::unscope`] so the *first* caller's scope
/// doesn't adopt it. The hazard, concretely: an embedded sub-app (e.g. a
/// whiteboard editor mounted on a navigator screen) re-installs the theme
/// while building inside its screen's transient scope. If that scope owned
/// the signal, popping the screen would free the slot while this thread-local
/// kept pointing at it — and the next `active_theme()` from a still-mounted
/// sibling (a drawer header re-tinting on the back-nav, say) would read a
/// recycled slot and abort with "signal used after its scope was dropped".
/// Reusing the slot on re-install also keeps a stable signal id (no
/// per-install leak across repeated installs).
fn store_active_theme(rc: Rc<dyn Any>) {
    if let Some(sig) = active_slot::current() {
        // `set_always`: a theme re-install must notify even if the same
        // Rc is re-stored (and `ThemeSlot`'s ptr-eq must never suppress
        // a swap).
        sig.set_always(ThemeSlot(rc));
        return;
    }
    let sig = runtime_core::unscope(|| runtime_core::signal(ThemeSlot(rc)));
    active_slot::store(sig);
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

    // Reuse the existing slot, or lazily create it outside any render scope —
    // see [`store_active_theme`] for why the un-scoping matters.
    store_active_theme(rc);

    update_tokens(&tokens);
    apply_host_surface_from_tokens(&tokens);
}

/// Token name for the host-surface background — body/UIWindow/clear color.
/// Same name across every idea-theme variant so the web backend's
/// `var(--color-background)` reference auto-resolves on swap.
const HOST_BG_TOKEN: &str = "color-background";

/// Token name for the platform scrollbar thumb. Tighter contrast on
/// hover comes from the web backend's `::-webkit-scrollbar-thumb:hover`
/// rule reading `--color-text-muted`, but that's web-only chrome.
const SCROLLBAR_THUMB_TOKEN: &str = "color-border-strong";

/// Token name for the scrollbar **track** (gutter). An OPAQUE surface, not
/// `transparent`: the `::-webkit-scrollbar` rule is global and several
/// scroll surfaces (notably the drawer-sidebar wrapper) have a transparent
/// background, so a transparent track let a floating/legacy scrollbar
/// reveal the content *behind* the scroller. `color-surface-alt` reads as a
/// subtle gutter on white surfaces and blends into the canvas on the page
/// scroll, while staying opaque so nothing shows through. Re-tints on theme
/// swap like the thumb.
const SCROLLBAR_TRACK_TOKEN: &str = "color-surface-alt";

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
            scrollbar_track_ref(tokens),
        );
    }
}

/// Resolve the scrollbar track reference from a theme's tokens: a themed,
/// opaque [`SCROLLBAR_TRACK_TOKEN`] surface, or `transparent` only when the
/// theme defines no such token. Pure (the regression test pins it).
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
    // `install_themes` runs outside a render scope, so the `watch` is
    // caller-owned; this keepalive is what keeps it alive past the
    // function return.
    //
    // Single-slot replacement: storing the new `Subscription` drops the
    // previous `Option`'s one, disposing the prior install's effect and
    // preventing unbounded growth across repeated calls.
    INSTALL_THEMES_KEEPALIVE.with(|k| *k.borrow_mut() = Some(sub));
}

/// Read the active theme. Subscribes the current effect (if any) to
/// theme changes — that's how reactive style application works for
/// callers that read theme struct fields directly (as opposed to
/// via tokenized stylesheet references).
///
/// Panics if no theme has been installed. Call [`install_theme`]
/// before render.
pub fn active_theme() -> Rc<dyn Any> {
    active_slot::current()
        .expect("no theme installed; call idea_ui::install_theme(...) before rendering")
        .get()
        .0
}

/// Whether a theme has been installed on this thread yet — a non-panicking
/// peek at the [`active_theme`] singleton.
///
/// Lets an embeddable sub-app decide whether to install its own theme: a
/// component mounted inside a host that already installed one (e.g. a
/// whiteboard editor on a themed app shell) should skip its own install so it
/// doesn't clobber the host's global theme. Standalone (no host theme) it
/// installs as usual.
pub fn theme_installed() -> bool {
    active_slot::current().is_some()
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

/// The active idea-theme's default body [`FontFamily`](runtime_core::FontFamily).
///
/// Exists for sheets that style **portal'd** text — the Select dropdown,
/// the Autocomplete menu, etc. On web a portal mounts under `<body>`,
/// OUTSIDE the app tree's inherited `font-family`, so its text falls back
/// to the browser's serif default unless the menu pins the font itself
/// (the "dropdown options render in Times" bug). Stamping this onto the
/// menu panel keeps the dropdown's font cascade self-contained instead of
/// depending on an ancestor that the portal has escaped.
///
/// Reads [`active_theme`] (tracked) so a theme swap re-resolves the font
/// in place, mirroring the Typography base sheet.
///
/// Panics if no theme is installed, exactly like [`active_theme`].
pub fn active_font_family() -> runtime_core::FontFamily {
    use crate::theme::IdeaTheme;
    active_theme()
        .downcast_ref::<crate::theme::IdeaThemeRef>()
        .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first")
        .font_family()
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

    /// Regression: the scrollbar track was hardcoded `transparent`, so a
    /// floating/legacy scrollbar revealed the content *behind* any scroll
    /// surface with a transparent background (the drawer-sidebar wrapper).
    /// The track must now resolve to the opaque `color-surface-alt` token.
    #[test]
    fn scrollbar_track_is_opaque_surface_not_transparent() {
        let tokens = vec![
            TokenEntry {
                name: "color-surface-alt",
                value: TokenValue::Color(Color("#f1f5f9".into())),
            },
            TokenEntry {
                name: "color-border-strong",
                value: TokenValue::Color(Color("#94a3b8".into())),
            },
        ];
        match scrollbar_track_ref(&tokens) {
            Tokenized::Token { name, fallback } => {
                assert_eq!(name, SCROLLBAR_TRACK_TOKEN);
                assert_ne!(
                    fallback.0.to_ascii_lowercase(),
                    "transparent",
                    "the track must be an opaque surface, not transparent",
                );
            }
            Tokenized::Literal(_) => panic!("expected a themed track token, got a literal"),
        }

        // Graceful fallback: a theme that defines no surface-alt keeps the
        // old transparent track rather than emitting a dangling token ref.
        let bare = vec![TokenEntry {
            name: "color-border-strong",
            value: TokenValue::Color(Color("#94a3b8".into())),
        }];
        match scrollbar_track_ref(&bare) {
            Tokenized::Literal(c) => assert_eq!(c.0.to_ascii_lowercase(), "transparent"),
            Tokenized::Token { .. } => {
                panic!("a theme without surface-alt must fall back to a transparent literal")
            }
        }
    }

    /// The docs-shell dark-theme-button crash (new core only): the whole
    /// swap surface — `set_theme` → slot `set_always` + token/host-surface
    /// forwarding — used to be ambient-only (`inject` from the world
    /// context), and platform event handlers run OUTSIDE `World::enter`,
    /// so the first theme swap aborted "signal()/effect() called outside
    /// World::enter". The swap must instead ride the install-time
    /// captures (slot handle + vocabulary ThemeCtx), stage, and commit
    /// on the owning world's next flush.
    #[cfg(feature = "new-core")]
    #[test]
    fn regression_set_theme_from_handler_outside_enter_commits_on_flush() {
        let world = runtime_core::__World::new();
        world.enter(|| install_theme(TestTheme { name: "light" }));
        world.flush();

        // The button handler: no ambient world. Must not panic.
        set_theme(TestTheme { name: "dark" });

        // The flush driver commits afterwards; the entered read then
        // sees the swapped theme through the per-world context.
        world.flush();
        let name = world.enter(|| {
            active_theme()
                .downcast_ref::<TestTheme>()
                .expect("stashed theme keeps its concrete type")
                .name
        });
        assert_eq!(name, "dark", "handler swap committed on flush");
    }

    /// Regression test for the `INSTALL_THEMES_KEEPALIVE` Vec growth audit
    /// finding. Repeated calls to `install_themes` (hot-reload, fixture
    /// teardown, tests) must not append to the keepalive indefinitely.
    /// The keepalive should hold at most one current effect; older
    /// installs are superseded and dropped cleanly.
    #[test]
    fn install_themes_keepalive_is_bounded_across_repeated_calls() {
        crate::testing::with_test_world(|| {
        let baseline = keepalive_len();
        let variants: [(&'static str, TestTheme); 2] = [
            ("light", TestTheme { name: "light" }),
            ("dark", TestTheme { name: "dark" }),
        ];
        for _ in 0..16 {
            // `signal(...)` == old `Signal::new(...)`; see the theme.rs test.
            let active = runtime_core::signal("light".to_string());
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
        });
    }
}
