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
use runtime_core::{
    component, ui, view, Color, IntoElement, Length, Overflow, Element, Shadow, StyleRules,
    StyleSheet,
};
// `DeviceProfile` is only constructed inside the `Simulator` component
// (which the home page mounts via `lazy! { Simulator(...) }`). Keeping
// the import out of file scope avoids a code-reachability path from
// base → `host_web` → `render-wgpu` → `glyphon` → `cosmic-text` + `wgpu`
// + `naga`. The base bundle would otherwise pay for the entire GPU
// + text-shaping stack even though it's only painted into the canvas
// inside the lazy chunk.
#[cfg(target_arch = "wasm32")]
use host_web::{DeviceProfile, Painter};
#[cfg(target_arch = "wasm32")]
use runtime_core::driver::spawn_async;

/// Target-agnostic identifier for which device chrome the embedded
/// Simulator should run. Call sites use this instead of constructing
/// a `host_web::Painter` directly so they don't need their own
/// `#[cfg(target_arch = "wasm32")]` branch — the painter wiring lives
/// inside `simulator()`, which is the one place that legitimately
/// cares about the underlying wgpu skin crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimulatorSkin {
    Ios,
    Android,
}

impl Default for SimulatorSkin {
    fn default() -> Self {
        SimulatorSkin::Ios
    }
}

#[cfg(target_arch = "wasm32")]
fn painter_for(skin: SimulatorSkin) -> Rc<dyn Painter> {
    // `with_corner_radius(0.0)` suppresses each painter's rounded
    // device-frame SDF pass, so the engine doesn't draw an inner
    // black bezel ring inside the canvas. The outer chassis on the
    // Simulator component (`chassis_sheet`) is the only device
    // frame visible on the page.
    match skin {
        SimulatorSkin::Android => Rc::new(android_sim::AndroidSim::new().with_corner_radius(0.0)),
        SimulatorSkin::Ios => Rc::new(ios_sim::IosSim::new().with_corner_radius(0.0)),
    }
}

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
    pub build_ui: Rc<dyn Fn() -> Element>,
    /// Which device chrome to paint. Defaults to `Ios`. Kept as a
    /// plain enum (not an `Rc<dyn Painter>`) so author call sites
    /// don't have to import or `#[cfg]`-gate `host_web::Painter` /
    /// `ios_sim` / `android_sim` themselves.
    pub skin: SimulatorSkin,
    /// When true, wrap the canvas in a black outer chassis (rounded
    /// corners + drop shadow + clip) so the embedded device reads
    /// as a complete handset rather than a bare wgpu surface.
    /// Matches the chassis used by [`simulator_placeholder`], so a
    /// `lazy! { simulator(...) }` + `placeholder(simulator_placeholder)`
    /// pair has zero on-load layout shift and a continuous bezel
    /// across the loading→loaded transition. Defaults to `true`.
    pub chassis: bool,
}

impl Default for SimulatorProps {
    fn default() -> Self {
        Self {
            build_ui: Rc::new(|| runtime_core::view(Vec::new()).into()),
            skin: SimulatorSkin::default(),
            chassis: true,
        }
    }
}

// Outer chassis around the wgpu canvas. Black so it blends with the
// engine's `device_frame_pipeline` (which paints opaque black on the
// canvas outside the screen rounded rect). `overflow: Hidden` + the
// matching corner radius clip the canvas to the bezel curve so the
// painter's edge-to-edge fills don't bleed past the chassis. Kept in
// sync with [`screen_inner_radius`] so the placeholder's inner screen
// nests concentric to this outer curve.
const CHASSIS_RADIUS_PX: f32 = 44.0;
const CHASSIS_PADDING_PX: f32 = 12.0;

/// Corner radius for whatever sits inside the chassis (the live
/// canvas wrapper, or the placeholder's off-screen). Concentric with
/// the outer chassis curve: outer radius minus the chassis padding.
/// The chassis' own `overflow: Hidden` does NOT reliably clip a wgpu
/// `<canvas>` (a replaced element paints its own pixels past an
/// ancestor's rounded corners), so the inner element has to round
/// itself.
fn screen_inner_radius() -> f32 {
    (CHASSIS_RADIUS_PX - CHASSIS_PADDING_PX).max(0.0)
}

