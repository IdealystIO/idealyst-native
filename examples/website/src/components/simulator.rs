//! `Simulator` \u{2014} embeds a live Idealyst app inside the marketing
//! site. Mirrors `examples/docs/src/components/simulator.rs`: thin
//! wrapper around the framework's `Graphics` primitive and the
//! `host-web` shell that runs the wgpu init, render loop, and
//! browser-event translation.
//!
//! Skin + device-profile are passed in by the caller (no reactive
//! observers here \u{2014} the home page swaps the simulator subtree
//! via a `switch` keyed on `is_android`, so a skin change forces
//! the underlying `Graphics` surface to unmount + remount, the new
//! skin gets baked into the fresh on_ready closure, and the wgpu
//! host rebuilds against the new painter). This keeps the
//! component itself stateless and the toggle implementation
//! framework-idiomatic.

use std::rc::Rc;

use runtime_core::primitives::graphics::{OnReadyEvent, OnResizeEvent};
use runtime_core::{component, ui, view, IntoPrimitive, Length, Primitive, StyleRules, StyleSheet};
use host_web::{DeviceProfile, Painter};

#[cfg(target_arch = "wasm32")]
use runtime_core::driver::spawn_async;

mod shared {
    use std::cell::RefCell;
    use std::rc::Rc;
    pub type Slot<T> = Rc<RefCell<Option<T>>>;
    pub fn new<T>() -> Slot<T> {
        Rc::new(RefCell::new(None))
    }
    pub fn with_ref<T, R>(slot: &Slot<T>, f: impl FnOnce(Option<&T>) -> R) -> R {
        f(slot.borrow().as_ref())
    }
    pub fn take<T>(slot: &Slot<T>) -> Option<T> {
        slot.borrow_mut().take()
    }
    pub fn fill<T>(slot: &Slot<T>, value: T) {
        *slot.borrow_mut() = Some(value);
    }
}

/// iPhone 14 / 15 portrait \u{2014} matches `variant_phone::{WIDTH, HEIGHT}`.
pub const DEFAULT_LOGICAL_W: u32 = 390;
pub const DEFAULT_LOGICAL_H: u32 = 844;

/// On-screen CSS width of the embedded canvas. Height is derived
/// from this + the device profile's aspect so the renderer's
/// logical\u{2192}surface mapping stays uniform (otherwise a wider-than-tall
/// CSS box stretches every glyph). 300 px keeps the simulator
/// compact enough that the home-page hero text reads alongside.
const PREVIEW_WIDTH_PX: f32 = 300.0;

pub struct SimulatorProps {
    /// The app to mount inside the simulator. Invoked once after the
    /// wgpu surface is up and the host is built.
    pub build_ui: Rc<dyn Fn() -> Primitive>,
    /// Painter that defines the chrome / palette / paint policy.
    /// `None` resolves to `IosSim`.
    pub skin: Option<Rc<dyn Painter>>,
    /// Device profile (logical size + title + color scheme). `None`
    /// resolves to the iPhone-portrait profile so the embedded
    /// preview matches `variant_phone`.
    pub profile: Option<DeviceProfile>,
}

impl Default for SimulatorProps {
    fn default() -> Self {
        Self {
            build_ui: Rc::new(|| runtime_core::view(Vec::new()).into()),
            skin: None,
            profile: None,
        }
    }
}

fn default_profile() -> DeviceProfile {
    DeviceProfile {
        logical_size: (DEFAULT_LOGICAL_W, DEFAULT_LOGICAL_H),
        position: None,
        title: "Idealyst Simulator".to_string(),
        color_scheme: runtime_core::ColorScheme::Light,
    }
}

fn default_painter() -> Rc<dyn Painter> {
    Rc::new(ios_sim::IosSim::new())
}

#[component(default(
    skin = None,
    profile = None,
))]
pub fn simulator(props: SimulatorProps) -> Primitive {
    let SimulatorProps {
        build_ui,
        skin,
        profile,
    } = props;

    let profile = profile.unwrap_or_else(default_profile);
    let skin = skin.unwrap_or_else(default_painter);
    let logical = (
        profile.logical_size.0 as f32,
        profile.logical_size.1 as f32,
    );

    #[cfg(target_arch = "wasm32")]
    let slot: shared::Slot<host_web::WebHostHandle> = shared::new();
    #[cfg(target_arch = "wasm32")]
    let slot_ready = slot.clone();
    #[cfg(target_arch = "wasm32")]
    let slot_resize = slot.clone();
    #[cfg(target_arch = "wasm32")]
    let slot_lost = slot;

    let graphics = runtime_core::primitives::graphics::graphics(move |_event: OnReadyEvent| {
        #[cfg(target_arch = "wasm32")]
        {
            let slot = slot_ready.clone();
            let build_ui = build_ui.clone();
            let skin = skin.clone();
            let profile = profile.clone();
            let surface = _event.surface;
            let size = _event.size;
            spawn_async(async move {
                let build_ui = build_ui.clone();
                match host_web::mount(surface, size, profile, skin, move || (&*build_ui)()).await {
                    Ok(handle) => shared::fill(&slot, handle),
                    Err(err) => {
                        web_sys::console::warn_1(
                            &format!("[website-simulator] host-web mount failed: {err}")
                                .into(),
                        );
                    }
                }
            });
        }
    })
    .on_resize(move |_event: OnResizeEvent| {
        #[cfg(target_arch = "wasm32")]
        {
            shared::with_ref(&slot_resize, |handle| {
                if let Some(h) = handle {
                    h.resize(_event.size);
                }
            });
        }
    })
    .on_lost(move || {
        #[cfg(target_arch = "wasm32")]
        {
            // Take the handle OUT of the slot before letting it drop:
            // the handle's Drop walks listeners, the render loop, and
            // the host \u{2014} we don't want any of that running while
            // the slot is still borrowed.
            let stale = shared::take(&slot_lost);
            drop(stale);
        }
    });

    // Pin the canvas to the device aspect via a sized wrapper. The
    // web Graphics primitive forces `width: 100%; height: 100%`
    // INLINE on the canvas, so a class-level size wouldn't win. The
    // wrapper View carries the fixed dimensions; the canvas fills
    // it at the right aspect ratio, keeping the renderer's
    // logical\u{2192}surface mapping uniform.
    let preview_height_px = PREVIEW_WIDTH_PX * logical.1 / logical.0;
    let wrapper_sheet = Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Length::Px(PREVIEW_WIDTH_PX).into()),
        height: Some(Length::Px(preview_height_px).into()),
        ..Default::default()
    }));
    let wrapper = view(vec![graphics.into_primitive()]).with_style(wrapper_sheet);

    ui! {
        wrapper
    }
}
