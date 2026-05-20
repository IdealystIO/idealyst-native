//! The backend's `Node` type.
//!
//! Each node owns a `LayoutNode` (Taffy handle) and a kind tag with
//! per-kind state (text content, click handler, etc.). Children are
//! tracked here too — Taffy already stores the parent→children
//! relation, but the renderer walks via these direct `Rc` pointers
//! so we don't have to round-trip through Taffy on every frame.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use framework_core::{StateBits, StyleRules, TouchHandler};
use native_layout::LayoutNode;

use crate::style_convert::RenderStyle;

/// Default size for an unstyled `Toggle` (matches UISwitch /
/// `NSSwitch.controlSize = .regular`). Authors can override via
/// `width` / `height` in the stylesheet; this is what the Taffy
/// `set_intrinsic_size` call seeds.
pub const TOGGLE_WIDTH: f32 = 51.0;
pub const TOGGLE_HEIGHT: f32 = 31.0;

/// Apple Human Interface Guidelines minimum hit-target. UIKit
/// expands the touch region around small controls (`UISwitch`,
/// `UISlider`, tiny buttons) to at least this size so users hit
/// them at finger precision rather than pixel precision.
/// We apply the same inflation in our hit-test path.
pub const IOS_MIN_HIT_TARGET: f32 = 44.0;

/// Unconditional touch slop added on every side of an interactive
/// leaf's hit rect, on top of the [`IOS_MIN_HIT_TARGET`] floor.
/// Real iOS apps routinely add a few pixels of slop even when a
/// control is nominally large enough — clicking 1px past the
/// visual edge of a switch should still register. 8pt matches
/// UIKit's typical `hitTest` override patterns.
pub const HIT_SLOP: f32 = 8.0;

/// Distance the pointer must move from its press origin before a
/// tap-press is converted to a scroll pan. Matches iOS
/// `UIScrollView`'s default `panGestureRecognizer` slop.
pub const PAN_THRESHOLD: f32 = 10.0;

/// Width of the scrollbar track / thumb. Matches iOS's overlay
/// scrollbar style.
pub const SCROLLBAR_WIDTH: f32 = 3.0;
/// Inset from the scrollview's trailing edge. iOS-style: hugs
/// the edge with a hairline of breathing room so the thumb's
/// rounded cap isn't cropped at high DPI.
pub const SCROLLBAR_INSET: f32 = 1.0;
/// Minimum thumb length so a very long content extent still
/// shows a tappable / visible thumb.
pub const SCROLLBAR_MIN_THUMB: f32 = 24.0;

/// Momentum-scroll exponential decay rate (per second). Matches
/// the feel of `UIScrollView` between its "Normal" (k≈2) and
/// "Fast" (k≈10) deceleration constants. After one second of
/// coasting the velocity has dropped to `exp(-k) ≈ 8%`.
pub const SCROLL_MOMENTUM_DECAY_PER_SEC: f32 = 2.5;
/// Below this speed (px/sec) the momentum scroll is considered
/// settled and the tick loop ends. Also the threshold for
/// kicking off momentum on lift-off — a release at less than
/// this velocity stays put.
pub const SCROLL_MOMENTUM_MIN_VELOCITY: f32 = 30.0;
/// If the user holds the pointer still for longer than this
/// before releasing, the residual velocity is treated as zero
/// (their finger had settled). Without this guard a long
/// "stop, then lift" gesture would re-fire the velocity from
/// before the stop.
pub const SCROLL_MOMENTUM_STALE_MS: u128 = 80;
/// EMA mix used to smooth raw `delta/dt` samples into a stable
/// pan velocity. Higher = more weight on the latest move.
pub const SCROLL_VELOCITY_SMOOTHING: f32 = 0.6;

/// Rubber-band resistance constant. Smaller values make the
/// overshoot stiffer; larger values let the user drag further
/// past the edge before resistance kicks in. iOS uses a similar
/// `c/(c+d)` saturation curve where `c` is roughly half the
/// viewport — keeping our overshoot subtle.
pub const SCROLL_RUBBERBAND_RESISTANCE: f32 = 0.55;
/// Exponential approach rate (per second) used by the
/// rubber-band spring-back. After 1s the gap to the target has
/// dropped to `exp(-k) ≈ 1.8%` at k=4.
pub const SCROLL_SPRINGBACK_RATE_PER_SEC: f32 = 6.0;
/// Below this distance to target (in px), the spring-back tick
/// snaps to the bound and ends.
pub const SCROLL_SPRINGBACK_EPSILON: f32 = 0.5;