fn chassis_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        background: Some(Color("#000000".into()).into()),
        border_top_left_radius: Some(Length::Px(CHASSIS_RADIUS_PX).into()),
        border_top_right_radius: Some(Length::Px(CHASSIS_RADIUS_PX).into()),
        border_bottom_left_radius: Some(Length::Px(CHASSIS_RADIUS_PX).into()),
        border_bottom_right_radius: Some(Length::Px(CHASSIS_RADIUS_PX).into()),
        padding_top: Some(Length::Px(CHASSIS_PADDING_PX).into()),
        padding_right: Some(Length::Px(CHASSIS_PADDING_PX).into()),
        padding_bottom: Some(Length::Px(CHASSIS_PADDING_PX).into()),
        padding_left: Some(Length::Px(CHASSIS_PADDING_PX).into()),
        overflow: Some(Overflow::Hidden),
        shadow: Some(Shadow {
            x: 0.0,
            y: 18.0,
            blur: 48.0,
            color: Color("rgba(15, 17, 30, 0.28)".into()),
        }),
        flex_shrink: Some(0.0_f32.into()),
        ..Default::default()
    }))
}

fn wrap_in_chassis(canvas_wrapper: Element) -> Element {
    view(vec![canvas_wrapper])
        .with_style(chassis_sheet())
        .into_element()
}

/// Compute the on-screen (CSS px) width/height of the simulator
/// canvas from a logical-pixel device size. Pure arithmetic — kept
/// out of the wasm-only block so the cross-target / placeholder paths
/// reach it without dragging `host_web::DeviceProfile` into base.
fn preview_dimensions(logical: (u32, u32)) -> (f32, f32) {
    let logical_w = logical.0 as f32;
    let logical_h = logical.1 as f32;
    (PREVIEW_WIDTH_PX, PREVIEW_WIDTH_PX * logical_h / logical_w)
}

/// Renders the same outer chassis the loaded simulator uses, with
/// an "off" screen inside (welcome's `COLOR_LIGHT_BG`). Designed to
/// be the placeholder for a `lazy! { simulator(...) }` block so the
/// hero layout reserves the device's exact footprint while the
/// chunk fetches and the only visual delta on load is the canvas
/// painting INSIDE the chassis.
///
/// Pass the logical device size (or `None` for the iPhone-portrait
/// default) so the placeholder's preview rectangle matches the loaded
/// canvas's aspect ratio. Plain `(u32, u32)` instead of
/// `host_web::DeviceProfile` so this function — which IS reachable
/// from the base bundle (via `.placeholder(|| simulator_placeholder())`)
/// — doesn't drag the wgpu / cosmic-text / naga stack into base.
pub fn simulator_placeholder(logical_size: Option<(u32, u32)>) -> Element {
    // Welcome app's COLOR_LIGHT_BG. Inlined rather than imported so
    // the placeholder stays in main's bundle on web (importing the
    // welcome crate would pull its full transitive deps into main).
    const SCREEN_FILL: &str = "#000000";
    let inner_radius = screen_inner_radius();

    let logical = logical_size.unwrap_or((DEFAULT_LOGICAL_W, DEFAULT_LOGICAL_H));
    let (w, h) = preview_dimensions(logical);

    let screen_style = Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Length::Px(w).into()),
        height: Some(Length::Px(h).into()),
        background: Some(Color(SCREEN_FILL.into()).into()),
        border_top_left_radius: Some(Length::Px(inner_radius).into()),
        border_top_right_radius: Some(Length::Px(inner_radius).into()),
        border_bottom_left_radius: Some(Length::Px(inner_radius).into()),
        border_bottom_right_radius: Some(Length::Px(inner_radius).into()),
        ..Default::default()
    }));

    let off_screen = view(Vec::new())
        .with_style(screen_style)
        .into_element();
    wrap_in_chassis(off_screen)
}

