//! `lazy-demo` — exercises `lazy! { … }` end-to-end, including a
//! deliberately-heavy variant that pulls in the same wgpu / welcome
//! stack the website's Simulator uses.
//!
//! Two lazy blocks:
//!
//! - **light**: pure `View { Text … }`. Tiny chunk. Sanity check
//!   that the pipeline (cargo + wasm-bindgen + wasm-split + wasm-opt)
//!   produces something the browser can load.
//! - **heavy**: wraps a wgpu Simulator surface around `welcome::app`.
//!   Same dep profile as the website's hero simulator, but without
//!   `idea-ui` in the way. Measures whether our patched wasm-split
//!   actually extracts the wgpu transitive closure (the website's
//!   real ask).

use std::rc::Rc;

use runtime_core::primitives::lazy::{lazy_split, LazyState};
use runtime_core::{lazy, signal, ui, IntoPrimitive, Primitive};

use host_web::{DeviceProfile, Painter};
use runtime_core::primitives::graphics::{OnReadyEvent, OnResizeEvent};

pub fn app() -> Primitive {
    let state = signal!(LazyState::Loading);
    let state_for_label = state.clone();
    let status = move || match state_for_label.get() {
        LazyState::Loading => "status: Loading chunk...".to_string(),
        LazyState::Loaded => "status: Loaded; mounting...".to_string(),
        LazyState::Rendered => "status: Rendered ✓".to_string(),
        LazyState::Error(e) => format!("status: Error — {e}"),
    };

    // LIGHT lazy block — same as before, sanity check.
    let light: Primitive = lazy! {
        ui! {
            View {
                Text { "[light chunk] Hello from the simple lazy block!" }
            }
        }
    }
    .on_state(move |s| state.set(s))
    .placeholder(|| {
        ui! { View { Text { "(loading light chunk...)" } } }.into_primitive()
    })
    .into_primitive();

    // HEAVY lazy block — wgpu Simulator + welcome::app. Real
    // stress test for the splitter; this is where the website's
    // main bundle should also shrink once everything works.
    let heavy_state = signal!(LazyState::Loading);
    let heavy_state_for_label = heavy_state.clone();
    let heavy_status = move || match heavy_state_for_label.get() {
        LazyState::Loading => "heavy: Loading chunk...".to_string(),
        LazyState::Loaded => "heavy: Loaded; mounting...".to_string(),
        LazyState::Rendered => "heavy: Rendered ✓".to_string(),
        LazyState::Error(e) => format!("heavy: Error — {e}"),
    };

    let heavy: Primitive = lazy! {
        // Full simulator-with-toggle, mirroring the website's
        // hero_simulator shape. Tab UI built from framework
        // primitives (button/pressable) so we don't need idea-ui.
        // State lives inside the lazy block (v1 lazy! has no
        // captures across the boundary).
        use runtime_core::{button, switch};
        use std::rc::Rc;

        let active: runtime_core::Signal<usize> = runtime_core::signal!(0_usize);
        let active_for_ios = active.clone();
        let active_for_android = active.clone();

        let ios_btn = button("iOS", move || active_for_ios.set(0));
        let android_btn = button("Android", move || active_for_android.set(1));

        let tab_strip = ui! {
            View {
                ios_btn
                android_btn
            }
        };

        let dynamic_sim = switch(
            move || active.get(),
            |&idx| build_simulator(idx),
        );

        let label = move || match active.get() {
            1 => "(Android skin)".to_string(),
            _ => "(iOS skin)".to_string(),
        };

        ui! {
            View {
                Text { "Heavy chunk content (wgpu + welcome):" }
                tab_strip
                Text { label }
                dynamic_sim
            }
        }
    }
    .on_state(move |s| heavy_state.set(s))
    .placeholder(|| {
        ui! { View { Text { "(loading heavy chunk — wgpu/welcome...)" } } }
            .into_primitive()
    })
    .into_primitive();

    ui! {
        View {
            Text { "Lazy Primitive Demo" }
            Text { "The status lines reflect each chunk's lifecycle:" }
            Text { status }
            light
            Text { heavy_status }
            heavy
        }
    }
}

// The actual wgpu Simulator. Lives at module scope so the `lazy!`
// block's body (which is a `fn`, not a closure) can call it without
// capturing.
//
// `skin_idx`: 0 = iOS, 1 = Android. Matches the website's hero
// simulator switch keying.
fn build_simulator(skin_idx: usize) -> Primitive {
    use runtime_core::driver::spawn_async;
    use runtime_core::{view, IntoPrimitive, Length, StyleRules, StyleSheet};

    let slot: Rc<std::cell::RefCell<Option<host_web::WebHostHandle>>> =
        Rc::new(std::cell::RefCell::new(None));
    let slot_ready = slot.clone();
    let slot_resize = slot.clone();
    let slot_lost = slot;

    let painter: Rc<dyn Painter> = match skin_idx {
        1 => Rc::new(android_sim::AndroidSim::new()),
        _ => Rc::new(ios_sim::IosSim::new()),
    };
    let profile = DeviceProfile {
        logical_size: (390, 844),
        position: None,
        title: "Lazy Demo Simulator".to_string(),
        color_scheme: runtime_core::ColorScheme::Light,
    };

    // Fixed-dimension wrapper. The web Graphics primitive forces
    // `width: 100%; height: 100%` INLINE on its canvas, so the
    // canvas's painted size comes from this wrapper. 0×0 means no
    // wgpu surface ever paints — the on_ready event still fires
    // but with a degenerate size, and the simulator stays blank.
    let preview_w = 300.0_f32;
    let preview_h = preview_w * (profile.logical_size.1 as f32 / profile.logical_size.0 as f32);
    let wrapper_style = std::rc::Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Length::Px(preview_w).into()),
        height: Some(Length::Px(preview_h).into()),
        ..Default::default()
    }));

    let graphics = runtime_core::primitives::graphics::graphics(move |event: OnReadyEvent| {
        let painter = painter.clone();
        let profile = profile.clone();
        let surface = event.surface;
        let size = event.size;
        let slot = slot_ready.clone();
        spawn_async(async move {
            let build_ui = || welcome::app();
            match host_web::mount(surface, size, profile, painter, build_ui).await {
                Ok(handle) => {
                    *slot.borrow_mut() = Some(handle);
                }
                Err(e) => {
                    eprintln!("lazy-demo: host-web mount failed: {e}");
                }
            }
        });
    })
    .on_resize(move |event: OnResizeEvent| {
        if let Some(handle) = slot_resize.borrow().as_ref() {
            handle.resize(event.size);
        }
    })
    .on_lost(move || {
        let stale = slot_lost.borrow_mut().take();
        drop(stale);
    });

    // Wrap in a fixed-size View so the canvas has non-zero
    // dimensions to paint into.
    view(vec![graphics.into_primitive()])
        .with_style(wrapper_style)
        .into_primitive()
}

// Silence the unused import (lazy_split is the macro's expansion
// target — surfaced here for completeness even though author code
// almost never touches it directly).
#[allow(dead_code)]
fn _refer_lazy_split() {
    let _ = lazy_split::<fn() -> _>;
}

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(_backend: &mut backend_web::WebBackend) {}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_ios_mobile::IosBackend) {}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_android_mobile::AndroidBackend) {}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn register_extensions(_backend: &mut backend_terminal::TerminalBackend) {}
