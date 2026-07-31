//! The reactive locale state — the single source of truth for "what
//! language are we rendering in right now".
//!
//! Modeled exactly on `runtime_vocabulary::viewport`'s `ViewportCtx`: the
//! locale signal and the pack-install epoch live in an [`I18nCtx`] stored in
//! the owning **world**'s context (`provide`/`inject`), not in a
//! thread-lifetime global.
//!
//! Why per-world and not a thread-local `OnceCell<Signal<_>>` (what this
//! module used to be): signal handles route to the world that created them,
//! worlds are transient (one per SSR request), and a read against a dropped
//! world panics. A thread-cached signal would therefore leak one request's
//! locale into the next on a reused thread, and blow up once the first
//! request's world was gone. The signals are created inside
//! [`runtime_core::unscope`] so they belong to the world ROOT rather than
//! being adopted by whichever transient subtree happens to read a locale
//! first (that scope's drop would recycle their slots).
//!
//! Reads/writes from an event handler are safe: handlers run outside
//! `World::enter`, so the ambient lookup falls back to the thread's last
//! ambient ctx — signal handles carry their own world, and writes to a dead
//! world are silent no-ops, so a stale capture is inert rather than wrong.
//!
//! The *code* additionally lives in a thread-local SEED that a freshly created
//! world's signal starts from. That is what keeps the SSG loop working —
//! `set_locale_code("fr")` runs BEFORE `render_path` creates the request's
//! world, so there is no signal to write; the seed carries the choice into the
//! world the render mints. The seed is plain data (an `Rc<str>`), not a signal,
//! so sharing it across worlds is safe; only the reactive slot is per-world.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::{inject, provide, signal, unscope, Signal};

/// Fallback code used before the app calls the generated `init()`. If no
/// locale named `"en"` exists, messages simply resolve through their
/// default-locale fallback, so this degrades gracefully.
const INITIAL_CODE: &str = "en";

/// Per-world i18n state. `Copy` because both fields are `Copy` signal
/// handles — a clone names the same two slots.
#[derive(Clone, Copy)]
pub(crate) struct I18nCtx {
    /// The active BCP-47-ish code (`"en"`, `"fr"`, `"ja"`). `Rc<str>` so
    /// reads are cheap clones.
    code: Signal<Rc<str>>,
    /// Monotonic counter bumped on every opt-in pack install. Reactive
    /// scopes that read the locale also read this, so a pack arriving
    /// recomputes the derives that were falling back to the default locale.
    epoch: Signal<u64>,
}

thread_local! {
    /// The most recent ambient world's ctx — the HANDLER fallback, refreshed
    /// on every ambient lookup and consulted only when no world is entered.
    /// Safe when stale: handles route to their own world and dead-world
    /// writes no-op.
    static LAST_CTX: Cell<Option<I18nCtx>> = const { Cell::new(None) };

    /// The code a newly created world's locale signal starts at. Plain data,
    /// shared by every world on the thread (see the module docs' SSG note).
    static SEED_CODE: RefCell<Option<Rc<str>>> = const { RefCell::new(None) };
}

fn seed_code() -> Rc<str> {
    SEED_CODE
        .with(|c| c.borrow().clone())
        .unwrap_or_else(|| Rc::from(INITIAL_CODE))
}

/// The ambient world's i18n context, created (and `provide`d) on first use.
/// Outside `World::enter` this returns the thread's last ambient ctx; with
/// no ctx ever created it panics through the creation path with the kernel's
/// canonical outside-`enter` message, which is the correct diagnosis.
fn ctx() -> I18nCtx {
    if let Some(found) = try_ctx() {
        return found;
    }
    let seed = seed_code();
    // The provision is world-root alongside the signals it carries — a
    // context entry belongs to the scope that made it, and first-touch
    // happens in whatever subtree localized a string first. Scope-owned,
    // the ctx would vanish on that subtree's unmount and the next `ctx()`
    // would build a second one, orphaning every locale subscriber.
    let fresh = unscope(|| {
        let fresh = I18nCtx { code: signal(seed), epoch: signal(0u64) };
        provide(fresh);
        fresh
    });
    LAST_CTX.with(|c| c.set(Some(fresh)));
    fresh
}

