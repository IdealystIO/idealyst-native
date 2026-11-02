//! Author surface for the platform's system Back (Android's back button
//! and back gesture), scoped to the component that asks for it.
//!
//! ```ignore
//! #[component]
//! fn KioskRoot() -> Element {
//!     // A kiosk: Back at the root never leaves the app.
//!     on_back(|| BackOutcome::Consume);
//!     ui! { /* … */ }
//! }
//! ```
//!
//! A handler sees the Back that would otherwise leave the app: native
//! navigators take Back first (a stack pops its pushed screens, and a
//! screen with `back_enabled(false)` keeps it). Handlers run newest first;
//! the first to return [`BackOutcome::Consume`] ends it, and when all
//! return [`BackOutcome::Default`] the platform does its default (Android
//! finishes the Activity or moves its task to the back).
//!
//! Platforms without a system Back (iOS, macOS, desktop, terminal) never
//! run a handler, and the web's history Back cannot be intercepted, so the
//! same tree compiles and behaves on every backend: the handler is simply
//! never offered a Back there. See `runtime_shared::system_back` for the
//! substrate contract.

use runtime_shared::system_back::{self as substrate, BackHandler, BackOutcome};

/// Handle the system Back for as long as the calling scope lives — a
/// component body (until it unmounts), an effect run (until it re-runs),
/// or a scoped deferred body. Return [`BackOutcome::Consume`] to keep the
/// app where it is, [`BackOutcome::Default`] to let the next handler (and
/// then the platform) have it.
///
/// The handler runs from the platform's Back callback; signal writes in it
/// commit on the backend's flush right after, like any event handler's.
///
/// Outside any scope (e.g. a button's `on_click`) there is no lifetime to
/// tie it to: use [`add_back_handler`] and hold the guard yourself. Calling
/// this there logs a warning and registers nothing, rather than leaking a
/// handler for the rest of the app's life.
pub fn on_back(handler: impl Fn() -> BackOutcome + 'static) {
    let guard = substrate::add_handler(handler);
    if let Err(guard) = crate::scoped_scheduling::own_in_scope(guard) {
        drop(guard);
        runtime_shared::logging::log(
            runtime_shared::logging::LogLevel::Warn,
            "on_back called outside any component/effect scope; nothing registered. \
             Use add_back_handler and keep the returned guard.",
        );
    }
}

/// Register a system-Back handler with an explicit lifetime: it stays
/// live until the returned guard drops. The imperative counterpart of
/// [`on_back`] for code that runs outside a scope.
pub fn add_back_handler(handler: impl Fn() -> BackOutcome + 'static) -> BackHandler {
    substrate::add_handler(handler)
}
