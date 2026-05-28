//! `Simulator` — embeds a live Idealyst app inside the docs page.
//!
//! Thin wrapper around the framework's `Graphics` surface primitive
//! and the `host-web` shell crate. The wgpu init, per-frame paint,
//! and browser-event → `EventSink` translation all live in
//! [`host_web::mount`]; this component is just the lifecycle glue
//! (`on_ready` → `mount`, `on_resize` → `handle.resize`, `on_lost`
//! → drop the handle).
//!
//! Invocation shape (via the `ui!` macro's `simulator!` emitter):
//!
//! ```ignore
//! ui! {
//!     Simulator(build_ui = Rc::new(|| pages::overview::page()))
//! }
//! ```
//!
//! Notes:
//!
//! - Sizes the `<canvas>` to the device profile's aspect via a
//!   wrapper `View`; the web `Graphics` primitive forces
//!   `width: 100%; height: 100%` inline on the canvas itself, so
//!   the wrapper carries the fixed dimensions and the canvas fills
//!   it at the right ratio (otherwise a wide content column would
//!   stretch every glyph horizontally — see
//!   `crates/backend/web/src/primitives/graphics.rs:153` for the
//!   inline `set_attribute("style", …)` that wins over class
//!   styles).
//! - Default skin is `IosSim`; pass a custom one via `skin`.
//! - Default device profile matches `variant_phone` (iPhone-14
//!   portrait, 390 × 844 logical).
//! - Pointer / wheel input is wired by `host-web`; the embedded
//!   host receives `pointerdown` / `pointermove` / `pointerup` /
//!   `pointercancel` / `wheel` through `EventSink` and dispatches
//!   them into the embedded reactive tree.

use std::rc::Rc;

use runtime_core::primitives::graphics::{OnReadyEvent, OnResizeEvent};
use runtime_core::{component, ui, view, IntoPrimitive, Length, Primitive, StyleRules, StyleSheet};
// `host_web` re-exports `DeviceProfile` and `Painter` so the Simulator
// only needs one preview-stack dep.
use host_web::{DeviceProfile, Painter};

#[cfg(target_arch = "wasm32")]
use runtime_core::driver::spawn_async;

// Cross-target shared-state cell. `host_web::WebHostHandle` is
// `!Send` on wasm; the lifecycle callbacks all fire on the JS event
// loop, so `Rc<RefCell<…>>` is enough. On native targets the
// Simulator is a no-op (no canvas to embed into) but the slot still
// has to compile — `Rc<RefCell<…>>` is fine there too since
// everything is local to the (also no-op) callbacks.
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

/// Default device profile when the caller doesn't supply one — iPhone
/// 14 / 15 portrait. Matches `variant_phone::{WIDTH, HEIGHT}` so the
/// embedded preview lays out identically to the native phone variant.
pub const DEFAULT_LOGICAL_W: u32 = 390;
pub const DEFAULT_LOGICAL_H: u32 = 844;

/// On-screen CSS width of the embedded canvas. The height is derived
/// from this and the device profile's aspect so the renderer's
/// logical→surface mapping stays uniform (a wider-than-tall CSS box
/// would otherwise stretch every glyph). 320 px fits comfortably in
/// a typical docs content column and leaves room for the prose
/// alongside.
const PREVIEW_WIDTH_PX: f32 = 320.0;

/// Props delivered by the `simulator!` invocation macro.
///
/// `build_ui` is `Rc<dyn Fn() -> Primitive>` so callers can build the
/// closure once (e.g. `Rc::new(|| pages::overview::page())`) and the
/// Simulator can clone it into the Graphics on_ready closure. The
/// closure runs exactly once, when the host mounts the preview.
pub struct SimulatorProps {
    /// The app to mount inside the simulator. Invoked once after the
    /// wgpu surface is up and the host is built.
    pub build_ui: Rc<dyn Fn() -> Primitive>,
    /// Pluggable simulator skin. `None` resolves to `IosSim`; pass
    /// `Some(Rc::new(AndroidSim::new()))` for the Material 3 look,
    /// or any other `render_wgpu::Painter` implementor.
    pub skin: Option<Rc<dyn Painter>>,
    /// Device profile (logical size + title + color scheme). When
    /// `None`, an iPhone-14-portrait profile is used so the embedded
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

// `skin` / `profile` default through the macro so callers only have
// to fill in `build_ui = …`. The closures pick `IosSim` + the
// iPhone-portrait profile, matching `variant_phone`.
#[component(default(
    skin = None,
    profile = None,
))]
pub fn Simulator(props: SimulatorProps) -> Primitive {
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

    // One slot held by all three callbacks. `on_ready` populates it
    // with the `WebHostHandle`; `on_resize` forwards new sizes;
    // `on_lost` drops the handle outside the slot guard.
    //
    // We use the same `Slot<_>` shape on every target — on wasm the
    // handle is `host_web::WebHostHandle`; on native the slot stays
    // empty (the Simulator's wgpu path doesn't run there yet).
    #[cfg(target_arch = "wasm32")]
    let slot: shared::Slot<host_web::WebHostHandle> = shared::new();
    #[cfg(target_arch = "wasm32")]
    let slot_ready = slot.clone();
    #[cfg(target_arch = "wasm32")]
    let slot_resize = slot.clone();
    #[cfg(target_arch = "wasm32")]
    let slot_lost = slot;

    let graphics = runtime_core::primitives::graphics::graphics(move |_event: OnReadyEvent| {
        // On native targets we don't have a wgpu/web shell yet; the
        // Graphics surface still allocates but nothing drives it.
        // Web is the only path that actually mounts.
        #[cfg(target_arch = "wasm32")]
        {
            let slot = slot_ready.clone();
            let build_ui = build_ui.clone();
            let skin = skin.clone();
            let profile = profile.clone();
            let surface = _event.surface;
            let size = _event.size;
            spawn_async(async move {
                // `host_web::mount` does the entire init: wgpu
                // surface, adapter, device, queue, host, renderer,
                // mount the UI, start the render loop, attach
                // pointer/wheel listeners. Returns a handle whose
                // drop tears everything back down in the right
                // order.
                let build_ui = build_ui.clone();
                match host_web::mount(surface, size, profile, skin, move || (&*build_ui)()).await {
                    Ok(handle) => shared::fill(&slot, handle),
                    Err(err) => {
                        web_sys::console::warn_1(
                            &format!("[simulator] host-web mount failed: {err}").into(),
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
            // Take the handle out FIRST, then let it drop after the
            // slot guard releases. Same anti-deadlock pattern as
            // `gradient.rs`: the handle's `Drop` walks its
            // listeners, the render loop, and the host — we don't
            // want any of that running while the slot is still
            // borrowed.
            let stale = shared::take(&slot_lost);
            drop(stale);
        }
    });

    // Pin the canvas to the device's aspect ratio by wrapping the
    // Graphics primitive in a sized `View`. The web Graphics
    // primitive forces `style="width: 100%; height: 100%"` *inline*
    // on the `<canvas>` (see `backend/web/src/primitives/graphics.rs`,
    // `set_attribute("style", …)`), which beats any class style we'd
    // apply to the canvas directly. The wrapper View carries the
    // fixed dimensions; the canvas's `100%` then fills the wrapper
    // at the right aspect, so the renderer's logical→surface mapping
    // stays uniform and glyphs don't stretch.
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
