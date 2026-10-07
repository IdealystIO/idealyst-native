//! # Per-world soft-keyboard state — the reactive `keyboard_inset()` source
//!
//! Layout avoids the keyboard through `keyboard_avoiding_view` (see
//! [`runtime_shared::keyboard`] for the per-platform mechanism). This
//! module is the other half: a reactive [`KeyboardInset`] so app code can
//! react to the keyboard itself — hide a tab bar while it is up, or run a
//! style transition in lockstep using the reported
//! [`KeyboardInset::transition`].
//!
//! Shape mirrors [`crate::viewport`]'s `ViewportCtx`:
//!
//! - The signal lives in a [`KeyboardCtx`] `provide`d into the owning
//!   world, created through [`runtime_world::unscoped`] so it is
//!   world-root-owned rather than adopted by whichever transient subtree
//!   first reads it.
//! - [`push`] is **handler-safe**: platform keyboard callbacks (a UIKit
//!   notification, an Android insets-animation callback, a DOM
//!   `geometrychange`) fire OUTSIDE `World::enter`. The ctx's signal
//!   handle is captured when the ctx is created; a push stages through
//!   it (equality-guarded, dead-world writes are silent no-ops). The
//!   backend then rides its own deduped flush (`schedule_flush`) — the
//!   same event-boundary glue as the viewport source.
//! - Pushes that arrive before any world has read `keyboard_inset()` are
//!   kept as the LATEST platform value, which a ctx seeds from when it is
//!   created — so a screen mounted with the keyboard already open reads
//!   the real inset, not `HIDDEN`.
//!
//! The signal carries the inset the keyboard is animating TO (one commit
//! per keyboard move), never per-frame values: per-frame tracking is the
//! backend's job (its layout pass), and author reactivity at 60 Hz would
//! re-run every subscriber on every frame of the animation.

use std::cell::{Cell, RefCell};

use runtime_shared::KeyboardInset;
use runtime_world::{inject, provide, signal, unscoped, ReadSignal, Signal};

/// The per-world keyboard context. Cloneable (context values must be) —
/// clones are the same handle.
#[derive(Clone)]
pub struct KeyboardCtx {
    inset: Signal<KeyboardInset>,
}

thread_local! {
    /// The platform's most recent report. Seeds every ctx created after
    /// it (see the module docs).
    static LATEST: Cell<KeyboardInset> = const { Cell::new(KeyboardInset::HIDDEN) };
    /// The signal [`push`] forwards to: the most recently created world
    /// ctx (the mounted app's). `None` until some world reads
    /// `keyboard_inset()` — until then the platform value only lands in
    /// [`LATEST`].
    static SINK: Cell<Option<Signal<KeyboardInset>>> = const { Cell::new(None) };
    /// The most recent ambient world's ctx — the handler fallback for
    /// reads outside `World::enter` (same contract as the viewport ctx).
    static LAST_CTX: RefCell<Option<KeyboardCtx>> = const { RefCell::new(None) };
}

/// The ambient world's keyboard context, created (and `provide`d) on
/// first use. Outside `World::enter` this returns the thread's last
/// ambient ctx; with none ever created it panics through the creation
/// path with the canonical outside-enter message.
pub fn keyboard_ctx() -> KeyboardCtx {
    if !runtime_world::is_entered() {
        if let Some(ctx) = LAST_CTX.with(|c| c.borrow().clone()) {
            return ctx;
        }
    }
    if let Some(ctx) = inject::<KeyboardCtx>() {
        LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
        return ctx;
    }
    let ctx = unscoped(|| KeyboardCtx { inset: signal(LATEST.with(Cell::get)) });
    unscoped(|| provide(ctx.clone()));
    SINK.with(|s| s.set(Some(ctx.inset)));
    LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
    ctx
}

/// The reactive keyboard inset — what `runtime_core::keyboard_inset()`
/// hands to author code. Read-only: only the platform writes it.
pub fn keyboard_inset() -> ReadSignal<KeyboardInset> {
    keyboard_ctx().inset.read_only()
}

