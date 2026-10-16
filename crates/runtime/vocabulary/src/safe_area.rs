//! # Per-world safe-area state — the reactive `safe_area_insets()` source
//!
//! The platform's safe-area insets (status bar, home indicator, notch /
//! dynamic island, rounded-corner sides) as a world signal. Two readers:
//!
//! - author code, through glue's `safe_area_insets()`;
//! - the vocabulary itself: every container mounted with `safe_area`
//!   sides (`view`, `keyboard_avoiding_view`, `scroll_view`, the
//!   virtualizer) re-applies its padding / inset from an effect that
//!   reads this signal, so the padding follows rotation, a status bar
//!   showing or hiding, and — the case that made this module necessary —
//!   the FIRST real report.
//!
//! That first report is why a one-shot apply at mount is not enough. A
//! native host mounts the tree before its root view is in a window (iOS
//! mounts from `viewDidLoad`), and a view outside a window has no safe
//! area: UIKit reports zero insets until the view is attached, then
//! delivers the real ones through `safeAreaInsetsDidChange`. A padding
//! applied only at mount therefore pads by nothing, for the life of the
//! app (CrewForge's kiosk drew its header under the iPad status bar).
//!
//! Shape mirrors [`crate::keyboard`] (and [`crate::viewport`]):
//!
//! - The signal lives in a [`SafeAreaCtx`] `provide`d into the owning
//!   world, created through [`runtime_world::unscoped`] so it is
//!   world-root-owned rather than adopted by whichever transient subtree
//!   first reads it.
//! - [`push`] is **handler-safe**: platform inset callbacks (UIKit's
//!   `safeAreaInsetsDidChange`, a wgpu host's device skin) fire OUTSIDE
//!   `World::enter`. The ctx's signal handle is captured when the ctx is
//!   created; a push stages through it (equality-guarded, dead-world
//!   writes are silent no-ops). The backend then rides its own deduped
//!   flush (`schedule_flush`).
//! - Pushes that arrive before any world has read `safe_area_insets()`
//!   are kept as the LATEST platform value, which a ctx seeds from when
//!   it is created.
//!
//! Backends that also serve the runtime-server dev client keep calling
//! [`runtime_shared::set_safe_area_insets`]: the dev client resolves
//! wire safe-area opt-ins against that value on its own effect.

use std::cell::{Cell, RefCell};

use runtime_shared::EdgeInsets;
use runtime_world::{inject, provide, signal, unscoped, ReadSignal, Signal};

/// The per-world safe-area context. Cloneable (context values must be) —
/// clones are the same handle.
#[derive(Clone)]
pub struct SafeAreaCtx {
    insets: Signal<EdgeInsets>,
}

runtime_tls::thread_local! {
    /// The platform's most recent report. Seeds every ctx created after
    /// it (see the module docs).
    static LATEST: Cell<EdgeInsets> = const { Cell::new(EdgeInsets::ZERO) };
    /// The signal [`push`] forwards to: the most recently created world
    /// ctx (the mounted app's). `None` until some world reads
    /// `safe_area_insets()` — until then the platform value only lands in
    /// [`LATEST`].
    static SINK: Cell<Option<Signal<EdgeInsets>>> = const { Cell::new(None) };
    /// The most recent ambient world's ctx — the handler fallback for
    /// reads outside `World::enter` (same contract as the keyboard ctx).
    static LAST_CTX: RefCell<Option<SafeAreaCtx>> = const { RefCell::new(None) };
}

/// The ambient world's safe-area context, created (and `provide`d) on
/// first use. Outside `World::enter` this returns the thread's last
/// ambient ctx; with none ever created it panics through the creation
/// path with the canonical outside-enter message.
pub fn safe_area_ctx() -> SafeAreaCtx {
    if !runtime_world::is_entered() {
        if let Some(ctx) = LAST_CTX.with(|c| c.borrow().clone()) {
            return ctx;
        }
    }
    if let Some(ctx) = inject::<SafeAreaCtx>() {
        LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
        return ctx;
    }
    let ctx = unscoped(|| SafeAreaCtx { insets: signal(LATEST.with(Cell::get)) });
    unscoped(|| provide(ctx.clone()));
    SINK.with(|s| s.set(Some(ctx.insets)));
    LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
    ctx
}

