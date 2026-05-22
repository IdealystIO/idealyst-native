//! The pluggable platform-skin contract.
//!
//! A `Skin` is a stateless palette + paint policy: how to draw
//! each native widget chrome (toggle, slider, text input,
//! activity indicator) plus the on-screen keyboard layout and
//! its key chrome. The renderer holds an `Rc<dyn Skin>` and
//! dispatches every per-frame paint call through it.
//!
//! Skins live in their own crates (`ios-sim`, `android-sim`)
//! and depend on this one for the trait + helper types
//! (`RectInstance`, `StagedText`, `KeySpec`, `LayoutMetrics`,
//! `rect_inst`). The preview variant crates instantiate a
//! concrete skin and hand it to the host shell at `run` time.
//!
//! No `SimulatedPlatform` matching here — the trait IS the
//! dispatch. Two-platform parity is enforced by the trait, not
//! by an enum that future skins would have to extend.

use std::collections::HashMap;

use glyphon::Buffer;

use crate::keyboard::{KeySpec, LaidKey, LayoutMetrics};
use crate::pipeline::Instance as RectInstance;
use crate::text::StagedText;

/// A platform skin: every paint method any of the native widgets
/// or the on-screen keyboard could need, plus the keyboard's
/// row content + inter-key spacing.
///
/// Skins are stateless w.r.t. per-widget animation — `t`,
/// `value`, `phase`, focus flags, etc. are passed in by the
/// renderer's walk. Use `Rc<dyn Skin>` to hand a skin to the
/// host; the renderer holds one for the lifetime of the frame.
/// Visual modulation a `Skin` applies to a Button on press.
/// Returned by [`Skin::button_press_visual`] given the current
/// press progress `t` (0 = rest, 1 = fully pressed).
///
/// Two channels, each optional in the no-op sense:
/// - `text_alpha_factor` is a multiplier on the label's alpha
///   channel. iOS dims to ~0.5 at full press; M3 leaves it at 1.
/// - `bg_overlay` is a color composited on top of the resolved
///   background fill *before* the rect is queued. M3 paints an
///   8% on-primary state-layer; iOS leaves it `None`.
///
/// Skins return [`ButtonPressVisual::rest`] when `t == 0` (or
/// always, if the skin doesn't opinion press feedback).
#[derive(Copy, Clone, Debug)]
pub struct ButtonPressVisual {
    pub text_alpha_factor: f32,
    pub bg_overlay: Option<[f32; 4]>,
}

impl ButtonPressVisual {
    pub fn rest() -> Self {
        Self { text_alpha_factor: 1.0, bg_overlay: None }
    }
}

pub trait Skin {
    // -----------------------------------------------------------
    // Identity
    // -----------------------------------------------------------

    /// The platform identity this skin presents to the framework.
    /// `WgpuBackend::platform` delegates here so author code reading
    /// `framework_core::platform()` sees the skin's emulated host
    /// (e.g. `Custom("Sim")` for the ios-sim / android-sim skins
    /// — the renderer is wgpu under the hood, but as far as the
    /// app is concerned it's running in a simulator).
    ///
    /// Default is `Custom("")` (no identity declared) for skins
    /// that don't care to opinion the platform read-out.
    fn platform(&self) -> framework_core::Platform {
        framework_core::Platform::Custom("")
    }

    // -----------------------------------------------------------
    // Primitive style defaults
    // -----------------------------------------------------------

    /// Default `StyleRules` for a `Button`. The backend merges
    /// these *underneath* the author's stylesheet, so an unstyled
    /// `button(...)` looks platform-native (iOS tinted text, M3
    /// filled-pill) while any field the author explicitly sets
    /// still wins. Skins that don't want to opinion buttons can
    /// leave the default impl, which returns an empty rules struct.
    fn button_defaults(&self) -> framework_core::StyleRules {
        framework_core::StyleRules::default()
    }

    /// Visual modulation applied to a Button at press progress
    /// `t` (0 = rest, 1 = fully pressed). The renderer
    /// interpolates `t` via the animator and calls this on every
    /// paint of a pressed (or unpressing) button, then folds the
    /// returned values into the text color and background fill.
    /// Default impl is a no-op so skins that don't care about
    /// press feedback opt out for free.
    fn button_press_visual(&self, t: f32) -> ButtonPressVisual {
        let _ = t;
        ButtonPressVisual::rest()
    }

    // -----------------------------------------------------------
    // Native widgets
    // -----------------------------------------------------------

    /// Append rect instances for a Toggle at the given frame.
    /// `t` is the thumb position in `0..=1`; `tint` overrides the
    /// ON-state track color (else use the skin's default accent).
    fn paint_toggle(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        t: f32,
        tint: Option<[f32; 4]>,
        rects: &mut Vec<RectInstance>,
    );