/// Total height of the simulator's on-screen keyboard, in
/// logical px. iOS portrait QWERTY is ~291pt; we round to a
/// clean number that leaves room for content above.
pub const KEYBOARD_HEIGHT: f32 = 280.0;
/// Horizontal margin between the keyboard's edge and the
/// first / last key in a row.
pub const KEYBOARD_SIDE_MARGIN: f32 = 4.0;
/// Vertical padding above the first row and below the last.
pub const KEYBOARD_VERT_MARGIN: f32 = 8.0;
/// Per-row vertical gap between keys.
pub const KEYBOARD_ROW_GAP: f32 = 8.0;
/// Per-row horizontal gap between keys.
pub const KEYBOARD_KEY_GAP: f32 = 6.0;
/// Corner radius on each key's rounded rect.
pub const KEYBOARD_KEY_RADIUS: f32 = 6.0;
/// Font size used for letter keys.
pub const KEYBOARD_KEY_FONT_SIZE: f32 = 18.0;

/// Caret blink period, full on→off→on cycle. Matches iOS's
/// ~1.06 sec UITextField caret rhythm.
pub const CARET_BLINK_PERIOD_SEC: f32 = 1.06;

/// Duration of the keyboard's slide-up / slide-down animation.
/// iOS uses ~250ms with an ease-out curve.
pub const KEYBOARD_ANIM_MS: u32 = 250;
/// Padding above the keyboard when auto-scrolling a focused
/// input into view. Gives the input a little breathing room
/// from the keyboard's top edge.
pub const KEYBOARD_INPUT_MARGIN: f32 = 16.0;
/// Padding between the track edge and the thumb at rest.
pub const TOGGLE_THUMB_INSET: f32 = 2.0;
/// Duration of the toggle's thumb-slide animation. Matches the
/// iOS UISwitch transition.
pub const TOGGLE_ANIM_MS: u32 = 200;

/// Default size for an unstyled `Slider`. Width is a sensible
/// minimum but the slider really wants to flex; authors should
/// set `flex_grow` or an explicit `width` on the slider node.
pub const SLIDER_DEFAULT_WIDTH: f32 = 200.0;
pub const SLIDER_HEIGHT: f32 = 28.0;
/// Thumb diameter. Matches the iOS / macOS slider thumb.
pub const SLIDER_THUMB_SIZE: f32 = 28.0;
/// Track thickness (the visible bar through the middle of the
/// slider's vertical extent).
pub const SLIDER_TRACK_HEIGHT: f32 = 4.0;

/// Default height for a `TextInput` that hasn't been explicitly
/// sized. Authors should still pass a `font_size` (and usually a
/// `padding`/`background`) via style — but a sensible height keeps
/// unstyled inputs visible in early bring-up.
pub const TEXT_INPUT_DEFAULT_HEIGHT: f32 = 36.0;
/// Width of the blinking caret. Half-pixel widths render fuzzy on
/// HiDPI; 1.5 splits the difference cleanly between thinness and
/// visibility.
pub const TEXT_INPUT_CARET_WIDTH: f32 = 1.5;

/// Pixel diameter of an `ActivityIndicatorSize::Small` spinner.
/// Matches `UIActivityIndicatorView.Style.medium` (20pt).
pub const ACTIVITY_INDICATOR_SMALL_SIZE: f32 = 20.0;
/// Pixel diameter of an `ActivityIndicatorSize::Large` spinner.
/// Matches `UIActivityIndicatorView.Style.large` (37pt rounded
/// down to keep dot-on-orbit math integer-clean).
pub const ACTIVITY_INDICATOR_LARGE_SIZE: f32 = 36.0;
/// One full rotation of the spinner's leading dot. Matches the
/// UIKit medium-size spinner — 12 ticks ≈ 1 Hz.
pub const ACTIVITY_INDICATOR_SPIN_PERIOD_SEC: f32 = 1.0;