/// The reactive safe-area insets — what `runtime_core::safe_area_insets()`
/// hands to author code. Read-only: only the platform writes it.
/// `EdgeInsets::ZERO` on platforms with no safe area, and on a native
/// host until its root view is attached to a window.
pub fn safe_area_insets() -> ReadSignal<EdgeInsets> {
    safe_area_ctx().insets.read_only()
}

/// Backend entry point: the platform reported new safe-area insets.
/// **Handler-safe** — call it from the raw platform callback, then
/// schedule the backend's flush so the staged write commits.
/// Equality-guarded: repeated reports of the same insets don't wake
/// subscribers. Negative sides (a bogus report) clamp to zero.
pub fn push(insets: EdgeInsets) {
    let insets = EdgeInsets {
        top: insets.top.max(0.0),
        right: insets.right.max(0.0),
        bottom: insets.bottom.max(0.0),
        left: insets.left.max(0.0),
    };
    LATEST.with(|l| l.set(insets));
    if let Some(sig) = SINK.with(Cell::get) {
        sig.set(insets);
    }
}

/// The platform's most recent report, whether or not any world has read
/// `safe_area_insets()` yet.
pub fn latest() -> EdgeInsets {
    LATEST.with(Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_world::{effect, World};
    use std::rc::Rc;

    fn top(t: f32) -> EdgeInsets {
        EdgeInsets { top: t, ..EdgeInsets::ZERO }
    }

    /// The platform callback fires outside `enter`; an author effect
    /// reading `safe_area_insets()` must re-fire on the flush, and must not
    /// re-fire for a repeated report of the same insets.
    #[test]
    fn push_from_platform_callback_refires_author_effect() {
        std::thread::spawn(|| {
            let world = World::new();
            let (runs, last) = world.enter(|| {
                let insets = safe_area_insets();
                let runs = Rc::new(Cell::new(0usize));
                let last = Rc::new(Cell::new(0.0f32));
                let (r, l) = (runs.clone(), last.clone());
                let _e = effect(move || {
                    l.set(insets.get().top);
                    r.set(r.get() + 1);
                });
                (runs, last)
            });
            world.flush();
            assert_eq!((runs.get(), last.get()), (1, 0.0));

            push(top(20.0));
            world.flush();
            assert_eq!((runs.get(), last.get()), (2, 20.0), "status bar inset arrived");

            push(top(20.0));
            world.flush();
            assert_eq!(runs.get(), 2, "same insets reported twice stay silent");
        })
        .join()
        .expect("safe-area push re-fires subscribers");
    }

    /// A report that lands before anything read `safe_area_insets()` must
    /// seed the ctx instead of being lost.
    #[test]
    fn ctx_created_after_a_push_seeds_from_the_latest_report() {
        std::thread::spawn(|| {
            push(top(47.0));
            let world = World::new();
            let t = world.enter(|| safe_area_insets().get().top);
            assert_eq!(t, 47.0);
            assert_eq!(latest().top, 47.0);
        })
        .join()
        .expect("late ctx seeds from the latest platform report");
    }

    /// Negative sides from a bogus platform report clamp to zero.
    #[test]
    fn push_clamps_negative_sides() {
        std::thread::spawn(|| {
            let world = World::new();
            let insets = world.enter(safe_area_insets);
            push(EdgeInsets { top: -3.0, right: 0.0, bottom: 34.0, left: -1.0 });
            world.flush();
            let got = insets.peek();
            assert_eq!((got.top, got.bottom, got.left), (0.0, 34.0, 0.0));
        })
        .join()
        .expect("negative insets clamp");
    }

    /// Reads from a handler (outside `enter`) resolve to the mounted
    /// world's ctx instead of panicking.
    #[test]
    fn safe_area_ctx_outside_enter_falls_back_to_last_ambient() {
        std::thread::spawn(|| {
            let world = World::new();
            world.enter(safe_area_ctx);
            push(top(24.0));
            world.flush();
            assert_eq!(safe_area_insets().peek().top, 24.0);
        })
        .join()
        .expect("handler-side read falls back to the last ambient ctx");
    }
}
