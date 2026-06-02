//! The global reactive locale signal — the single source of truth for
//! "what language are we rendering in right now".
//!
//! Modeled exactly on `runtime_core::viewport`: a thread-lifetime global
//! signal created via `unscope` so it isn't adopted by whatever render
//! scope happens to be active on first access (otherwise its arena slot
//! would dangle when that transient scope drops — the same trap the token
//! registry and viewport signal avoid).

use runtime_core::Signal;
use std::cell::OnceCell;
use std::rc::Rc;

/// Fallback code used before the app calls the generated `init()`. If no
/// locale named `"en"` exists, messages simply resolve through their
/// default-locale fallback, so this degrades gracefully.
const INITIAL_CODE: &str = "en";

thread_local! {
    /// The authoritative active-locale signal. Holds the BCP-47-ish code
    /// (`"en"`, `"fr"`, `"ja"`). `Rc<str>` so reads are cheap clones.
    static LOCALE: OnceCell<Signal<Rc<str>>> = const { OnceCell::new() };
}

fn locale_signal() -> Signal<Rc<str>> {
    LOCALE.with(|cell| {
        *cell.get_or_init(|| runtime_core::unscope(|| Signal::new(Rc::from(INITIAL_CODE))))
    })
}

/// The active locale code. Reading this inside a reactive scope (a
/// `Reactive::derive` / effect / memo) subscribes the scope to **both**
/// locale changes and opt-in pack installs, so generated message functions
/// re-render when either happens.
pub fn current_locale_code() -> Rc<str> {
    // Subscribe to pack installs too: a fetched opt-in pack arriving must
    // recompute the very derives that read the locale, upgrading them from
    // the default-locale fallback to the localized string.
    crate::packs::subscribe_epoch();
    locale_signal().get()
}

/// Set the active locale by code. Idempotent — a same-value call doesn't
/// re-fire dependents (compared by equality, like `set_viewport_size`).
///
/// This does **not** trigger an opt-in pack fetch; the generated typed
/// `set_locale(Locale)` does that for `lazy` locales. If you switch by raw
/// code to an opt-in locale, call [`crate::ensure_pack_loaded`] yourself.
pub fn set_locale_code(code: &str) {
    let sig = locale_signal();
    if &*sig.get() != code {
        sig.set(Rc::from(code));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_then_read_roundtrips() {
        set_locale_code("fr");
        assert_eq!(&*current_locale_code(), "fr");
        // Idempotent same-value set.
        set_locale_code("fr");
        assert_eq!(&*current_locale_code(), "fr");
        set_locale_code("en");
        assert_eq!(&*current_locale_code(), "en");
    }
}