// `DeviceProfile` lives on `host_web` (re-exported from `render-api`).
// Constructing one is what links the heavy `render-wgpu` / `glyphon` /
// `cosmic-text` / `wgpu` / `naga` graph — keep this fn wasm32-only
// (where the simulator actually runs) so non-wasm compiles stay clean
// AND wasm-split sees no base-reachable path that materializes a
// `DeviceProfile`.
#[cfg(target_arch = "wasm32")]
fn default_profile() -> DeviceProfile {
    DeviceProfile {
        logical_size: (DEFAULT_LOGICAL_W, DEFAULT_LOGICAL_H),
        position: None,
        title: "Idealyst Simulator".to_string(),
        color_scheme: runtime_core::ColorScheme::Light,
    }
}

#[component(default(
    skin = SimulatorSkin::Ios,
    chassis = true,
))]
pub fn Simulator(props: SimulatorProps) -> Element {
    let SimulatorProps {
        build_ui,
        skin,
        chassis,
    } = props;

    let (preview_w_px, preview_height_px) =
        preview_dimensions((DEFAULT_LOGICAL_W, DEFAULT_LOGICAL_H));
    // `skin` + `build_ui` are only consumed by the wasm32 on_ready
    // closure below (the painter wiring + `host_web::mount`). Bind
    // them on non-wasm so the destructure stays exhaustive without
    // warnings. iOS Metal mount is intentionally a follow-up — the
    // iOS Graphics primitive already provides a Metal-backed
    // `raw_window_handle` (see [crates/backend/ios/mobile/src/imp/graphics.rs]),
    // and `render-wgpu`'s `Host`/`Renderer` stack is platform-neutral,
    // but no `host-ios-mobile` crate analogous to `host-web` exists
    // yet to spin up `wgpu::Instance(Backends::METAL)`, configure the
    // surface, drive a CADisplayLink render loop, and load fonts.
    // Until that lands, the Simulator on iOS renders the chassis
    // around an unmounted (visually empty) CAMetalLayer.
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (&skin, &build_ui);

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
            let painter = painter_for(skin);
            // Build the DeviceProfile lazily on mount — its construction
            // is the load-bearing path that brings host_web in, and we
            // want it confined to the wgpu-mounting path inside the
            // lazy chunk.
            let profile = default_profile();
            let surface = _event.surface;
            let size = _event.size;
            spawn_async(async move {
                let build_ui = build_ui.clone();
                match host_web::mount(surface, size, profile, painter, move || (&*build_ui)()).await {
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
    //
    // When wrapped in the chassis, round + clip the wrapper to the
    // concentric inner radius so the canvas's painted pixels follow
    // the bezel curve. The chassis' own `overflow: Hidden` can't be
    // relied on to clip the canvas (see `screen_inner_radius`).
    let mut wrapper_rules = StyleRules {
        width: Some(Length::Px(preview_w_px).into()),
        height: Some(Length::Px(preview_height_px).into()),
        ..Default::default()
    };
    if chassis {
        let r = screen_inner_radius();
        wrapper_rules.border_top_left_radius = Some(Length::Px(r).into());
        wrapper_rules.border_top_right_radius = Some(Length::Px(r).into());
        wrapper_rules.border_bottom_left_radius = Some(Length::Px(r).into());
        wrapper_rules.border_bottom_right_radius = Some(Length::Px(r).into());
        wrapper_rules.overflow = Some(Overflow::Hidden);
    }
    // Force the Graphics primitive to fill its sized wrapper. Without
    // this, native backends (iOS UIView + Taffy) lay the canvas out at
    // its intrinsic main-axis size (0) and the chassis renders around a
    // collapsed CAMetalLayer — the `on_ready` event fires with a
    // `300×0` surface. On web the canvas's inline `width:100%;
    // height:100%` already wins; this just keeps native backends
    // symmetrical with that default.
    let graphics_rules = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    };
    let wrapper = view(vec![graphics
        .with_style(Rc::new(StyleSheet::r#static(graphics_rules)))
        .into_element()])
        .with_style(Rc::new(StyleSheet::r#static(wrapper_rules)))
        .into_element();

    if chassis {
        wrap_in_chassis(wrapper)
    } else {
        wrapper
    }
}
