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
// `DeviceProfile` + `Painter` come through the `host-wgpu` umbrella,
// which cfg-routes internally to `host-web` on wasm and
// `host-ios-mobile` on iOS. Author code names one symbol per
// concept and the right backend is picked at link time.
//
// Why this isn't behind a `cfg(target_arch)` here: per §7 of
// CLAUDE.md and `[[feedback_cfg_hack_signals_missing_backend_method]]`,
// platform variance belongs in the backend layer (the host crates),
// not in author code. `host-wgpu` is that backend abstraction —
// `mount` returns `Err(MountError::Unsupported)` on targets without
// a wgpu host wired (macOS-AppKit, terminal, …) so the consumer can
// fall back without target gates.
//
// Code-reachability cost on web: `host_wgpu` transitively pulls the
// same `host-web` → `render-wgpu` → `glyphon` / `cosmic-text` / `wgpu`
// / `naga` graph the previous direct `host_web` import did. wasm-split
// keeps that behind the same `lazy!` chunk as before because the
// `mount`/`DeviceProfile` reachability surface is unchanged.
use host_wgpu::{DeviceProfile, Painter};
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

/// Outer device chassis — bezel + corner clip + drop shadow. Sits
/// around the simulator's GPU canvas (or the placeholder's "off"
/// screen) so the device chrome and the screen contents share one
/// rounded boundary.
///
/// Promoted from the snake_case `wrap_in_chassis` helper because it
/// takes children and is called from multiple sites (CLAUDE.md §9.5).
/// Container shape: `children` flows into the inner `view`, the
/// chassis stylesheet decorates the wrapper.
#[derive(Default)]
pub struct ChassisProps {
    pub children: Vec<Element>,
}

#[component]
pub fn Chassis(props: ChassisProps) -> Element {
    view(props.children)
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
    ui! {
        Chassis {
            off_screen
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

    // Single slot for the wgpu host handle — `host_wgpu::HostHandle`
    // type-aliases to `WebHostHandle` on wasm, `IosHostHandle` on
    // iOS, and a stub on unsupported targets. The on_ready /
    // on_resize / on_lost callbacks below share it through clones,
    // no per-target branching needed.
    let slot: shared::Slot<host_wgpu::HostHandle> = shared::new();
    let slot_ready = slot.clone();
    let slot_resize = slot.clone();
    let slot_lost = slot;

    // KNOWN ISSUE — auto-pause via `use_focus()` → `host.pause()`
    // disabled until the wgpu Host's renderer state-reset bug is
    // understood. Symptom: after `Host::unmount()` + `mount()`,
    // Taffy layout compute succeeds (root=390×844, children=6,
    // child0=390×844 all confirmed via diagnostic), but the
    // `Renderer::walk` produces no visible draws — the surface
    // stays at the white clear color forever. Tried: clearing
    // `presence_tweens`, `animator`, `sticky_registry`,
    // `graphics_cache`, `image_cache`, text-shaper `buffers`,
    // `session::REGISTRY`, calling `surface.configure()` on resume.
    // None of those individually fix it.
    //
    // The cooperative approach already in place delivers the
    // observable pause/resume behavior the auto-pause was meant
    // to deliver, without the renderer bug:
    //
    //   - `host-ios-mobile::draw_frame` skips Metal encode +
    //     present via `is_view_visible()` while the MetalView is
    //     hidden (`setHidden:true` on a navigator-persistent
    //     screen). GPU work pauses.
    //
    //   - The welcome app's `raf_loop_scoped` body reads
    //     `runtime_core::is_frame_active()` and skips advancement
    //     when painting is paused — and tracks paused duration so
    //     positions resume exactly where they left off.
    //
    // The wgpu Host stays mounted across navigation; its renderer
    // state is preserved; the canvas content is intact on return.
    // `IosHostHandle::pause` / `resume` stays exposed for opt-in
    // once the renderer-no-op bug is understood.

    let graphics = runtime_core::primitives::graphics::graphics(move |event: OnReadyEvent| {
        // Fresh `on_ready` = fresh wgpu surface = a truly fresh mount
        // of the embedded app (e.g. after a `MountPolicy::LazyDisposing`
        // remount of the host screen). Wipe any session-keyed AVs the
        // previous embedded run left in the thread's `session::REGISTRY`
        // so the new mount's `keyed(…, default)` calls return AVs at
        // their initial defaults — the welcome demo replays its act
        // timeline + sun pulse from time=0 instead of resuming
        // mid-orbit with stale values.
        //
        // Scoped to `"welcome_"` so the outer website's session state
        // (if it grows any in the future) isn't collateral damage.
        // Hot-patch rerenders DON'T fire `on_ready` (the surface
        // survives the rerender), so the existing
        // `[[project_session_animated]]` "skip re-running acts on
        // save" property is preserved for the dev edit loop.
        runtime_core::session::clear_prefix("welcome_");
        // Also drop the session clock so the embedded app's next
        // `session::epoch_micros()` re-reads `now`. Without this, a
        // `clear_prefix` alone leaves the epoch frozen at original
        // install time — the welcome's `raf_loop` body computes
        // `elapsed = now - epoch` and jumps straight to mid-orbit on
        // remount, so the "reset" doesn't read as a reset.
        runtime_core::session::reset_epoch();
        let slot = slot_ready.clone();
        let build_ui = build_ui.clone();
        let painter = painter_for(skin);
        // Build the DeviceProfile lazily on mount — its construction
        // is the load-bearing path that brings the wgpu graph in,
        // and on web wasm-split keeps that behind the lazy chunk
        // that materializes this on_ready closure.
        let profile = default_profile();
        let surface = event.surface;
        let size = event.size;
        // `spawn_async` is the runtime-installed executor. On wasm it
        // rides wasm-bindgen-futures; on iOS the `async-driver`
        // feature on `backend-ios-mobile` plugs into libdispatch.
        // Either way the `request_adapter` / `request_device`
        // futures resolve on the main thread without blocking.
        spawn_async(async move {
            match host_wgpu::mount(surface, size, profile, painter, build_ui).await {
                Ok(handle) => shared::fill(&slot, handle),
                Err(err) => {
                    // On wasm this goes to the browser console via
                    // the runtime's panic-hook routing; on native
                    // it lands on stderr captured by the dev loop.
                    // Targets without a wgpu host hit
                    // `MountError::Unsupported` here — that's the
                    // documented "no preview, fall back to chassis"
                    // path and not a real failure.
                    eprintln!("[website-simulator] host-wgpu mount failed: {err}");
                }
            }
        });
    })
    .on_resize(move |event: OnResizeEvent| {
        shared::with_ref(&slot_resize, |handle| {
            if let Some(h) = handle {
                h.resize(event.size);
            }
        });
    })
    .on_lost(move || {
        // Take the handle OUT of the slot before letting it drop —
        // the handle's Drop walks the render loop + host state, and
        // we don't want any of that running while the slot is still
        // borrowed.
        let stale = shared::take(&slot_lost);
        drop(stale);
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
        ui! {
            Chassis {
                wrapper
            }
        }
    } else {
        wrapper
    }
}