/// Default intrinsic size for an unstyled `Image` — gives the
/// placeholder visible bulk before a real texture pipeline lands.
pub const IMAGE_DEFAULT_SIZE: f32 = 96.0;
/// Default square size for an unstyled `Icon`. Authors usually
/// constrain it via styles; this is what an unstyled icon looks
/// like.
pub const ICON_DEFAULT_SIZE: f32 = 24.0;
/// Height of a tab navigator's tab bar (the strip along the
/// bottom of a tabs screen). Matches the iOS UITabBar default.
pub const TAB_BAR_HEIGHT: f32 = 49.0;
/// Height of a navigator's header bar.
pub const NAV_HEADER_HEIGHT: f32 = 44.0;
/// Width of the drawer sidebar when fully open, as a fraction of
/// the viewport width. iOS UIKit drawers use ~80%.
pub const DRAWER_WIDTH_RATIO: f32 = 0.78;
/// Drawer slide-in / slide-out duration in milliseconds.
pub const DRAWER_ANIM_MS: u32 = 250;
/// Maximum alpha of the scrim painted behind an open drawer.
/// Material guidelines call for 32% (`0x52`) under the scrim;
/// matches both iOS and Android drawer chrome closely.
pub const DRAWER_SCRIM_MAX_ALPHA: f32 = 0.32;
/// Default height for an `Unsupported` placeholder so authors
/// can see the "X not supported" panel without explicit sizing.
pub const UNSUPPORTED_DEFAULT_HEIGHT: f32 = 80.0;

/// Duration of the visual flash applied to a tapped keyboard
/// key. Mirrors iOS's brief depress-then-release animation —
/// long enough to register at typing cadence, short enough to
/// not overlap consecutive keystrokes.
pub const KEY_PRESS_FLASH_MS: u32 = 120;

/// Public alias used by the `Backend` impl's associated type.
pub type WgpuNode = Rc<RefCell<NodeData>>;

/// One mounted route in a tab or drawer navigator. The
/// dispatcher uses `name` to match `NavCommand::Select`
/// targets; `scope_id` is the framework scope created when
/// the route was first mounted, so `release_screen(id)` can
/// fire on unmount.
#[derive(Clone, Debug)]
pub struct TabRoute {
    pub name: &'static str,
    pub scope_id: u64,
}

/// In-flight push/pop animation on a stack navigator. The
/// dispatcher seeds this when a Push or Pop command fires; the
/// renderer samples it each frame to translate the top two
/// screens; the host's tick advances it (and, on completion of
/// a Pop, fires the deferred `release_screen` + `drop_subtree`
/// that the synchronous Pop deferred so the popping subtree
/// could stay on-screen during the slide).
pub struct NavTransition {
    pub kind: NavTransitionKind,
    pub start: web_time::Instant,
}

pub enum NavTransitionKind {
    /// New screen sliding in from the right. The screen is
    /// already mounted as the navigator's last child; the
    /// renderer translates it by `(1 - progress) * width`.
    Push,
    /// Top screen sliding out to the right. Still mounted as
    /// the navigator's last child for the duration of the
    /// animation; the under-screen (the new top after pop) is
    /// the second-to-last child. On completion the popping
    /// node is dropped + its scope released.
    Pop {
        popping_scope_id: u64,
        release_screen: Rc<dyn Fn(u64)>,
    },
}