/// Backend entry point: the platform reported a keyboard move (the inset
/// it is animating to, with that animation's timing). **Handler-safe** —
/// call it from the raw platform callback, then schedule the backend's
/// flush so the staged write commits. Equality-guarded: repeated reports
/// of the same inset don't wake subscribers.
pub fn push(inset: KeyboardInset) {
    let inset = KeyboardInset::new(inset.height, inset.transition);
    LATEST.with(|l| l.set(inset));
    if let Some(sig) = SINK.with(Cell::get) {
        sig.set(inset);
    }
}

/// The platform's most recent report, whether or not any world has read
/// `keyboard_inset()` yet. The runtime-server shells relay it to the
/// sidecar (`AppToDev::KeyboardChanged`), where author code runs.
pub fn latest() -> KeyboardInset {
    LATEST.with(Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::keyboard::WEB_KEYBOARD_ESTIMATE;
    use runtime_world::{effect, World};
    use std::rc::Rc;

    fn up(h: f32) -> KeyboardInset {
        KeyboardInset::new(h, WEB_KEYBOARD_ESTIMATE)
    }

    /// The platform callback fires outside `enter`; an author effect
    /// reading `keyboard_inset()` must re-fire on the flush, and must not
    /// re-fire for a repeated report of the same inset.
    #[test]
    fn push_from_platform_callback_refires_author_effect() {
        std::thread::spawn(|| {
            let world = World::new();
            let (runs, last) = world.enter(|| {
                let kb = keyboard_inset();
                let runs = Rc::new(Cell::new(0usize));
                let last = Rc::new(Cell::new(0.0f32));
                let (r, l) = (runs.clone(), last.clone());
                let _e = effect(move || {
                    l.set(kb.get().height);
                    r.set(r.get() + 1);
                });
                (runs, last)
            });
            world.flush();
            assert_eq!((runs.get(), last.get()), (1, 0.0));

            push(up(336.0));
            world.flush();
            assert_eq!((runs.get(), last.get()), (2, 336.0), "keyboard opened");

            push(up(336.0));
            world.flush();
            assert_eq!(runs.get(), 2, "same inset reported twice stays silent");

            push(KeyboardInset::HIDDEN);
            world.flush();
            assert_eq!((runs.get(), last.get()), (3, 0.0), "keyboard closed");
        })
        .join()
        .expect("keyboard push re-fires subscribers");
    }

    /// A report that lands before anything read `keyboard_inset()` (the
    /// keyboard was already open when a screen mounted) must seed the ctx
    /// instead of being lost.
    #[test]
    fn ctx_created_after_a_push_seeds_from_the_latest_report() {
        std::thread::spawn(|| {
            push(up(291.0));
            let world = World::new();
            let h = world.enter(|| keyboard_inset().get().height);
            assert_eq!(h, 291.0);
        })
        .join()
        .expect("late ctx seeds from the latest platform report");
    }

    /// Negative heights from a bogus platform report clamp to hidden.
    #[test]
    fn push_clamps_negative_heights() {
        std::thread::spawn(|| {
            let world = World::new();
            let kb = world.enter(keyboard_inset);
            push(KeyboardInset { height: -4.0, transition: WEB_KEYBOARD_ESTIMATE });
            world.flush();
            assert!(!world.enter(|| kb.get().is_visible()));
        })
        .join()
        .expect("negative inset clamps");
    }

    /// Reads from a handler (outside `enter`) resolve to the mounted
    /// world's ctx instead of panicking.
    #[test]
    fn keyboard_ctx_outside_enter_falls_back_to_last_ambient() {
        std::thread::spawn(|| {
            let world = World::new();
            world.enter(keyboard_ctx);
            push(up(120.0));
            world.flush();
            assert_eq!(keyboard_inset().peek().height, 120.0);
        })
        .join()
        .expect("handler-side read falls back to the last ambient ctx");
    }
}
