//! `AnimatedValue::bind` — wire a per-frame animated value to a
//! mounted primitive without any per-platform glue code in author
//! land.
//!
//! Before this module existed, authors had to write a `drive_av`
//! helper that subscribed to the value, downcast the type-erased
//! `Ref<H>` node to each backend's concrete node type via
//! `#[cfg(target_arch = "wasm32")]` / `#[cfg(target_os = "ios")]` /
//! `#[cfg(target_os = "android")]` blocks, and dispatched to that
//! backend's `set_animated_*` free function. That's plumbing the
//! framework should own — and now does. The flow:
//!
//! 1. Author: `av.bind(view_ref, AnimProp::Opacity)`.
//! 2. Framework subscribes to `av`. On each fire it calls
//!    [`ViewHandle::set_animated_f32`](crate::ViewHandle::set_animated_f32),
//!    which delegates to [`ViewOps::set_animated_f32`](crate::ViewOps::set_animated_f32).
//! 3. Each backend's `ViewOps` impl downcasts the type-erased
//!    `&dyn Any` to its native node and calls the backend's
//!    existing per-platform `set_animated_*` writer. No `cfg` block
//!    in author code.
//!
//! ## Lifetime semantics
//!
//! Both the cloned `AnimatedValue` handle AND the returned
//! `Subscription` are **anchored to the active reactive scope** —
//! they drop when the scope drops (which on runtime-server hot-patch
//! rerenders happens once per save). On scope drop:
//!
//! - The `Subscription` drops first → the listener is removed from
//!   the AV's `Inner.listeners` list. Without this, every
//!   rerender's `bind()` would pile a new listener on top of the
//!   previous ones; stale listeners targeting freed `Ref` slots
//!   that have since been **recycled to a different handle type**
//!   would then panic during dispatch
//!   (`internal: ref handle type mismatch`), aborting the
//!   animation tick on every frame. Anchoring the subscription is
//!   what keeps post-rerender animation continuous.
//! - The strong AV clone drops second. For `session::animated`
//!   AVs the registry holds another strong reference so the AV
//!   stays alive; for plain `animated!()` AVs the local
//!   construction site is also gone by this point, so the AV's
//!   `Inner` drops, the animation clock's `Weak` tick fails to
//!   upgrade, and the per-frame work cleanly deregisters. Either
//!   way, the visible state is consistent with what an unbinding
//!   would produce.
//!
//! Outside any active scope `bind()` is effectively a no-op — the
//! anchor closure is dropped immediately, taking the
//! `Subscription` and the AV clone with it. Callers that need
//! a binding outside a scope should arrange a scope around the
//! binding (typical: bind during render, which always runs inside
//! `mount()`'s root scope).

use crate::animation::value::{AnimatedValue, Subscription};
use crate::animation::AnimProp;
use crate::reactive::on_cleanup;
use crate::{Ref, TextHandle, ViewHandle};

impl AnimatedValue<f32> {
    /// Subscribe a scalar animation property to `target`. Every
    /// per-frame value gets written to the bound view's
    /// `prop` (one of `AnimProp::Opacity`, `Scale`, `ScaleX/Y`,
    /// `TranslateX/Y`, `RotateZ`) on whichever backend is active.
    ///
    /// Until the ref is filled (the build walker hasn't mounted the
    /// view yet), writes silently skip; once mounted, every frame
    /// applies. The subscription + a strong AV handle are anchored
    /// to the active reactive scope — see the module docs for
    /// lifetime semantics.
    pub fn bind(&self, target: Ref<ViewHandle>, prop: AnimProp) {
        let av_clone = self.clone();
        let sub: Subscription<f32> = self.subscribe_and_apply(move |v, _vel| {
            let value = *v;
            target.with(|handle| handle.set_animated_f32(prop, value));
        });
        on_cleanup(move || {
            drop(sub);
            drop(av_clone);
        });
        // Re-apply the current value once, just after mount. See
        // [`reapply_after_mount_note`].
        let av = self.clone();
        crate::scheduling::after_ms_scoped(0, move || {
            target.with(|handle| handle.set_animated_f32(prop, av.get()));
        });
    }
}