/// Per-node kind discriminant + payload.
pub enum NodeKind {
    View,
    Text {
        content: String,
    },
    Pressable {
        on_click: Rc<dyn Fn()>,
    },
    Button {
        label: String,
        on_click: Rc<dyn Fn()>,
    },
    /// Editable single-line text input. The framework owns the
    /// authoritative value via a `Signal<String>`; the backend
    /// fires `on_change` on each native edit and the framework
    /// pushes value updates back through `update_text_input_value`.
    /// `placeholder` is shown when `value` is empty.
    TextInput {
        value: String,
        placeholder: Option<String>,
        on_change: Rc<dyn Fn(String)>,
    },
    /// On/off switch. Same controlled-component pattern as `TextInput`.
    Toggle {
        value: bool,
        on_change: Rc<dyn Fn(bool)>,
    },
    /// Continuous slider with optional step. Step is enforced when
    /// the user drags by rounding the candidate value to
    /// `min + step * round((v - min) / step)`. `None` = freely
    /// continuous.
    Slider {
        value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    },
    /// Scrolling container. `horizontal=false` scrolls vertically;
    /// `true` scrolls horizontally. The current scroll position
    /// lives here and is mutated by the host's `scroll` event
    /// dispatch. Children are laid out by Taffy at their natural
    /// sizes (no main-axis constraint from this node) and the
    /// renderer translates them by `-offset` when painting.
    ScrollView {
        horizontal: bool,
        offset_x: f32,
        offset_y: f32,
    },
    /// Stable parent for reactive `when`/`switch` branch swaps.
    /// Same shape as a View; named separately so the renderer can
    /// treat it as layout-transparent later if needed.
    ReactiveAnchor,
    /// Indeterminate loading spinner. `size` selects the diameter
    /// at construction (matches `UIActivityIndicatorView.Style`).
    /// `color` is the author's tint override — `None` means use
    /// the platform default (iOS systemGray, M3 primary).
    ActivityIndicator {
        size: framework_core::primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<[f32; 4]>,
    },
    /// Navigable link — text + on-activate callback. Same
    /// interaction shape as a Pressable; held as its own kind
    /// so future enhancements (URL preview, etc.) can branch.
    Link {
        on_activate: Rc<dyn Fn()>,
    },
    /// Bitmap image. The simulator renders a placeholder rect
    /// with the alt text — a real texture pipeline is future
    /// work. `src` and `alt` are kept so that placeholder can
    /// surface what would have been loaded.
    Image {
        src: String,
        alt: Option<String>,
    },
    /// Vector icon. Rendered as a small placeholder square
    /// stamped with the icon's color until path/SDF rendering
    /// lands. `color` defaults to `style.color` if unset.
    Icon {
        /// SVG path `d` strings. Lucide's icons are stroke-only;
        /// the renderer parses each path into line segments and
        /// strokes them with capsule rects.
        paths: &'static [&'static str],
        /// viewBox `(width, height)` in design units. Path
        /// coords are in this space; the renderer scales them
        /// onto the icon's actual screen rect.
        view_box: (u16, u16),
        /// Author-set tint. `None` falls back to the node's
        /// text color so an icon inside a colored label gets
        /// the same hue.
        color: Option<[f32; 4]>,
        /// Stroke-reveal progress in `[0.0, 1.0]`. The renderer
        /// paints strokes whose accumulated length is below
        /// `progress * total_path_length`; segments past the
        /// threshold are skipped, with the boundary segment
        /// painted partially. Driven by `update_icon_stroke` /
        /// `animate_icon_stroke`. Default 1.0 = fully drawn.
        stroke_progress: std::cell::Cell<f32>,
    },
    /// Top-of-stack portal. The renderer hoists Portal subtrees
    /// to a top z-layer after the main walk so they paint above
    /// regular content. Positioning is derived from `target`:
    /// [`PortalTarget::Viewport`] uses the embedded placement
    /// against the full window; [`PortalTarget::Anchor`] re-queries
    /// the anchor's rect each frame (cheap because we re-render
    /// every frame anyway); [`PortalTarget::Named`] is unsupported
    /// in this backend. Backdrop is no longer a backend concern —
    /// the composition layer (`framework_core::primitives::overlay`)
    /// emits a backdrop primitive as a child of the portal, so it
    /// just flows through the regular walk.
    Portal {
        target: framework_core::primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
    },
    /// Virtualizer container. The simulator mounts every item
    /// eagerly (no actual windowing) — fine for the moderate
    /// list sizes a smoke preview uses.
    Virtualizer {
        horizontal: bool,
        /// `mount_item(idx) -> (node, scope_id)`. Kept so
        /// `virtualizer_data_changed` can re-mount items when
        /// the data signal updates.
        mount_item: Rc<dyn Fn(usize) -> (WgpuNode, u64)>,
        /// `release_item(scope_id)`. Called for every removed
        /// item during the rebuild — drops the framework scope.
        release_item: Rc<dyn Fn(u64)>,
        /// `item_count()` — fresh at-call value so rebuilds
        /// don't see stale snapshots.
        item_count: Rc<dyn Fn() -> usize>,
        /// Scope ids for currently-mounted items, in insertion
        /// order. Parallel to `NodeData.children`.
        scope_ids: std::cell::RefCell<Vec<u64>>,
    },
    /// Stack-based navigator. The renderer paints only the
    /// last child (top of stack); older screens stay mounted
    /// for back-navigation but are clipped out of the visible
    /// area. `scope_ids` records the framework scope id for
    /// each mounted screen so `release_screen` can be called
    /// with the right id on pop / replace / reset.
    /// `control` is the `NavigatorControl` the framework handed
    /// us at `create_navigator` time, kept so
    /// `make_navigator_handle` can wire the user-facing
    /// `NavigatorHandle` to the same control (otherwise calls
    /// like `handle.push(...)` reach a no-op stub).
    Navigator {
        scope_ids: std::cell::RefCell<Vec<u64>>,
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
        /// Current in-flight push/pop animation, or `None` when
        /// the navigator is at rest. Sampled by the renderer's
        /// Navigator branch; advanced + cleared by the host's
        /// `tick_nav_transitions`.
        transition: std::cell::RefCell<Option<NavTransition>>,
        /// Animator deciding how a push/pop slides — under +
        /// top translates, duration, easing. Seeded with the
        /// crate-wide default ([`crate::nav_anim::default_transition`])
        /// at create time; future builder API can swap it per
        /// navigator (slide vs. modal-up vs. fade). The same
        /// `Rc` is sampled every frame, so cloning is cheap.
        transition_anim: Rc<dyn crate::nav_anim::ScreenTransition>,
        /// Navigator-level chrome styles set via
        /// `.header_style(...)` / `.title_style(...)` /
        /// `.button_style(...)` on the framework's `Navigator`
        /// builder. Each screen's own `ScreenOptions` (title
        /// color, header background, …) merges on top. The
        /// renderer reads these every frame so theme swaps
        /// repaint the header in lockstep with content.
        header_style: std::cell::RefCell<Option<Rc<framework_core::StyleRules>>>,
        title_style: std::cell::RefCell<Option<Rc<framework_core::StyleRules>>>,
        button_style: std::cell::RefCell<Option<Rc<framework_core::StyleRules>>>,
        /// Style for the body area below the header — shows
        /// through any transparent regions in the active
        /// screen's content. Currently stored but unread by the
        /// renderer (screens paint full-bleed by default); kept
        /// so the framework's `.body_style(...)` builder call
        /// doesn't silently drop the value.
        body_style: std::cell::RefCell<Option<Rc<framework_core::StyleRules>>>,
    },
    /// Tab navigator. The active tab index decides which child
    /// is painted; non-active tabs stay mounted (or get released
    /// per mount-policy — that's the framework's call). A
    /// platform-skinned tab bar is painted at the bottom of the
    /// navigator's rect. `routes` parallel-tracks each mounted
    /// tab's route name + scope id so `Select` can find / mount
    /// the right tab.
    TabNavigator {
        active_tab: std::cell::Cell<usize>,
        tab_count: std::cell::Cell<usize>,
        routes: std::cell::RefCell<Vec<TabRoute>>,
        /// The framework's `NavigatorControl` for this tab nav.
        /// Kept so `make_tab_navigator_handle` can wire the
        /// user-facing `TabsHandle` to the same control the
        /// installed dispatcher subscribes to — otherwise
        /// `handle.select(...)` would dispatch into a no-op
        /// default handle.
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
        /// Tab-bar chrome styles. `bar_style` is read at paint
        /// time by the renderer's `paint_tab_bar`; the icon /
        /// label styles are stored for future use (the wgpu
        /// sim's tab bar paints abstract dots, not icons +
        /// labels, so those don't yet have visible effect).
        bar_style: std::cell::RefCell<Option<Rc<framework_core::StyleRules>>>,
        icon_style: std::cell::RefCell<Option<Rc<framework_core::StyleRules>>>,
        label_style: std::cell::RefCell<Option<Rc<framework_core::StyleRules>>>,
    },
    /// Drawer navigator. `is_open` controls the slide-in
    /// state; the renderer animates `sidebar_offset` via the
    /// host's animator just like the on-screen keyboard.
    /// `routes` tracks the body screens (one per drawer item)
    /// the same way `TabNavigator.routes` does.
    DrawerNavigator {
        is_open: Rc<std::cell::Cell<bool>>,
        active_screen: std::cell::Cell<usize>,
        routes: std::cell::RefCell<Vec<TabRoute>>,
        /// The sidebar subtree, attached via
        /// `drawer_navigator_attach_sidebar`. `None` until then.
        sidebar: std::cell::RefCell<Option<WgpuNode>>,
        /// `NavigatorControl` for this drawer. Same reason as
        /// the tab variant: lets `make_drawer_navigator_handle`
        /// produce a `DrawerHandle` whose `open_drawer()` /
        /// `toggle_drawer()` calls reach the installed
        /// dispatcher.
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
        /// Wall-clock at which the most recent open/close edge
        /// fired. Sampled by the renderer to interpolate the
        /// sidebar's slide-in / slide-out — the drawer's
        /// position is `lerp(closed, open, elapsed/duration)`
        /// where direction comes from `is_open`. `None` means no
        /// transition currently in flight; sidebar is resting at
        /// its current `is_open` extreme.
        anim_started_at: std::cell::Cell<Option<web_time::Instant>>,
        /// Drawer chrome styles. `scrim_style.background` is
        /// read at paint time by `paint_drawer_overlay` to
        /// override the default 32%-black scrim. `sidebar_style`
        /// is stored for future use (the sidebar is a user-built
        /// subtree that styles itself; this hook would supply
        /// an outer wrap around it).
        scrim_style: std::cell::RefCell<Option<Rc<framework_core::StyleRules>>>,
        sidebar_style: std::cell::RefCell<Option<Rc<framework_core::StyleRules>>>,
    },
    /// An author-driven render-to-texture region. The wgpu
    /// backend allocates a per-node offscreen texture (created
    /// lazily in the renderer once we know the node's size) and
    /// hands the texture view to a user-supplied closure each
    /// frame; the main UI walk samples the texture and composites
    /// it as a textured quad through the existing image pipeline.
    ///
    /// The framework's cross-platform `Graphics` primitive ships
    /// a `GraphicsSurface` to authors via `OnReady`; that contract
    /// assumes a real OS-window handle which we can't satisfy for
    /// a sub-region. Authors targeting this backend register the
    /// drawer via [`crate::register_graphics_drawer`] after
    /// constructing the primitive instead.
    Graphics {
        /// User's per-frame draw closure. None until the author
        /// calls `register_graphics_drawer` on the node's handle.
        /// `RefCell` because the closure is invoked from the
        /// renderer (which only holds an immutable backend borrow)
        /// — the cell promotes that to a mutable closure call
        /// without cloning the closure each frame.
        drawer: std::cell::RefCell<Option<GraphicsDrawer>>,
        /// Wall-clock at which the node was created. The renderer
        /// hands `elapsed = now - created_at` to the drawer so
        /// animations keep a stable origin even if the user
        /// remounts the node (each remount gets its own zero).
        created_at: web_time::Instant,
    },
    /// Video playback node. The wgpu preview decodes H.264 mp4
    /// files in-process via `openh264` + `re_mp4` (no system
    /// FFmpeg dep). The decoder runs on its own thread and posts
    /// the latest decoded RGBA frame into a shared cell; the
    /// renderer's pre-pass uploads that frame to a wgpu texture
    /// and the main UI walk composites it through the image
    /// pipeline. Play / pause / seek route from the framework's
    /// `VideoHandle` to the decoder thread.
    Video {
        /// The owning decoder (`Drop` joins its thread). Wrapped
        /// in `Rc` so the `VideoHandle` ops can read the shared
        /// state without going through `NodeData`.
        decoder: std::rc::Rc<crate::video::VideoDecoder>,
        /// Author opted in via `.controls(true)` — the renderer
        /// paints a play/pause + scrubber bar over the texture,
        /// and pointer routing dispatches clicks against the
        /// cached sub-rects.
        controls: bool,
        /// `Instant` of the last pointer move over the video.
        /// Drives the hover-fade — `None` means "not hovered";
        /// `Some(t)` keeps controls visible for ~2 s past `t`,
        /// then fades out. Always-on when the decoder is paused
        /// so the user can still grab the scrubber when stopped.
        last_hover: std::cell::Cell<Option<std::time::Instant>>,
        /// Sub-rects the renderer cached on the last frame so
        /// pointer routing can hit-test without re-deriving them.
        /// All zero before the first render with controls on.
        play_btn_rect: std::cell::Cell<(f32, f32, f32, f32)>,
        scrubber_rect: std::cell::Cell<(f32, f32, f32, f32)>,
        /// World-space rect of the video frame itself, cached
        /// during walk so the pointer pipeline can hover-test
        /// without re-running layout.
        frame_rect: std::cell::Cell<(f32, f32, f32, f32)>,
    },
    /// Renders a "not supported in this simulator" panel for
    /// primitives we don't implement (WebView, Video, Graphics).
    /// Keeps an app that uses them visibly intact instead of
    /// rendering a 0×0 invisible node.
    Unsupported {
        label: &'static str,
    },
}