    /// Append rect instances for a Slider at the given frame.
    /// `tint` overrides the active-track color.
    #[allow(clippy::too_many_arguments)]
    fn paint_slider(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        value: f32,
        min: f32,
        max: f32,
        tint: Option<[f32; 4]>,
        rects: &mut Vec<RectInstance>,
    );

    /// Append rect + text instances for a TextInput. `field_bg`
    /// overrides the field's fill color.
    #[allow(clippy::too_many_arguments)]
    fn paint_text_input<'a>(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        is_focused: bool,
        draw_caret: bool,
        is_placeholder: bool,
        buffer: &'a Buffer,
        caret_x_local: f32,
        text_color: [f32; 4],
        field_bg: Option<[f32; 4]>,
        rects: &mut Vec<RectInstance>,
        texts: &mut Vec<StagedText<'a>>,
    );

    /// Append rect instances for an ActivityIndicator. `phase`
    /// is the rotation phase in `[0.0, 1.0)`; `tint` overrides
    /// the skin's default spinner color.
    fn paint_activity_indicator(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        phase: f32,
        tint: Option<[f32; 4]>,
        rects: &mut Vec<RectInstance>,
    );

    // -----------------------------------------------------------
    // On-screen keyboard
    // -----------------------------------------------------------

    /// Row content for the keyboard. Called by the shared
    /// keyboard layout engine on every frame the keyboard is
    /// visible. Keep allocations light — the `Vec`s are
    /// short-lived.
    fn keyboard_rows(&self) -> Vec<Vec<KeySpec>>;

    /// Inter-key spacing knobs. Consumed by the shared layout
    /// engine; lets each skin tighten or loosen its key gaps
    /// without touching the layout math.
    fn keyboard_layout_metrics(&self) -> LayoutMetrics;

    /// Paint the keyboard overlay. The shared layout engine has
    /// already produced `keyboard_rect` (full panel) and
    /// `laid_keys` (each key's absolute screen rect). The skin
    /// is responsible for the panel background, every key's
    /// chrome, and the label glyphs.
    ///
    /// `pressed_label` is the label of a key currently
    /// highlighted as pressed (matches the `KeySpec.label` of
    /// the recently-tapped key). Skins paint that key with a
    /// darker chrome — `None` means no key is pressed.
    fn paint_keyboard<'a>(
        &self,
        keyboard_rect: (f32, f32, f32, f32),
        laid_keys: &[LaidKey],
        pressed_label: Option<&'static str>,
        glyphs: &'a HashMap<&'static str, Buffer>,
        rects: &mut Vec<RectInstance>,
        texts: &mut Vec<StagedText<'a>>,
    );

    // -----------------------------------------------------------
    // Navigator header
    // -----------------------------------------------------------

    /// Paint a navigator's header bar at the top of a screen.
    ///
    /// `rect` is the header strip's screen-space rect (always
    /// `NAV_HEADER_HEIGHT` tall, spans the navigator's width).
    /// `chrome` describes what to put in each slot — what title
    /// to draw, whether to show a back chevron, what icons go in
    /// the left and right slots, and which author-supplied
    /// colors (from per-screen `ScreenOptions` + navigator-level
    /// `header_style` / `title_style` / `button_style`) override
    /// the skin's defaults.
    ///
    /// `hit_regions` is appended to with each tappable slot's
    /// rect + action so the host's pointer dispatch can route
    /// header-bar presses back through `NavigatorControl`.
    /// Skins decide button slot positions; the renderer just
    /// trusts whatever's pushed here.
    fn paint_navigator_header<'a, 'b>(
        &self,
        rect: (f32, f32, f32, f32),
        chrome: NavigatorHeaderChrome<'a, 'b>,
        rects: &mut Vec<RectInstance>,
        texts: &mut Vec<StagedText<'a>>,
        hit_regions: &mut Vec<NavigatorHeaderHit>,
    );

    // -----------------------------------------------------------
    // Device chrome (status bar + home indicator / gesture nav)
    // -----------------------------------------------------------

    /// Reserved system-UI insets — the top status bar height,
    /// the bottom home-indicator / gesture-nav height, and any
    /// horizontal cutouts. The host writes this into the
    /// framework's reactive `safe_area_insets` signal on
    /// startup so apps using `.safe_area(...)` push their
    /// content into the safe region.
    ///
    /// Default: all zeros — backwards-compatible for any future
    /// skin that doesn't paint device chrome.
    fn safe_area_insets(&self) -> framework_core::EdgeInsets {
        framework_core::EdgeInsets::ZERO
    }

    /// Paint the simulator's mock device chrome — top status
    /// bar, bottom home indicator / gesture nav — into the
    /// renderer's top-z batch. Skins decide what to draw
    /// (clock, signal/wifi/battery icons, home pill). The
    /// status bar's clock can read `now` for a live tick.
    ///
    /// `viewport` is the full window size in logical px;
    /// `insets` is whatever this skin returned from
    /// [`Self::safe_area_insets`] so the skin can size its
    /// strips without re-computing.
    ///
    /// Default: no-op. Pre-rendered glyph buffers for the
    /// clock + status digits live on the `Host`'s
    /// `chrome_glyphs` map — the skin looks up labels by name
    /// the same way it does keyboard glyphs.
    #[allow(unused_variables)]
    fn paint_device_chrome<'a>(
        &self,
        viewport: (f32, f32),
        insets: framework_core::EdgeInsets,
        now: web_time::Instant,
        glyphs: &'a HashMap<&'static str, Buffer>,
        rects: &mut Vec<RectInstance>,
        texts: &mut Vec<StagedText<'a>>,
    ) {
    }

    /// Outer corner radius of the simulated device's display,
    /// in logical px. The renderer's `device_frame` pipeline
    /// uses this to paint opaque black in the region OUTSIDE
    /// the rounded display path — replaces the older corner-
    /// mask + bezel-border combo with a single fullscreen SDF
    /// draw. Default 0 = no rounding, no device frame painted.
    fn device_corner_radius(&self) -> f32 {
        0.0
    }

    /// Labels the skin needs glyphon buffers for in
    /// [`paint_device_chrome`]. The host pre-builds one
    /// `Buffer` per label at startup and refreshes any whose
    /// value changes (e.g., the clock once per minute). Each
    /// entry is `(key, initial_text, font_size)`; the host
    /// uses `key` to look the buffer up and `initial_text` to
    /// shape it.
    ///
    /// Default: empty — skins that don't paint device chrome
    /// don't need any glyphs.
    fn chrome_glyph_labels(&self) -> Vec<(&'static str, String, f32)> {
        Vec::new()
    }
}