/// ## `reapply_after_mount_note`
///
/// Each `bind*` schedules a 0-delay scoped re-apply of its CURRENT value after
/// `subscribe_and_apply`. Why: `subscribe_and_apply` writes the starting value
/// immediately at `bind()` time — but that's during component construction, when
/// the target `Ref` isn't filled yet, so the write silently no-ops. The bound
/// value then first reaches the backend on the *next* change. For a STATIC value
/// (or a `SpringTo` whose start already equals its target, which never ticks)
/// there is no next change, so the value would never be applied at all — e.g.
/// macOS leaves a transform-positioned view stuck at frame (0,0) until something
/// else moves it; web/iOS happen to land it but the gap is the same.
///
/// The deferred task runs once the build walker has filled the ref (a 0-delay
/// task fires on the next loop turn). It WRITES the value via `set_animated_*`
/// WITHOUT going through `AnimatedValue::set`, so an in-flight animation's
/// animator is untouched, and it's idempotent where the first apply already
/// landed. `after_ms_scoped` anchors it to the active scope: a hot-patch rerender
/// that recycles the ref slot before it fires cancels it (its cancelled-flag
/// guard), so it never writes through a recycled handle.
const _REAPPLY_AFTER_MOUNT_NOTE: () = ();

impl AnimatedValue<(f32, f32, f32, f32)> {
    /// Subscribe a color animation property to a `Ref<ViewHandle>`.
    /// `prop` is typically `AnimProp::BackgroundColor` or
    /// `AnimProp::GradientStopColor(idx)`. The 4-tuple is sRGB
    /// `(r, g, b, a)` with all channels in `0..=1`.
    pub fn bind_color(&self, target: Ref<ViewHandle>, prop: AnimProp) {
        let av_clone = self.clone();
        let sub: Subscription<(f32, f32, f32, f32)> =
            self.subscribe_and_apply(move |v, _vel| {
                let (r, g, b, a) = *v;
                target.with(|handle| handle.set_animated_color(prop, [r, g, b, a]));
            });
        on_cleanup(move || {
            drop(sub);
            drop(av_clone);
        });
        // Re-apply the current value after mount — see `reapply_after_mount_note`.
        let av = self.clone();
        crate::scheduling::after_ms_scoped(0, move || {
            let (r, g, b, a) = av.get();
            target.with(|handle| handle.set_animated_color(prop, [r, g, b, a]));
        });
    }

    /// Convenience for the common case of animating one gradient
    /// stop. Equivalent to
    /// `bind_color(target, AnimProp::GradientStopColor(stop_idx))`,
    /// but reads as the call site's intent.
    pub fn bind_gradient_stop(&self, target: Ref<ViewHandle>, stop_idx: u8) {
        self.bind_color(target, AnimProp::GradientStopColor(stop_idx));
    }

    /// Subscribe a color animation property to a `Ref<TextHandle>`.
    /// Routes through `TextOps::set_animated_color`, which on each
    /// backend hits the text-bearing widget's own color setter
    /// (`UILabel.textColor` on iOS, `TextView.setTextColor` on
    /// Android, inline `style.color` on web) — the text element's
    /// color doesn't piggyback on the parent view's animated
    /// color, so the framework dispatches through the text-handle
    /// ops here rather than the view-handle ops above.
    ///
    /// Typically used with `AnimProp::ForegroundColor`.
    pub fn bind_text_color(&self, target: Ref<TextHandle>, prop: AnimProp) {
        let av_clone = self.clone();
        let sub: Subscription<(f32, f32, f32, f32)> =
            self.subscribe_and_apply(move |v, _vel| {
                let (r, g, b, a) = *v;
                target.with(|handle| handle.set_animated_color(prop, [r, g, b, a]));
            });
        on_cleanup(move || {
            drop(sub);
            drop(av_clone);
        });
        // Re-apply the current value after mount — see `reapply_after_mount_note`.
        let av = self.clone();
        crate::scheduling::after_ms_scoped(0, move || {
            let (r, g, b, a) = av.get();
            target.with(|handle| handle.set_animated_color(prop, [r, g, b, a]));
        });
    }
}

#[cfg(test)]
mod tests {
    //! Regression coverage for hot-patch rerender + `Ref` slot
    //! recycling. Pre-fix, `bind()` `mem::forget`'d its
    //! `Subscription` so the AV's listener list grew by one per
    //! rerender. Slot recycling is LIFO, so on the second mount
    //! the slot that used to back a `Ref<TextHandle>` ends up
    //! holding a `Ref<ViewHandle>` (or vice versa). The
    //! still-leaked subscription captured the old `Ref<H>`, so its
    //! per-frame `target.with(|h: &H| ...)` would panic with
    //! `internal: ref handle type mismatch` on the first animator
    //! tick after rerender — aborting the entire animation tick
    //! and silently halting every raf-driven animation in the
    //! scene. The fix anchors the `Subscription` (and the AV's
    //! strong handle) to the active scope via [`on_cleanup`] so
    //! old subscriptions are gone before the new mount's bind()
    //! runs.
    use super::*;
    use crate::animation::AnimatedValue;
    use crate::reactive::{with_scope, Scope};
    use crate::{Ref, TextHandle, TextOps, ViewHandle, ViewOps};
    use std::any::Any;
    use std::rc::Rc;