/// A user-supplied draw closure for a Graphics node. Invoked
/// once per frame with the shared wgpu device + queue and the
/// node's offscreen render target view; the closure encodes
/// its own render pass(es) against `view`. See
/// [`crate::register_graphics_drawer`] for how authors wire
/// this onto a `GraphicsHandle`.
pub type GraphicsDrawer = Box<dyn FnMut(&mut GraphicsFrame)>;

/// Per-frame state handed to a [`GraphicsDrawer`]. Borrowed
/// fields point at GPU resources the host owns; the closure
/// reads them, encodes draw commands, and returns — no
/// `present()` call needed (the host composites the resulting
/// texture into the UI walk itself).
pub struct GraphicsFrame<'a> {
    /// Shared wgpu device (same one the host uses for UI
    /// rendering). Authors reuse it for their own pipelines,
    /// buffers, and shaders — no second adapter / device
    /// initialization required.
    pub device: &'a wgpu::Device,
    /// Shared command queue. Use `queue.write_buffer(...)` for
    /// uniform / per-frame data; don't `submit` directly — the
    /// host owns the submit and orders Graphics passes ahead of
    /// the main UI pass.
    pub queue: &'a wgpu::Queue,
    /// The Graphics node's offscreen render target view. Always
    /// a `Rgba8UnormSrgb` 2D texture sized to the node's current
    /// pixel-space frame.
    pub view: &'a wgpu::TextureView,
    /// Same encoder the host uses for the rest of the frame.
    /// Authors call `encoder.begin_render_pass(...)` to draw.
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// Drawable size in physical pixels. Matches `view`'s extent
    /// for the current frame; resizes happen at frame boundaries
    /// so this is always coherent with `view`.
    pub size: (u32, u32),
    /// Wall-clock duration since the Graphics node was created.
    /// Convenient driver for procedural animations.
    pub elapsed: std::time::Duration,
}

