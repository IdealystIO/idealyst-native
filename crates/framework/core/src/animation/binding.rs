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
//! ## Leak semantics
//!
//! Both the cloned `AnimatedValue` handle AND the returned
//! `Subscription` are `mem::forget`'d. The framework's animation
//! clock holds only `Weak<Inner>` references — if every strong
//! handle drops mid-animation (which happens when the only outside
//! references are `FnOnce` closures from a `timeline!` that
//! consume themselves on fire), the AV's `Inner` is dropped, the
//! tick deregisters, and the value freezes at whatever the
//! closure last wrote. The leak is fine for the welcome use case
//! (one-shot intro, lives for the page lifetime). If a future
//! use case needs scope-bounded bindings, add `bind_in_scope` that
//! returns a `Subscription` for the caller to hold.

use crate::animation::value::{AnimatedValue, Subscription};
use crate::animation::AnimProp;
use crate::{Ref, TextHandle, ViewHandle};

impl AnimatedValue<f32> {
    /// Subscribe a scalar animation property to `target`. Every
    /// per-frame value gets written to the bound view's
    /// `prop` (one of `AnimProp::Opacity`, `Scale`, `ScaleX/Y`,
    /// `TranslateX/Y`, `RotateZ`) on whichever backend is active.
    ///
    /// Until the ref is filled (the build walker hasn't mounted the
    /// view yet), writes silently skip; once mounted, every frame
    /// applies. The subscription + a strong AV handle are leaked
    /// for the page lifetime — see the module docs for rationale.
    pub fn bind(&self, target: Ref<ViewHandle>, prop: AnimProp) {
        std::mem::forget(self.clone());
        let sub: Subscription<f32> = self.subscribe_and_apply(move |v, _vel| {
            let value = *v;
            target.with(|handle| handle.set_animated_f32(prop, value));
        });
        std::mem::forget(sub);
    }
}

impl AnimatedValue<(f32, f32, f32, f32)> {
    /// Subscribe a color animation property to a `Ref<ViewHandle>`.
    /// `prop` is typically `AnimProp::BackgroundColor` or
    /// `AnimProp::GradientStopColor(idx)`. The 4-tuple is sRGB
    /// `(r, g, b, a)` with all channels in `0..=1`.
    pub fn bind_color(&self, target: Ref<ViewHandle>, prop: AnimProp) {
        std::mem::forget(self.clone());
        let sub: Subscription<(f32, f32, f32, f32)> =
            self.subscribe_and_apply(move |v, _vel| {
                let (r, g, b, a) = *v;
                target.with(|handle| handle.set_animated_color(prop, [r, g, b, a]));
            });
        std::mem::forget(sub);
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
        std::mem::forget(self.clone());
        let sub: Subscription<(f32, f32, f32, f32)> =
            self.subscribe_and_apply(move |v, _vel| {
                let (r, g, b, a) = *v;
                target.with(|handle| handle.set_animated_color(prop, [r, g, b, a]));
            });
        std::mem::forget(sub);
    }
}