    struct StubViewOps;
    impl ViewOps for StubViewOps {}
    static STUB_VIEW_OPS: StubViewOps = StubViewOps;

    struct StubTextOps;
    impl TextOps for StubTextOps {}
    static STUB_TEXT_OPS: StubTextOps = StubTextOps;

    /// Mimics the welcome's scope pattern: a single root scope that
    /// owns refs, AV bindings, and (in real life) the `effect!`.
    /// `body` runs inside the scope and returns nothing it intends
    /// to outlive the scope drop.
    fn in_scope(body: impl FnOnce()) {
        let mut scope = Scope::new();
        with_scope(&mut scope, body);
        // Scope::drop fires cleanups + frees ref slots (pushing
        // them onto the free list LIFO).
    }

    #[test]
    fn rerender_does_not_panic_from_recycled_text_slot() {
        // First mount: a Ref<TextHandle> is bound to an AV via
        // bind_text_color. The AV is keyed-equivalent (we hold a
        // strong outer handle for the whole test) so the second
        // mount reuses it.
        let av: AnimatedValue<(f32, f32, f32, f32)> =
            AnimatedValue::new((0.0, 0.0, 0.0, 1.0));

        // ---- First mount: bind a Ref<TextHandle> in scope 1.
        in_scope(|| {
            let text_ref: Ref<TextHandle> = Ref::<TextHandle>::new();
            // Fill so `target.with(|h| ...)` actually runs the
            // closure. The handle's set_animated_color is a
            // trait-default no-op (StubTextOps doesn't override it),
            // so the dispatch reaches it without producing any side
            // effect — which is exactly what we want to verify
            // doesn't panic.
            let node: Rc<dyn Any> = Rc::new(0u32);
            text_ref.fill(TextHandle::new(node, &STUB_TEXT_OPS));
            av.bind_text_color(text_ref, AnimProp::ForegroundColor);
            // Trigger the listener once inside this scope so we
            // know dispatch works during the first lifetime.
            av.set((0.5, 0.5, 0.5, 1.0));
        });
        // Scope 1 drops here. Pre-fix, the subscription was
        // mem::forget'd → still in av.listeners. Post-fix, the
        // on_cleanup anchor dropped it → listener removed.

        // ---- Second mount: a Ref<ViewHandle> grabs the recycled
        // slot. The pre-fix bug: the OLD bind's subscription is
        // still in av.listeners targeting `Ref<TextHandle>{id: X}`,
        // where slot X now holds a ViewHandle. Calling
        // `target.with::<TextHandle>` downcasts the ViewHandle as
        // TextHandle → panics.
        in_scope(|| {
            let view_ref: Ref<ViewHandle> = Ref::<ViewHandle>::new();
            let node: Rc<dyn Any> = Rc::new(0u32);
            view_ref.fill(ViewHandle::new(node, &STUB_VIEW_OPS));
            // No bind here — we just want to fire the AV and see
            // what happens to any leftover listeners from scope 1.
            // Pre-fix: panic. Post-fix: clean.
            av.set((0.25, 0.25, 0.25, 1.0));
        });

        // If we got here without panicking, the fix is working.
    }