impl std::fmt::Debug for NodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeKind::View => f.write_str("View"),
            NodeKind::Text { content } => write!(f, "Text({content:?})"),
            NodeKind::Pressable { .. } => f.write_str("Pressable"),
            NodeKind::Button { label, .. } => write!(f, "Button({label:?})"),
            NodeKind::TextInput { value, .. } => write!(f, "TextInput({value:?})"),
            NodeKind::Toggle { value, .. } => write!(f, "Toggle({value})"),
            NodeKind::Slider { value, min, max, .. } => {
                write!(f, "Slider({value} in {min}..={max})")
            }
            NodeKind::ScrollView { horizontal, offset_x, offset_y } => {
                write!(
                    f,
                    "ScrollView(horizontal={horizontal}, offset={offset_x},{offset_y})"
                )
            }
            NodeKind::ReactiveAnchor => f.write_str("ReactiveAnchor"),
            NodeKind::ActivityIndicator { size, .. } => {
                write!(f, "ActivityIndicator({size:?})")
            }
            NodeKind::Link { .. } => f.write_str("Link"),
            NodeKind::Image { src, .. } => write!(f, "Image({src:?})"),
            NodeKind::Icon { .. } => f.write_str("Icon"),
            NodeKind::Portal { .. } => f.write_str("Portal"),
            NodeKind::Virtualizer { horizontal, .. } => {
                write!(f, "Virtualizer(horizontal={horizontal})")
            }
            NodeKind::Navigator { scope_ids, .. } => {
                write!(f, "Navigator(depth={})", scope_ids.borrow().len())
            }
            NodeKind::TabNavigator { active_tab, tab_count, .. } => write!(
                f,
                "TabNavigator(active={}, count={})",
                active_tab.get(),
                tab_count.get()
            ),
            NodeKind::DrawerNavigator { is_open, .. } => {
                write!(f, "DrawerNavigator(open={})", is_open.get())
            }
            NodeKind::Graphics { drawer, .. } => write!(
                f,
                "Graphics(drawer={})",
                if drawer.borrow().is_some() { "set" } else { "unset" },
            ),
            NodeKind::Video { .. } => f.write_str("Video"),
            NodeKind::Unsupported { label } => write!(f, "Unsupported({label})"),
        }
    }
}

