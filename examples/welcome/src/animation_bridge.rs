//! Per-platform `AnimatedValue` → node-property bridge.
//!
//! Each `drive_*` helper subscribes an `AnimatedValue` so that every
//! per-frame value is written to its bound element's inline style on
//! the active backend. Subscriptions and a strong AV reference are
//! intentionally leaked — the welcome page is a one-shot intro and
//! its animations live for the page lifetime.

use framework_core::animation::{AnimProp, AnimatedValue};
use framework_core::{Ref, TextHandle, ViewHandle};

/// Subscribe `av` so every per-frame value writes to `view_ref`'s
/// node under `prop`. Until the ref is filled (the walker hasn't
/// mounted the view yet), the listener silently skips. After mount,
/// each frame writes one inline CSS property.
///
/// The returned `Subscription` is intentionally leaked — this is
/// the page's animation, not a per-component effect, so its
/// lifetime is the page lifetime.
pub fn drive_av(av: &AnimatedValue<f32>, view_ref: Ref<ViewHandle>, prop: AnimProp) {
    // Leak a strong ref to the AV so its `Inner` (and the animator
    // running inside it) outlive the timeline `after_ms` closures
    // that call `av.animate(...)`. The framework's animation system
    // holds only `Weak<Inner>` from the tick driver — if every
    // strong ref drops mid-tween (which happens when the only
    // outside handles are FnOnce closures that consume themselves
    // on fire), the animation unregisters and the AV freezes at
    // whatever value the closure wrote. The welcome page is a
    // one-shot intro, so a permanent leak is fine.
    std::mem::forget(av.clone());
    let sub = av.subscribe_and_apply(move |v, _vel| {
        let value = *v;
        view_ref.with(|handle| {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(node) = handle.as_any().downcast_ref::<web_sys::Node>() {
                    crate::web::set_animated_f32(node, prop, value);
                }
            }
            #[cfg(target_os = "ios")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_ios::IosNode>()
                {
                    backend_ios::set_animated_f32(node, prop, value);
                }
            }
            #[cfg(target_os = "android")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_android::AndroidNode>()
                {
                    backend_android::set_animated_f32(node, prop, value);
                }
            }
            // Sim / desktop preview — `idealyst run sim` builds for
            // the host OS (macOS / Linux / Windows) and runs the
            // wgpu render backend. `WgpuNode` is the type-alias
            // stored inside the `ViewHandle`'s erased
            // `Rc<dyn Any>`; downcasting + dispatching through the
            // crate-level `set_animated_f32` is the same shape as
            // the mobile branches above.
            #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<render_wgpu::WgpuNode>()
                {
                    render_wgpu::set_animated_f32(node, prop, value);
                }
            }
        });
    });
    std::mem::forget(sub);
}

/// Color-family counterpart of [`drive_av`], targeted at a Text
/// element. Subscribes a 4-tuple AnimatedValue (sRGB
/// `(r, g, b, a)` in `0..=1`) to a `Ref<TextHandle>` and writes
/// the channels through `set_animated_color` each frame.
///
/// On iOS this lands on the underlying `UILabel`'s `textColor`
/// (per the backend's per-widget routing in
/// `set_animated_color`), which is what makes the headline's
/// dark→light color transition visible through Act 2's wash. On
/// web the inline `color` write on the text element produces the
/// same visual effect.
pub fn drive_color_text_av(
    av: &AnimatedValue<(f32, f32, f32, f32)>,
    text_ref: Ref<TextHandle>,
    prop: AnimProp,
) {
    // See `drive_av` — same leak so the color AV's `Inner` survives a
    // running tween once the timeline closure that kicked it off has
    // consumed itself.
    std::mem::forget(av.clone());
    let sub = av.subscribe_and_apply(move |v, _vel| {
        let (r, g, b, a) = *v;
        text_ref.with(|handle| {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(node) = handle.as_any().downcast_ref::<web_sys::Node>() {
                    crate::web::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            #[cfg(target_os = "ios")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_ios::IosNode>()
                {
                    backend_ios::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            #[cfg(target_os = "android")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_android::AndroidNode>()
                {
                    backend_android::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            // Sim — see `drive_av` above for the rationale on this
            // branch.
            #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<render_wgpu::WgpuNode>()
                {
                    render_wgpu::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
        });
    });
    std::mem::forget(sub);
}

/// Per-stop counterpart of [`drive_color_text_av`]. Animates one
/// stop in the node's `background_gradient` via
/// `AnimProp::GradientStopColor(stop_idx)`. The view's other
/// gradient state (kind, offsets, other stops) survives — each
/// backend's per-frame writer mutates only the targeted stop.
pub fn drive_gradient_stop_av(
    av: &AnimatedValue<(f32, f32, f32, f32)>,
    view_ref: Ref<ViewHandle>,
    stop_idx: u8,
) {
    std::mem::forget(av.clone());
    let prop = AnimProp::GradientStopColor(stop_idx);
    let sub = av.subscribe_and_apply(move |v, _vel| {
        let (r, g, b, a) = *v;
        view_ref.with(|handle| {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(node) = handle.as_any().downcast_ref::<web_sys::Node>() {
                    crate::web::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            #[cfg(target_os = "ios")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_ios::IosNode>()
                {
                    backend_ios::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            #[cfg(target_os = "android")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_android::AndroidNode>()
                {
                    backend_android::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            // Sim — wgpu backend has no gradient pipeline yet, so
            // per-stop writes land in `AnimatedOverrides.gradient_stops`
            // for future use but don't visibly affect the rendered
            // node. See `WgpuBackend::set_animated_color`'s comment
            // on `GradientStopColor` for the plan.
            #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<render_wgpu::WgpuNode>()
                {
                    render_wgpu::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
        });
    });
    std::mem::forget(sub);
}