/// Slots + author-supplied colors handed to
/// [`Skin::paint_navigator_header`]. Borrows everything by
/// reference: the renderer owns the underlying glyphon buffer,
/// option strings, and color slots, and they outlive the paint
/// call.
///
/// Two lifetimes: `'a` ties the title buffer to the renderer's
/// long-lived text store; `'b` is a possibly-shorter lifetime
/// for the icon-name string slices (held inside a screen's
/// `RefCell` borrow that doesn't outlive a single frame).
pub struct NavigatorHeaderChrome<'a, 'b> {
    /// Title buffer pre-measured by the renderer. `None` when
    /// the active screen didn't set a title.
    pub title: Option<&'a Buffer>,
    /// `true` when a back chevron should appear in the left
    /// slot. The renderer decides this from stack depth (>=2)
    /// AND the absence of an explicit `header_left` override.
    pub show_back: bool,
    /// User-supplied icon name for the left slot, when set.
    /// `None` falls back to the back chevron (if `show_back`)
    /// or nothing.
    pub header_left_icon: Option<&'b str>,
    pub header_right_icon: Option<&'b str>,
    /// Height of the safe-area strip directly *above* the
    /// header rect. The skin paints the header's background
    /// upward by this amount so the bg slides with the screen
    /// during a push/pop transition (otherwise the status-bar
    /// strip stays empty and visibly "ignores" the slide). The
    /// title + icons still position inside the header rect
    /// itself, not the extended bg.
    pub safe_area_top: f32,
    /// Header bar background — author override from per-screen
    /// or navigator-level style. `None` ⇒ skin default.
    pub background: Option<[f32; 4]>,
    /// Title text color — author override. `None` ⇒ skin default.
    pub title_color: Option<[f32; 4]>,
    /// Icon tint for the back chevron + any
    /// `header_left`/`header_right` icons that don't carry their
    /// own per-button tint. `None` ⇒ skin default.
    pub tint: Option<[f32; 4]>,
}

/// One tappable region of the header bar. Pushed by skins from
/// `paint_navigator_header`; consumed by the host's pointer
/// dispatch when resolving a press inside a navigator's header
/// strip.
#[derive(Copy, Clone, Debug)]
pub struct NavigatorHeaderHit {
    pub rect: (f32, f32, f32, f32),
    pub action: NavigatorHeaderAction,
}

#[derive(Copy, Clone, Debug)]
pub enum NavigatorHeaderAction {
    /// Pop the owning navigator's stack.
    Back,
    /// Fire the screen's `header_left` button's `on_press`.
    HeaderLeft,
    /// Fire the screen's `header_right` button's `on_press`.
    HeaderRight,
}