    #[test]
    fn rerender_clears_bound_listener_so_old_target_stops_receiving_writes() {
        // Wrap the standard `ViewOps` so we can count
        // set_animated_f32 calls per backing handle. Use the count
        // as a proxy for "which subscription is still firing."
        // `AtomicU32` instead of `Cell` so the ops can live in a
        // `static` (ViewHandle wants `&'static dyn ViewOps`).
        struct CountingViewOps {
            counter: std::sync::atomic::AtomicU32,
        }
        impl ViewOps for CountingViewOps {
            fn set_animated_f32(
                &self,
                _node: &dyn Any,
                _prop: AnimProp,
                _value: f32,
            ) {
                self.counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        impl CountingViewOps {
            fn count(&self) -> u32 {
                self.counter.load(std::sync::atomic::Ordering::Relaxed)
            }
        }

        let av: AnimatedValue<f32> = AnimatedValue::new(0.0);

        // First mount: bind to handle backed by ops `A`.
        static OPS_A: CountingViewOps = CountingViewOps {
            counter: std::sync::atomic::AtomicU32::new(0),
        };
        in_scope(|| {
            let r: Ref<ViewHandle> = Ref::<ViewHandle>::new();
            let node: Rc<dyn Any> = Rc::new(0u32);
            r.fill(ViewHandle::new(node, &OPS_A));
            av.bind(r, AnimProp::Opacity);
            // subscribe_and_apply fired once during bind → A=1.
            av.set(0.5); // → A=2.
        });
        // First mount dropped. Pre-fix, the bind's subscription
        // would still be in av.listeners and would route writes
        // to handle A even after the scope dropped. Post-fix, the
        // on_cleanup anchor removes the subscription, so handle A
        // sees no further writes.
        let a_after_drop = OPS_A.count();
        assert!(
            a_after_drop >= 2,
            "first scope: bind+set should have written to A at least twice (got {})",
            a_after_drop
        );

        // Outside any scope, fire the AV. With the anchor in place,
        // there are no listeners left, so A should not increment.
        av.set(0.6);
        assert_eq!(
            OPS_A.count(),
            a_after_drop,
            "scope-anchored subscription should be cleared on scope drop; \
             writes after the drop must not reach the old handle (A counter \
             went {} → {})",
            a_after_drop,
            OPS_A.count()
        );

        // Second mount: bind to handle backed by ops `B`. Verify
        // writes route to B only, not A.
        static OPS_B: CountingViewOps = CountingViewOps {
            counter: std::sync::atomic::AtomicU32::new(0),
        };
        in_scope(|| {
            let r: Ref<ViewHandle> = Ref::<ViewHandle>::new();
            let node: Rc<dyn Any> = Rc::new(0u32);
            r.fill(ViewHandle::new(node, &OPS_B));
            av.bind(r, AnimProp::Opacity);
            av.set(0.7);
        });
        let b_count = OPS_B.count();
        assert!(
            b_count >= 2,
            "second scope's bind+set should have written to B at least twice (got {})",
            b_count
        );
        assert_eq!(
            OPS_A.count(),
            a_after_drop,
            "second scope's writes must not bleed into the previous scope's \
             handle (A went {} → {})",
            a_after_drop,
            OPS_A.count()
        );
    }

    #[test]
    fn bind_reapplies_value_after_mount_so_static_value_lands() {
        // Regression for: a STATIC bound value (no animation, no later change)
        // never reached the backend, because `subscribe_and_apply`'s one-time
        // write happens at bind() time when the ref is still empty. On macOS that
        // left transform-positioned views stuck at (0,0) until an interaction.
        // The fix schedules a re-apply after mount (see `reapply_after_mount_note`);
        // it writes a SECOND time so the value lands even with no change.
        //
        // We observe it inline via the no-scheduler SYNCHRONOUS fallback
        // (`after_ms` runs `f()` immediately off-web). `install_scheduler` is a
        // process-wide OnceLock shared with other tests' schedulers — installing
        // a queueing one here would hijack theirs — so if some other test already
        // installed one (which routes our `after_ms_scoped` to it instead of the
        // sync fallback), skip rather than assert. Same posture as
        // `no_scheduler_on_web_panics`.
        if crate::scheduling::is_scheduler_installed() {
            return;
        }
        // Pin platform non-web so the no-scheduler fallback is synchronous, not a
        // panic (mirrors the scheduling fallback tests).
        crate::backend::install_current_platform(crate::Platform::Ios);

        struct CountOps {
            n: std::sync::atomic::AtomicU32,
        }
        impl ViewOps for CountOps {
            fn set_animated_f32(&self, _node: &dyn Any, _prop: AnimProp, _value: f32) {
                self.n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        static OPS: CountOps = CountOps {
            n: std::sync::atomic::AtomicU32::new(0),
        };

        let av: AnimatedValue<f32> = AnimatedValue::new(0.42);
        in_scope(|| {
            let r: Ref<ViewHandle> = Ref::<ViewHandle>::new();
            let node: Rc<dyn Any> = Rc::new(0u32);
            r.fill(ViewHandle::new(node, &OPS));
            // Bind ONLY — no `av.set(...)`, no animation. Pre-fix this was a
            // single write (subscribe_and_apply); post-fix the re-apply adds a
            // second one, which is what makes a static value actually land.
            av.bind(r, AnimProp::Opacity);
        });
        assert!(
            OPS.n.load(std::sync::atomic::Ordering::Relaxed) >= 2,
            "bind of a static value must apply it AND re-apply after mount (got {} writes)",
            OPS.n.load(std::sync::atomic::Ordering::Relaxed),
        );
    }
}
