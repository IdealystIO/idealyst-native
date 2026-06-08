//! Light/dark theme state for the docs, defaulting to the platform's
//! reported color scheme.
//!
//! `runtime_core::color_scheme()` carries the platform default captured
//! at mount (web `prefers-color-scheme`, iOS `UITraitCollection`, etc.),
//! so the docs open in the user's preferred mode instead of flashing
//! white. The user can then flip it with the sidebar toggle.
//!
//! The mode lives in a process-global `Signal<bool>` (created off any
//! render scope, the same pattern idea-ui's toast queue uses) so it
//! survives the navigator's per-screen scope swaps and the toggle button
//! living in the drawer's sidebar builder — both of which drop their
//! local reactive scopes.

use std::cell::RefCell;

use idea_ui::{dark_theme, install_idea_theme, light_theme, set_idea_theme};
use runtime_core::{color_scheme, unscope, ColorScheme, Signal};

thread_local! {
    /// `true` = dark. Lazily created from the platform default on first
    /// access; persists for the life of the app.
    static DARK_MODE: RefCell<Option<Signal<bool>>> = const { RefCell::new(None) };
}

/// The persistent dark-mode signal. Read it reactively (e.g. in a `text`
/// node or `rx!`) to label the toggle; the initial value is the
/// platform's reported preference (`Auto` → light).
pub fn dark_mode() -> Signal<bool> {
    DARK_MODE.with(|d| {
        if d.borrow().is_none() {
            let prefers_dark = matches!(color_scheme(), ColorScheme::Dark);
            // Off any render scope, so the signal outlives every screen.
            let sig = unscope(|| Signal::new(prefers_dark));
            *d.borrow_mut() = Some(sig);
        }
        *d.borrow().as_ref().unwrap()
    })
}

/// Install the theme matching the current mode. Call once at startup
/// (before the first render) so the platform default is honored.
pub fn install_initial_theme() {
    if dark_mode().get() {
        install_idea_theme(dark_theme());
    } else {
        install_idea_theme(light_theme());
    }
}

/// Flip light ⇆ dark: swap the live theme (idea-ui's `set_idea_theme`
/// re-pushes tokens in a batch, re-resolving styles in place) and update
/// the mode signal so the toggle's label/icon reacts.
pub fn toggle_theme() {
    let sig = dark_mode();
    let now_dark = !sig.get();
    if now_dark {
        set_idea_theme(dark_theme());
    } else {
        set_idea_theme(light_theme());
    }
    sig.set(now_dark);
}