/// The ambient (or last-ambient) ctx **without creating one**. Used by the
/// notify side: with no ctx there is nothing subscribed, so there is nothing
/// to notify and a missing world is not an error.
pub(crate) fn try_ctx() -> Option<I18nCtx> {
    if !runtime_core::__world_is_entered() {
        return LAST_CTX.with(|c| c.get());
    }
    let found = inject::<I18nCtx>()?;
    LAST_CTX.with(|c| c.set(Some(found)));
    Some(found)
}

impl I18nCtx {
    /// Subscribe the calling reactive scope to pack installs.
    pub(crate) fn subscribe_epoch(&self) {
        let _ = self.epoch.get();
    }

    /// Bump the pack epoch, waking every derive that read the locale.
    pub(crate) fn bump_epoch(&self) {
        self.epoch.update(|n| n.wrapping_add(1));
    }
}

/// The active locale code. Reading this inside a reactive scope (a
/// `Reactive::derive` / effect / memo) subscribes the scope to **both**
/// locale changes and opt-in pack installs, so generated message functions
/// re-render when either happens.
pub fn current_locale_code() -> Rc<str> {
    let ctx = ctx();
    // Subscribe to pack installs too: a fetched opt-in pack arriving must
    // recompute the very derives that read the locale, upgrading them from
    // the default-locale fallback to the localized string.
    ctx.subscribe_epoch();
    ctx.code.get()
}

/// Set the active locale by code. Idempotent — a same-value call doesn't
/// re-fire dependents (the kernel's `set` is equality-guarded at commit).
///
/// The write **stages**: dependents observe the new locale at the driver's
/// flush, not on the next line. That is the framework-wide contract for
/// every signal write — a handler that switches locale and then reads a
/// message back in the same turn still sees the previous string.
///
/// Callable **before** any world exists (the SSG loop's per-locale pass runs
/// before `render_path` mints the request's world): the code is recorded as
/// the seed every subsequently-created world starts from, and only the live
/// signal write is skipped — there is nothing rendered to notify.
///
/// This does **not** trigger an opt-in pack fetch; the generated typed
/// `set_locale(Locale)` does that for `lazy` locales. If you switch by raw
/// code to an opt-in locale, call [`crate::ensure_pack_loaded`] yourself.
pub fn set_locale_code(code: &str) {
    let code: Rc<str> = Rc::from(code);
    SEED_CODE.with(|c| *c.borrow_mut() = Some(code.clone()));
    if let Some(ctx) = try_ctx() {
        ctx.code.set(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_then_read_roundtrips() {
        runtime_core::__with_fresh_world(|| {
            set_locale_code("fr");
            runtime_core::__flush_test_world();
            assert_eq!(&*current_locale_code(), "fr");
            // Idempotent same-value set.
            set_locale_code("fr");
            runtime_core::__flush_test_world();
            assert_eq!(&*current_locale_code(), "fr");
            set_locale_code("en");
            runtime_core::__flush_test_world();
            assert_eq!(&*current_locale_code(), "en");
        });
    }

    /// Regression: the locale signal used to be a thread-lifetime
    /// `OnceCell<Signal<_>>`, so a second world (an SSR request on a reused
    /// thread) inherited the FIRST world's slot — and reading a dropped
    /// world's slot panics. Each world must mint its own, seeded from the
    /// thread's current code so the SSG loop's per-locale choice carries over.
    #[test]
    fn regression_locale_signal_is_per_world_not_thread_cached() {
        runtime_core::__with_fresh_world(|| {
            set_locale_code("fr");
            runtime_core::__flush_test_world();
            assert_eq!(&*current_locale_code(), "fr");
        });
        // The first world is gone. A read here would have panicked on a
        // thread-cached handle; instead a fresh slot is minted from the seed.
        runtime_core::__with_fresh_world(|| {
            assert_eq!(&*current_locale_code(), "fr");
        });
        // …and back to a clean baseline for the next test on this thread.
        runtime_core::__with_fresh_world(|| {
            set_locale_code(INITIAL_CODE);
            runtime_core::__flush_test_world();
        });
    }

    /// The SSG shape: the locale is chosen BEFORE the render's world exists.
    /// It must not panic, and the world the render mints must start there.
    #[test]
    fn regression_set_locale_before_any_world_seeds_the_next_world() {
        set_locale_code("ja");
        runtime_core::__with_fresh_world(|| {
            assert_eq!(&*current_locale_code(), "ja");
        });
        set_locale_code(INITIAL_CODE);
    }
}