impl std::fmt::Debug for NodeData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeData")
            .field("kind", &self.kind)
            .field("layout", &self.layout)
            .field("children", &self.children.len())
            .field("has_style", &self.style.is_some())
            .finish()
    }
}

pub struct NodeData {
    pub kind: NodeKind,
    /// Taffy node handle. Always set — even text nodes get one,
    /// with a measure function installed.
    pub layout: LayoutNode,
    /// Direct child pointers, in insertion order. The renderer uses
    /// this to walk the tree front-to-back without going through
    /// Taffy. The Taffy tree mirrors this hierarchy.
    pub children: Vec<WgpuNode>,
    /// The framework's most recently applied style. Held so the
    /// renderer can re-read paint properties (background, borders,
    /// opacity, …) on every frame without re-deriving them.
    pub style: Option<Rc<StyleRules>>,
    /// Cached render-time projection of `style` — concrete colors,
    /// resolved border widths, etc. Rebuilt by `apply_style`.
    pub render: RenderStyle,
    /// State-bits setter installed by the framework's
    /// `attach_states` hook. Present only on nodes whose stylesheet
    /// declares one or more `state {hovered,pressed,focused,disabled}`
    /// overlays. The host calls
    /// `setter(StateBits::PRESSED, true|false)` from press tracking;
    /// the framework re-resolves the style and pushes the result
    /// through `apply_style`. Unused state bits are no-ops.
    pub state_setter: Option<Rc<dyn Fn(StateBits, bool)>>,
    /// Raw touch handler installed by
    /// [`framework_core::Backend::install_touch_handler`]. Present
    /// only on nodes whose primitive carries an `on_touch` slot. The
    /// host's pointer dispatch resolves the responder chain by
    /// collecting, during hit-test, every ancestor whose
    /// `touch_handler` is `Some` and invoking them deepest-first
    /// until one returns `consumed: true`.
    pub touch_handler: Option<TouchHandler>,
    /// Set when this node is the root of a screen mounted into a
    /// `Navigator`. Forces the node to fill its navigator's rect
    /// regardless of what styles the screen author set on the
    /// outer view — matches the iOS / Android contract where a
    /// pushed VC fills the navigation controller's bounds. The
    /// flag is sticky across `apply_style` re-applies (theme
    /// swap, reactive style flips) so the fill survives.
    pub navigator_screen: bool,
    /// Per-screen header config: title, button slots, header
    /// background / tint / title color closures. Populated by
    /// the navigator's attach methods from the framework's
    /// `MountResult.options`. Stays `None` for non-screen nodes.
    /// Boxed because `ScreenOptions` contains six `Rc<dyn Fn>`s
    /// plus several `Option<String>`s — bulky for nodes that
    /// don't carry one.
    pub screen_options: Option<Box<framework_core::primitives::navigator::ScreenOptions>>,
    /// Identifier of the stack `Navigator` this screen belongs
    /// to. Lets the renderer find the navigator's chrome styles
    /// (header_style etc.) when painting the screen's header and
    /// lets the host's pointer dispatch route header-bar taps to
    /// the right `NavigatorControl` for `pop()` or open-drawer.
    /// `None` for screens of tab / drawer navigators (header
    /// chrome of those kinds is owned by the navigator itself,
    /// not the per-screen options shipped through `attach_initial`).
    ///
    /// Held as `Weak` to avoid a parent↔child Rc cycle: the
    /// navigator's `children` Vec already strongly owns this
    /// screen, so a strong back-pointer here would keep both
    /// alive forever — leaking the navigator's per-screen `scopes`
    /// (and every `StyleHandle` / theme-cohort entry inside them)
    /// past `release_screen` + `drop_subtree`.
    pub owning_navigator: Option<Weak<RefCell<NodeData>>>,
    /// Free-standing Taffy node used as a key into `TextStore`
    /// for this screen's header title buffer. Allocated when the
    /// screen has `ScreenOptions.title = Some(_)`. Not a child
    /// of any Taffy parent — pure handle storage so the renderer
    /// can fetch the pre-shaped glyph buffer each frame without
    /// reshaping.
    pub screen_title_layout: Option<LayoutNode>,
}

pub fn new_node(kind: NodeKind, layout: LayoutNode) -> WgpuNode {
    Rc::new(RefCell::new(NodeData {
        kind,
        layout,
        children: Vec::new(),
        navigator_screen: false,
        screen_options: None,
        owning_navigator: None,
        screen_title_layout: None,
        style: None,
        render: RenderStyle::default(),
        state_setter: None,
        touch_handler: None,
    }))
}
