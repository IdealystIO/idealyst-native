//! Light/dark theme state for the docs, defaulting to the platform's
//! reported color scheme.
//!
//! `runtime_core::color_scheme()` carries the platform default captured
//! at mount (web `prefers-color-scheme`, iOS `UITraitCollection`, etc.),
//! so the docs open in the user's preferred mode instead of flashing
//! white. The user can then flip it with the sidebar toggle.
//!
//! ## Why the mode signal is created in `app()`, not lazily
//!
//! Signals may only be created with the world entered — component
//! builds and effects. Event handlers run OUTSIDE the world, so a
//! lazily-initialized `thread_local` signal whose first touch happened
//! to be the toggle's press handler would panic. [`init`] therefore runs
//! once from `app()`'s body (a build window) and stashes the handle;
//! every later reader — the toggle's label, `render_markdown`'s theme,
//! the press handler itself — copies the stashed `Signal<bool>`, which
//! is `Copy` and routes to its own world, so it is handler-safe.
//!
//! The same rule is why nothing here calls a free theme fn from a
//! handler ([`toggle_theme`] only writes the signal): `install_idea_theme`
//! resolves the ambient world's `ThemeCtx` and panics outside `enter`.
//! [`init`] installs the theme *reactively* instead — one effect, created
//! at build time, that swaps the token set whenever the mode flips.

use std::cell::RefCell;

use idea_ui::{dark_theme, install_idea_theme_reactive, light_theme};
use runtime_core::{color_scheme, signal, ColorScheme, Signal};

thread_local! {
    /// `true` = dark. Created once from `app()`'s build window (see the
    /// module docs) and read from every scope thereafter.
    static DARK_MODE: RefCell<Option<Signal<bool>>> = const { RefCell::new(None) };
}

/// Create the mode signal from the platform's reported preference and
/// wire the reactive theme install. Call once from `app()`'s body,
/// before the first screen builds.
pub fn init() {
    let prefers_dark = matches!(color_scheme(), ColorScheme::Dark);
    let sig = signal(prefers_dark);
    DARK_MODE.with(|d| *d.borrow_mut() = Some(sig));
    // First run installs (and registers every component sheet); later
    // runs `set_theme`, re-pushing tokens so live styles re-resolve in
    // place. The effect is owned by `app()`'s scope, i.e. the app.
    install_idea_theme_reactive(move || if sig.get() { dark_theme() } else { light_theme() });
}

/// The persistent dark-mode signal. Read it reactively (e.g. in a `text`
/// node or `rx!`) to label the toggle; the initial value is the
/// platform's reported preference (`Auto` → light).
///
/// Panics if called before [`init`] — that would mean a screen built
/// before `app()` ran, which cannot happen.
pub fn dark_mode() -> Signal<bool> {
    DARK_MODE
        .with(|d| *d.borrow())
        .expect("docs-app: theme::init() must run in app()'s body before any screen builds")
}

/// Flip light ⇆ dark. Handler-safe: it only writes the mode signal, and
/// the reactive install wired by [`init`] swaps the token set on the
/// resulting flush.
pub fn toggle_theme() {
    let sig = dark_mode();
    sig.set(!sig.get());
}
