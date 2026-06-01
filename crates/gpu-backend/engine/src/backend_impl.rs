//! The `Backend` trait impl for `WgpuBackend`.
//!
//! Builds and mutates the node tree + Taffy layout tree. Text state
//! (glyphon buffers + the shared `FontSystem`) lives in two shared
//! `Rc<RefCell<>>`s so this module can mutate them inline from
//! Backend methods while the renderer and Taffy measure closures
//! reach the same data through their own clones.
//!
//! The renderer (see [`crate::app`]) walks the tree on each frame
//! and submits draws — the Backend itself never talks to wgpu.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
// `web-time` for wasm32 compat — see `host.rs` for the rationale.
use web_time::Instant;

use runtime_core::accessibility::{
    default_role, AccessibilityNode, AccessibilityProps, AccessibilityRect, AccessibilityTree,
    LiveRegionPriority, PrimitiveKind, Role,
};
use runtime_core::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_core::{Action, Backend, Color, ColorScheme, Easing, StateBits, StyleRules, Tokenized};
use glyphon::FontSystem;
use runtime_layout::{AvailableSpace, LayoutNode, LayoutTree, Size as TaffySize};

use crate::animation::{AnimProperty, Animator, TweenKey};
use crate::node::{
    new_node, NodeData, NodeKind, WgpuNode, ACTIVITY_INDICATOR_LARGE_SIZE,
    ACTIVITY_INDICATOR_SMALL_SIZE, ICON_DEFAULT_SIZE, IMAGE_DEFAULT_SIZE,
    SLIDER_DEFAULT_WIDTH, SLIDER_HEIGHT, TEXT_AREA_DEFAULT_HEIGHT,
    TEXT_INPUT_DEFAULT_HEIGHT, TOGGLE_ANIM_MS, TOGGLE_HEIGHT, TOGGLE_WIDTH,
};
use crate::style_convert::parse_color;
use crate::scheduler::request_redraw;
use crate::text::TextStore;

pub struct WgpuBackend {
    pub(crate) layout: LayoutTree,
    /// Root node of the rendered tree. The framework's
    /// `render(...)` calls `finish(root)` once the build walker has
    /// emitted every create/insert/apply_style for the tree; we
    /// remember the root there so the renderer can find it.
    pub(crate) roots: Vec<WgpuNode>,
    /// Shared text-buffer store. Cloned into Taffy measure closures
    /// and read by the renderer's frame walk.
    pub(crate) text: Rc<RefCell<TextStore>>,
    /// Shared font system. Same shape and reason as `text`.
    pub(crate) font_system: Rc<RefCell<FontSystem>>,
    /// Tween engine driving native-widget animations (toggle
    /// slide, future slider snap, button press scale, …). Owned
    /// here so any backend method can start a tween; sampled by
    /// the renderer in `walk()`; ticked by the host before each
    /// frame.
    pub(crate) animator: Animator,
    /// Color scheme reported to the app on init. Variants override
    /// via the constructor.
    pub(crate) color_scheme: ColorScheme,
    /// Count of live `ActivityIndicator` nodes in the tree.
    /// Incremented in [`Backend::create_activity_indicator`],
    /// decremented in [`drop_subtree`]. The host's `tick` returns
    /// `true` while this is non-zero so spinners keep spinning.
    pub(crate) active_spinner_count: u32,
    /// Active skin — held here so `apply_style` can merge
    /// platform defaults (button visuals, etc.) under the
    /// author's stylesheet without a round-trip through the
    /// host. The renderer holds its own clone; both stay in
    /// sync because changing the skin requires a full
    /// re-render.
    pub(crate) skin: Rc<dyn crate::painter::Painter>,
    /// Weak reference to *this* backend's outer `Rc<RefCell<Self>>`.
    /// Set once by `Host::new` immediately after the backend Rc
    /// is constructed. Lets navigator + tab + drawer command
    /// dispatchers (which run from user code outside the
    /// framework's borrow window) re-acquire a mutable borrow
    /// to insert / remove screens without re-entering the
    /// framework's build walker.
    pub(crate) self_weak: std::cell::OnceCell<std::rc::Weak<RefCell<WgpuBackend>>>,
    /// One-shot live-region announcements queued by
    /// [`Backend::announce_for_accessibility`]. The host shell
    /// (winit shell on desktop, future AppKit / UIKit / AT-SPI
    /// wgpu hosts) drains the queue via
    /// [`WgpuBackend::drain_pending_announcements`] on its next
    /// layout-commit pass and posts each entry to the platform's
    /// announcement API (NSAccessibility, UIAccessibility,
    /// `aria-live`).
    ///
    /// Not embedded in [`AccessibilityTree`] because that struct
    /// is the persistent semantics tree (queryable any time for
    /// AX-walker focus resolution); announcements are transient
    /// fire-and-forget messages. The two have different drain
    /// lifetimes, so they live on separate getters.
    ///
    /// **Host shell consumer**: no winit-side AX bridge crate
    /// exists yet — this is GPU-backend prep work. See
    /// `docs/accessibility-design.md` §5 for the projection
    /// contract the future host shell must follow.
    pub(crate) pending_announcements: Vec<(String, LiveRegionPriority)>,
    /// Active per-node presence tweens. Keyed by node pointer
    /// (stable for the node's lifetime). Each tween interpolates
    /// the four animatable presence properties (opacity,
    /// translate_x, translate_y, scale) from a captured "from"
    /// state to a target "to" state over `duration` with `easing`.
    /// [`tick_presence_tweens`] (called from `host::tick`)
    /// advances each tween, writes intermediate values to the
    /// node's `AnimatedOverrides`, and drops finished entries.
    pub(crate) presence_tweens: std::collections::HashMap<usize, PresenceTween>,
    /// `Position::Sticky` bookkeeping. Keyed by sticky-node
    /// pointer (`Rc::as_ptr(node) as usize`). Each entry carries
    /// the pin threshold, the enclosing scroll view's Taffy id
    /// (or `None` if there's no scrolling ancestor), and the
    /// cached natural-y in the scroll view's content space.
    ///
    /// Populated by [`Backend::apply_style`] when a node's
    /// `position` becomes `Sticky`; drained by [`drop_subtree`]
    /// when the node is removed; refreshed (natural-y values
    /// only) by [`crate::sticky::refresh_layout_positions`] after
    /// every Taffy compute. The render walker consults this
    /// registry per-node to decide whether to apply a pin
    /// translate. See `crate::sticky` for the full design.
    pub(crate) sticky_registry: crate::sticky::StickyRegistry,
    /// Cached image-asset bytes keyed by `AssetId`. Populated by
    /// `register_asset` for `AssetTag::Image`. The renderer
    /// resolves `asset://{id}` image sources by looking up the
    /// bytes here and decoding through the same `image` crate
    /// path as filesystem sources — same mount surface as iOS /
    /// Android / macOS, which all store the decoded image keyed
    /// by `AssetId` in their per-backend `ImageCache`.
    pub(crate) image_asset_bytes:
        std::collections::HashMap<runtime_core::AssetId, Vec<u8>>,
    /// Served-file URLs of `Bundled` fonts the app registered but
    /// whose bytes aren't in the binary (i.e. `embed-font-bytes` is
    /// off — the web host path). `register_asset` pushes `/{path}`
    /// here; the async host (`host-web`) drains this via
    /// [`WgpuBackend::drain_pending_font_urls`] after mount, fetches
    /// each file, and feeds the bytes to cosmic-text. Stays empty on
    /// native, where `face!` carries the bytes inline (`BundledEmbedded`)
    /// and they're loaded synchronously below.
    pub(crate) pending_font_urls: Vec<String>,
    /// Third-party `Element::External` registry. Populated by
    /// per-platform leaf crates at app bootstrap. wgpu apps wire
    /// WebView / Maps / etc. by calling
    /// `backend.register_external::<T, _>(handler)` on the wgpu
    /// engine — the handler renders into the same engine surface
    /// (no native overlay yet; the overlay-per-host story remains
    /// pending). Same registry shape iOS / Android / macOS use.
    pub(crate) external_handlers: runtime_core::ExternalRegistry<WgpuBackend>,
    /// Registry of `Element::Navigator` handler factories. SDK
    /// leaves (`stack_navigator`, `tab_navigator`, `drawer_navigator`)
    /// call `register_navigator::<TheirPresentation, _>(factory)` at
    /// bootstrap; `create_navigator` resolves the matching factory
    /// and runs `init`, stashing the handler in
    /// `nav_handler_instances` for follow-up dispatch.
    pub(crate) navigator_handlers:
        runtime_core::NavigatorRegistry<WgpuBackend>,
    /// Per-navigator-instance handler, keyed by the
    /// `WgpuNode`'s pointer. Subsequent trait methods
    /// (`navigator_attach_initial`, `release_navigator`,
    /// `make_navigator_handle`, `apply_navigator_slot_style`) look
    /// the handler up here and delegate.
    pub(crate) nav_handler_instances: std::collections::HashMap<
        usize,
        std::rc::Rc<
            std::cell::RefCell<Box<dyn runtime_core::NavigatorHandler<WgpuBackend>>>,
        >,
    >,
}

/// Per-node presence interpolation entry. `node` is a strong ref so
/// the tween survives even if the rest of the framework drops its
/// last handle mid-animation — the framework drives drop via
/// `clear_children`, which only fires after the exit animation's
/// scheduled task completes (see [`crate::primitives::presence`]'s
/// walker). The tween's lifetime is therefore upper-bounded by the
/// duration, after which `tick_presence_tweens` removes the entry
/// and drops the strong ref.
pub(crate) struct PresenceTween {
    pub(crate) node: WgpuNode,
    pub(crate) from: PresenceSnapshot,
    pub(crate) to: PresenceSnapshot,
    pub(crate) started: Instant,
    pub(crate) duration: std::time::Duration,
    pub(crate) easing: Easing,
}

/// Concrete (non-`Option`) snapshot of the four animatable presence
/// properties. `PresenceState` uses `Option` so authors can declare
/// "only opacity changes"; the tween path resolves the `None`s to
/// each field's identity value (1.0 / 0.0 / 1.0) before recording
/// `from` / `to` so the interpolator doesn't need to special-case
/// missing fields per frame.
#[derive(Copy, Clone, Debug)]
pub(crate) struct PresenceSnapshot {
    pub(crate) opacity: f32,
    pub(crate) translate_x: f32,
    pub(crate) translate_y: f32,
    pub(crate) scale: f32,
}

impl PresenceSnapshot {
    /// "Rest" identity — the rendered values when no presence
    /// override is active.
    pub(crate) fn rest() -> Self {
        Self {
            opacity: 1.0,
            translate_x: 0.0,
            translate_y: 0.0,
            scale: 1.0,
        }
    }
}

impl WgpuBackend {
    pub fn new(
        text: Rc<RefCell<TextStore>>,
        font_system: Rc<RefCell<FontSystem>>,
        color_scheme: ColorScheme,
        skin: Rc<dyn crate::painter::Painter>,
    ) -> Self {
        Self {
            layout: LayoutTree::new(),
            roots: Vec::new(),
            text,
            font_system,
            animator: Animator::new(),
            color_scheme,
            active_spinner_count: 0,
            skin,
            self_weak: std::cell::OnceCell::new(),
            pending_announcements: Vec::new(),
            presence_tweens: std::collections::HashMap::new(),
            sticky_registry: crate::sticky::StickyRegistry::new(),
            image_asset_bytes: std::collections::HashMap::new(),
            pending_font_urls: Vec::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
            navigator_handlers: runtime_core::NavigatorRegistry::new(),
            nav_handler_instances: std::collections::HashMap::new(),
        }
    }

    /// Register a handler for the third-party external primitive
    /// whose payload type is `T`. Called by per-platform leaf crates
    /// (e.g. a future `webview-wgpu`, `maps-wgpu`) at app bootstrap.
    /// The handler receives the typed payload + a mutable borrow of
    /// the backend and produces the `WgpuNode` to mount. Mirrors
    /// `IosBackend::register_external` / `MacosBackend::register_external`.
    pub fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&std::rc::Rc<T>, &mut WgpuBackend) -> WgpuNode + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }

    /// Register a `Element::Navigator` handler factory keyed by
    /// presentation type `P`. SDK leaf crates call this once at
    /// bootstrap. Mirrors `IosBackend::register_navigator` /
    /// `MacosBackend::register_navigator`.
    pub fn register_navigator<P, F>(&mut self, factory: F)
    where
        P: 'static,
        F: Fn() -> Box<dyn runtime_core::NavigatorHandler<WgpuBackend>> + 'static,
    {
        self.navigator_handlers.register::<P, _>(factory);
    }

    /// Look up the raw bytes for a registered image asset. Returns
    /// `None` if no asset with that id was registered (the renderer
    /// then falls back to its filesystem-path resolver). Exposed as
    /// `pub` so the renderer in [`crate::renderer`] — which is in
    /// the same crate but accesses `WgpuBackend` through `Host::
    /// backend()` — can call without going through a private field.
    pub fn image_asset_bytes(
        &self,
        id: runtime_core::AssetId,
    ) -> Option<&[u8]> {
        self.image_asset_bytes.get(&id).map(|v| v.as_slice())
    }

    /// Drain the live-region announcement queue accumulated by
    /// [`Backend::announce_for_accessibility`]. Returns the
    /// announcements in insertion order and clears the internal
    /// buffer.
    ///
    /// The host shell calls this after every layout-commit pass
    /// (same point it calls
    /// [`Backend::dump_accessibility_tree`]) and routes each entry
    /// to the platform announcement API:
    ///
    /// - macOS host: `NSAccessibilityAnnouncementRequestedNotification`.
    /// - iOS host:   `UIAccessibility.post(notification: .announcement, ...)`.
    /// - Linux/AT-SPI host: `AtspiObject.Announcement` signal.
    ///
    /// Separate from
    /// [`Backend::dump_accessibility_tree`] because announcements
    /// are transient one-shots (each fires once and is gone), whereas
    /// the semantics tree is a persistent snapshot the AX walker
    /// can re-query at any time.
    pub fn drain_pending_announcements(&mut self) -> Vec<(String, LiveRegionPriority)> {
        std::mem::take(&mut self.pending_announcements)
    }

    /// Take the served-file URLs of `Bundled`/`Remote` fonts the app
    /// registered without inline bytes (the web path; see
    /// [`WgpuBackend::pending_font_urls`]). The host shell fetches each
    /// and feeds the bytes back via the font system. Empty on native.
    pub fn drain_pending_font_urls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_font_urls)
    }

    /// Snapshot of the active root, or `None` if nothing has been
    /// mounted yet. The renderer reads this on each frame.
    pub fn root(&self) -> Option<WgpuNode> {
        self.roots.last().cloned()
    }

    /// Drop transient per-tree state — presence tweens, animator,
    /// sticky registry — so a subsequent `Host::mount(build_ui)`
    /// doesn't carry stale references into the new tree. Called by
    /// [`Host::unmount`]. GPU pipelines and shared device/queue
    /// resources survive.
    ///
    /// `presence_tweens` is the load-bearing one: it holds strong
    /// `WgpuNode` refs and is iterated by [`tick_presence_tweens`]
    /// every frame. Without this clear, the per-frame tick keeps
    /// advancing tweens on ghosts of the previous tree, and the
    /// renderer's walk over the new root's subtree never visits
    /// them — net effect is the renderer emits no draws for what
    /// the user expects to see.
    pub fn reset_per_tree_state(&mut self) {
        self.presence_tweens.clear();
        self.animator.clear();
        self.sticky_registry = crate::sticky::StickyRegistry::new();
        self.active_spinner_count = 0;
        // `text.buffers` is keyed by Taffy `LayoutNode` IDs. Taffy's
        // SlotMap recycles freed IDs, so without this clear the new
        // tree's text nodes may inherit stale `BufferEntry` values
        // from the old tree (the keys collide). That's the highest-
        // probability cause of "everything rendered fine on initial
        // mount, but a remount renders a blank canvas" — the walk
        // reads `text.buffers.get(&data.layout)` and either skips
        // (if the stale entry is unusable) or stages the old glyph
        // data which doesn't match the new viewport.
        self.text.borrow_mut().buffers.clear();
    }

    /// Install a Taffy measure callback that asks glyphon for the
    /// wrapped extent given the constraint Taffy passes in. Captures
    /// `Weak`s to the shared text + font_system stores so the
    /// closure can outlive the backend without dangling.
    fn install_text_measure(&mut self, id: LayoutNode) {
        let text_weak: Weak<RefCell<TextStore>> = Rc::downgrade(&self.text);
        let fs_weak: Weak<RefCell<FontSystem>> = Rc::downgrade(&self.font_system);
        self.layout.set_measure_fn(
            id,
            Rc::new(move |known, available| {
                let (Some(text), Some(fs)) = (text_weak.upgrade(), fs_weak.upgrade()) else {
                    return TaffySize { width: 0.0, height: 0.0 };
                };
                let mut text = text.borrow_mut();
                let mut fs = fs.borrow_mut();
                let max_w = known.width.or_else(|| match available.width {
                    AvailableSpace::Definite(v) => Some(v),
                    _ => None,
                });
                let (w, h) = text.measure(&mut fs, id, max_w);
                TaffySize { width: w, height: h }
            }),
        );
    }
}

impl Backend for WgpuBackend {
    type Node = WgpuNode;

    fn platform(&self) -> runtime_core::Platform {
        // The wgpu renderer itself is platform-agnostic; the active
        // `Painter` decides what host it's pretending to be. Delegating
        // here means iOS-sim / android-sim skins each self-report
        // (typically `Custom("Sim")`) without the wgpu crate having
        // to enumerate them.
        self.skin.platform()
    }

    fn color_scheme(&self) -> ColorScheme {
        self.color_scheme
    }

    fn create_view(
        &mut self,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        let node = new_node(NodeKind::View, layout);
        init_node_a11y(&node, a11y, PrimitiveKind::View);
        self.roots.push(node.clone());
        node
    }

    fn create_text(
        &mut self,
        content: &str,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.create(&mut fs, layout, content, 14.0);
        }
        self.install_text_measure(layout);
        let node = new_node(
            NodeKind::Text { content: content.to_string() },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::Text);
        self.roots.push(node.clone());
        node
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&runtime_core::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_core::primitives::icon::IconData>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.create(&mut fs, layout, label, 14.0);
        }
        self.install_text_measure(layout);
        let fire = on_click.fire.clone();
        let cb: Rc<dyn Fn()> = Rc::new(move || {
            fire();
            request_redraw();
        });
        let node = new_node(
            NodeKind::Button { label: label.to_string(), on_click: cb },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::Button);

        // Stamp the skin's button defaults *at create time* so an
        // unstyled `button(...)` looks platform-native without
        // depending on the framework to call `apply_style` at all
        // (it doesn't, for primitives the author leaves unstyled).
        // If the author *does* attach a stylesheet, `apply_style`
        // fires later and re-stamps via the same merge path —
        // defaults under, author on top — so this isn't redundant
        // with that flow, it's the unstyled-path entry point.
        let defaults = self.skin.button_defaults();
        if !defaults_are_empty(&defaults) {
            // Re-use `apply_style`'s machinery so font-size,
            // background color tween anchoring, text font-size
            // sync, etc. all run through one code path. The
            // merge in `apply_style` handles `defaults <- defaults`
            // as a no-op when the author hasn't supplied any.
            let rules = Rc::new(defaults);
            self.apply_style(&node, &rules);
        }

        self.roots.push(node.clone());
        node
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        // Wrap to request a redraw — the user's closure mutates
        // app state, but the framework doesn't drive a redraw on
        // its own; we ping winit so the next frame paints the
        // updated tree.
        let cb: Rc<dyn Fn()> = Rc::new(move || {
            on_click();
            request_redraw();
        });
        let node = new_node(NodeKind::Pressable { on_click: cb }, layout);
        init_node_a11y(&node, a11y, PrimitiveKind::Pressable);
        self.roots.push(node.clone());
        node
    }

    fn install_touch_handler(
        &mut self,
        node: &Self::Node,
        handler: runtime_core::TouchHandler,
    ) {
        node.borrow_mut().touch_handler = Some(handler);
    }

    // `claim_touch` keeps the default no-op. The wgpu Host owns the
    // touch dispatcher end-to-end, so claim bookkeeping lives there
    // — there is no external native subsystem to inform. iOS and
    // Android backends will override this to call into UIKit /
    // Android-View claim mechanisms.

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        // The visible glyph buffer holds whichever of value /
        // placeholder is currently being shown. Empty value =>
        // placeholder; otherwise => value. The widget renderer
        // reads `value.is_empty()` from the node to know which
        // color to use.
        let visible = if initial_value.is_empty() {
            placeholder.unwrap_or("")
        } else {
            initial_value
        };
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.create(&mut fs, layout, visible, 17.0);
        }
        // Pin the field's height; width flexes by default so an
        // input in a column stretches across the parent.
        self.layout
            .set_intrinsic_size(layout, -1.0, TEXT_INPUT_DEFAULT_HEIGHT);
        let node = new_node(
            NodeKind::TextInput {
                value: initial_value.to_string(),
                placeholder: placeholder.map(|s| s.to_string()),
                on_change,
            },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::TextInput);
        self.roots.push(node.clone());
        node
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        let layout = node.borrow().layout;
        let visible = {
            let mut data = node.borrow_mut();
            if let NodeKind::TextInput { value: stored, placeholder, .. } = &mut data.kind {
                *stored = value.to_string();
                if value.is_empty() {
                    placeholder.clone().unwrap_or_default()
                } else {
                    value.to_string()
                }
            } else {
                return;
            }
        };
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.set_text(&mut fs, layout, &visible);
        }
        self.layout.mark_dirty(layout);
        request_redraw();
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        // No-wrap (code) vs. soft-wrap. The wgpu text path is still a
        // single-line MVP (multi-line wrap + caret are pending the
        // text-shaping work noted below), so `wrap` is accepted and
        // honored as a follow-up. No `auto_grow`: content-height growth
        // is intrinsic sizing, which lands with the same shaping work.
        _wrap: bool,
        on_change: Rc<dyn Fn(String)>,
        _on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // MVP: shape identical to TextInput, larger default height.
        // The renderer + host treat NodeKind::TextArea the same as
        // NodeKind::TextInput for now; multi-line wrap + caret are
        // pending separate text-shaping work.
        let layout = self.layout.new_node();
        let visible = if initial_value.is_empty() {
            placeholder.unwrap_or("")
        } else {
            initial_value
        };
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.create(&mut fs, layout, visible, 17.0);
        }
        self.layout
            .set_intrinsic_size(layout, -1.0, TEXT_AREA_DEFAULT_HEIGHT);
        let node = new_node(
            NodeKind::TextArea {
                value: initial_value.to_string(),
                placeholder: placeholder.map(|s| s.to_string()),
                on_change,
            },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::TextArea);
        self.roots.push(node.clone());
        node
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        let layout = node.borrow().layout;
        let visible = {
            let mut data = node.borrow_mut();
            if let NodeKind::TextArea { value: stored, placeholder, .. } = &mut data.kind {
                *stored = value.to_string();
                if value.is_empty() {
                    placeholder.clone().unwrap_or_default()
                } else {
                    value.to_string()
                }
            } else {
                return;
            }
        };
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.set_text(&mut fs, layout, &visible);
        }
        self.layout.mark_dirty(layout);
        request_redraw();
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        self.layout
            .set_intrinsic_size(layout, TOGGLE_WIDTH, TOGGLE_HEIGHT);
        let node = new_node(
            NodeKind::Toggle { value: initial_value, on_change },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::Toggle);
        self.roots.push(node.clone());
        node
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        let layout = node.borrow().layout;
        // Only animate on an actual value change. The framework
        // re-fires the controlled-value Effect even when the new
        // value matches the old, and we don't want a wasted tween.
        let old_value = match &node.borrow().kind {
            NodeKind::Toggle { value: stored, .. } => *stored,
            _ => return,
        };
        if let NodeKind::Toggle { value: stored, .. } = &mut node.borrow_mut().kind {
            *stored = value;
        }
        if old_value != value {
            let target = if value { 1.0 } else { 0.0 };
            // The rest position *before* this flip is where the
            // thumb visually sits when there's no in-flight tween.
            // Pass it so the very first animation (no existing
            // tween to sample) starts from the right place.
            let rest_before = if old_value { 1.0 } else { 0.0 };
            self.animator.animate(
                TweenKey::new(layout, AnimProperty::ToggleThumb),
                target,
                rest_before,
                TOGGLE_ANIM_MS,
                Easing::EaseOut,
                Instant::now(),
            );
        }
        request_redraw();
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        self.layout
            .set_intrinsic_size(layout, SLIDER_DEFAULT_WIDTH, SLIDER_HEIGHT);
        let node = new_node(
            NodeKind::Slider {
                value: initial_value,
                min,
                max,
                step,
                on_change,
            },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::Slider);
        self.roots.push(node.clone());
        node
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        if let NodeKind::Slider { value: stored, .. } = &mut node.borrow_mut().kind {
            *stored = value;
        }
        request_redraw();
    }

    fn create_activity_indicator(
        &mut self,
        size: ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        let diameter = match size {
            ActivityIndicatorSize::Small => ACTIVITY_INDICATOR_SMALL_SIZE,
            ActivityIndicatorSize::Large => ACTIVITY_INDICATOR_LARGE_SIZE,
        };
        // Intrinsic square — author can still override via `width`
        // / `height` in styles, same convention as the other native
        // widgets.
        self.layout.set_intrinsic_size(layout, diameter, diameter);
        let node = new_node(
            NodeKind::ActivityIndicator {
                size,
                color: color.map(parse_color),
            },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::ActivityIndicator);
        self.active_spinner_count = self.active_spinner_count.saturating_add(1);
        self.roots.push(node.clone());
        request_redraw();
        node
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        // Pin the scrollview's main-axis `min-size` to 0 so the
        // parent's flex layout can shrink it below its children's
        // content height (CSS flex-item-`min: auto` gotcha — the
        // Taffy default would otherwise lock the scrollview to
        // its content's intrinsic size, defeating overflow). The
        // scrollview's children get `flex_shrink: 0` in `insert`
        // so they stay at natural sizes and overflow the now
        // smaller scrollview frame. `-1.0` on the other axis
        // means "leave that axis untouched".
        if horizontal {
            self.layout.set_intrinsic_size(layout, 0.0, -1.0);
        } else {
            self.layout.set_intrinsic_size(layout, -1.0, 0.0);
        }
        let node = new_node(
            NodeKind::ScrollView {
                horizontal,
                offset_x: 0.0,
                offset_y: 0.0,
                on_scroll,
            },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::ScrollView);
        self.roots.push(node.clone());
        node
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        let layout = self.layout.new_node();
        let node = new_node(NodeKind::ReactiveAnchor, layout);
        // ReactiveAnchor is a transparent control-flow container —
        // it never carries author-supplied a11y props (the walker
        // doesn't pass a primitive kind for it). Defaults on
        // `NodeData` are correct (empty props, no inferred role).
        self.roots.push(node.clone());
        node
    }

    // -----------------------------------------------------------
    // Link — text + on-activate. Same interaction shape as
    // Pressable; the framework hands us the on_activate closure
    // pre-baked with its push/replace/reset dispatch logic.
    // -----------------------------------------------------------

    fn create_link(
        &mut self,
        config: runtime_core::primitives::link::LinkConfig,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        // Wrap the activate closure to also request a redraw so
        // a click that mutates app state repaints immediately.
        let activate = config.on_activate.clone();
        let cb: Rc<dyn Fn()> = Rc::new(move || {
            activate();
            request_redraw();
        });
        let node = new_node(NodeKind::Link { on_activate: cb }, layout);
        init_node_a11y(&node, a11y, PrimitiveKind::Link);
        self.roots.push(node.clone());
        node
    }

    // -----------------------------------------------------------
    // Image — placeholder for now. Stores src + alt so the
    // renderer can paint a labeled placeholder rect. A real
    // textured-quad pipeline is future work.
    // -----------------------------------------------------------

    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        self.layout
            .set_intrinsic_size(layout, IMAGE_DEFAULT_SIZE, IMAGE_DEFAULT_SIZE);
        let node = new_node(
            NodeKind::Image {
                src: src.to_string(),
                alt: alt.map(|s| s.to_string()),
            },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::Image);
        self.roots.push(node.clone());
        node
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        if let NodeKind::Image { src: stored, .. } = &mut node.borrow_mut().kind {
            *stored = src.to_string();
        }
        request_redraw();
    }

    // -----------------------------------------------------------
    // Icon — placeholder square. Path/SDF rendering pending; the
    // icon's tint flows through `update_icon_color`.
    // -----------------------------------------------------------

    fn create_icon(
        &mut self,
        data: &runtime_core::primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        self.layout
            .set_intrinsic_size(layout, ICON_DEFAULT_SIZE, ICON_DEFAULT_SIZE);
        // `IconData.paths` is `&'static [&'static str]` and
        // `view_box` is plain `(u16, u16)` — both Copy and
        // safe to stash on the node without lifetime tricks.
        let node = new_node(
            NodeKind::Icon {
                paths: data.paths,
                view_box: data.view_box,
                color: color.map(parse_color),
                stroke_progress: std::cell::Cell::new(1.0),
                filled: data.filled,
            },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::Icon);
        self.roots.push(node.clone());
        node
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        if let NodeKind::Icon { color: stored, .. } = &mut node.borrow_mut().kind {
            *stored = Some(parse_color(color));
        }
        request_redraw();
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        if let NodeKind::Icon { stroke_progress, .. } = &node.borrow().kind {
            stroke_progress.set(progress.clamp(0.0, 1.0));
        }
        request_redraw();
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: Easing,
        _infinite: bool,
        _autoreverses: bool,
    ) {
        // Looping / autoreverse aren't wired yet — the animator
        // only tracks one-shot tweens. For V1 we run the one-shot
        // from→to; infinite + autoreverse fall back to "play
        // once and hold at `to`", which is the most useful
        // degenerate behavior for static screenshots.
        let layout = node.borrow().layout;
        if let NodeKind::Icon { stroke_progress, .. } = &node.borrow().kind {
            stroke_progress.set(from.clamp(0.0, 1.0));
        }
        self.animator.animate(
            TweenKey::new(layout, AnimProperty::IconStroke),
            to,
            from,
            duration_ms,
            easing,
            Instant::now(),
        );
        request_redraw();
    }

    // -----------------------------------------------------------
    // Portals — painted in a top-z pass after the main walk.
    //
    // We own the entire frame, so a portal is just a scene-graph
    // entry hoisted to a viewport-rooted top-z layer. The
    // renderer's existing `walk_overlay` pass handles both viewport
    // placement and anchor tracking — anchor positions are
    // re-queried each frame, which is cheap because we re-render
    // every frame anyway. `Named` slots aren't wired up.
    // -----------------------------------------------------------

    fn create_portal(
        &mut self,
        target: runtime_core::primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        if matches!(target, runtime_core::primitives::portal::PortalTarget::Named(_)) {
            unimplemented!(
                "PortalTarget::Named is not supported by the wgpu backend"
            );
        }
        let layout = self.layout.new_node();
        let node = new_node(NodeKind::Portal { target, on_dismiss }, layout);
        init_node_a11y(&node, a11y, PrimitiveKind::Portal);
        self.roots.push(node.clone());
        node
    }

    // -----------------------------------------------------------
    // Virtualizer — for the simulator, mount every item up front
    // (no actual windowing). The framework's eager-mount path
    // calls `mount_item` for each index; we insert the result.
    // -----------------------------------------------------------

    fn create_virtualizer(
        &mut self,
        callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        horizontal: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        // Stash the callbacks on the node so
        // `virtualizer_data_changed` can re-mount items when
        // the data signal fires — without these, the only
        // mount path was create time and any later insert /
        // remove would silently drop on the floor.
        let mount = callbacks.mount_item.clone();
        let release = callbacks.release_item.clone();
        let count_fn = callbacks.item_count.clone();
        let node = new_node(
            NodeKind::Virtualizer {
                horizontal,
                mount_item: mount.clone(),
                release_item: release,
                item_count: count_fn,
                scope_ids: std::cell::RefCell::new(Vec::new()),
            },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::Virtualizer);
        // Eagerly mount every item — no windowing yet. A real
        // windowed implementation would mount on demand based
        // on viewport intersection.
        let count = (callbacks.item_count)();
        for i in 0..count {
            let (child, scope_id) = mount(i);
            let child_layout = child.borrow().layout;
            self.layout.add_child(layout, child_layout);
            node.borrow_mut().children.push(child.clone());
            self.roots.retain(|n| !Rc::ptr_eq(n, &child));
            if let NodeKind::Virtualizer { scope_ids, .. } = &node.borrow().kind {
                scope_ids.borrow_mut().push(scope_id);
            }
        }
        self.roots.push(node.clone());
        node
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        // Snapshot the callbacks + current scope ids before
        // we start mutating. Cloning the `Rc`s is cheap and
        // avoids re-borrowing the node mid-mutation.
        let (mount, release, count_fn, prev_ids) = {
            let data = node.borrow();
            if let NodeKind::Virtualizer {
                mount_item,
                release_item,
                item_count,
                scope_ids,
                ..
            } = &data.kind
            {
                (
                    mount_item.clone(),
                    release_item.clone(),
                    item_count.clone(),
                    scope_ids.borrow().clone(),
                )
            } else {
                return;
            }
        };
        // Drop the existing children — release each scope via
        // the framework callback and clean Taffy state via
        // `drop_subtree`. Equivalent to `clear_children` but
        // also calls `release_item` so the framework reclaims
        // the per-item Scope arenas.
        //
        // Order matters: detach from Taffy, then `release` (frees
        // the framework `Scope`, which unregisters this node's
        // theme-cohort entries), then `drop_subtree` (removes the
        // Taffy slot). Doing `drop_subtree` before `release` would
        // leave cohort entries pointing at freed slots — the next
        // `set_theme` would panic with "invalid SlotMap key used".
        let parent_layout = node.borrow().layout;
        let children: Vec<WgpuNode> = node.borrow_mut().children.drain(..).collect();
        for (child, scope_id) in children.iter().zip(prev_ids.iter()) {
            let child_layout = child.borrow().layout;
            self.layout.remove_child(parent_layout, child_layout);
            release(*scope_id);
            drop_subtree(
                &mut self.layout,
                &self.text,
                &mut self.animator,
                &mut self.active_spinner_count,
                &mut self.sticky_registry,
                child,
            );
        }
        // Re-mount based on the new count.
        let new_count = count_fn();
        let mut new_ids = Vec::with_capacity(new_count);
        for i in 0..new_count {
            let (child, scope_id) = mount(i);
            let child_layout = child.borrow().layout;
            self.layout.add_child(parent_layout, child_layout);
            node.borrow_mut().children.push(child.clone());
            self.roots.retain(|n| !Rc::ptr_eq(n, &child));
            new_ids.push(scope_id);
        }
        if let NodeKind::Virtualizer { scope_ids, .. } = &node.borrow().kind {
            *scope_ids.borrow_mut() = new_ids;
        }
        request_redraw();
    }

    // -----------------------------------------------------------
    // Navigators — each is a container node that the framework's
    // dispatcher pushes/pops/replaces screens into via the
    // backend's normal `insert` / `clear_children` paths.
    //
    // Our dispatcher implementation is the simplest possible:
    // commands re-insert the mounted screen, framework releases
    // popped scopes via `release_screen`. For chrome (header
    // bar, tab bar, drawer sidebar) we paint a platform-skinned
    // strip around the active screen's rect.
    // -----------------------------------------------------------


    fn create_graphics(
        &mut self,
        _on_ready: runtime_core::primitives::graphics::OnReady,
        _on_resize: runtime_core::primitives::graphics::OnResize,
        _on_lost: runtime_core::primitives::graphics::OnLost,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // We can't satisfy the framework's `OnReady(GraphicsSurface)`
        // contract — `GraphicsSurface` is a real-window handle, and
        // we're rendering into a sub-region of our own surface, not
        // into a child window. Authors register a draw closure via
        // [`crate::register_graphics_drawer`] instead; the
        // framework callbacks are dropped on the floor here.
        //
        // Layout: a leaf node with a non-zero intrinsic size so an
        // unsized Graphics still occupies space until the author
        // gives it a width/height via stylesheet.
        let layout = self.layout.new_node();
        self.layout.set_intrinsic_size(layout, 200.0, 200.0);
        let node = new_node(
            NodeKind::Graphics {
                drawer: std::cell::RefCell::new(None),
                created_at: web_time::Instant::now(),
            },
            layout,
        );
        init_node_a11y(&node, a11y, PrimitiveKind::Graphics);
        self.roots.push(node.clone());
        node
    }

    fn make_graphics_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::graphics::GraphicsHandle {
        // Wrap the `WgpuNode` itself as the handle's userdata so
        // `register_graphics_drawer` can downcast back to recover
        // it. `WgpuNode = Rc<RefCell<NodeData>>`; the `Rc<dyn Any>`
        // GraphicsHandle holds therefore points at a fresh Rc whose
        // inner concrete type is `WgpuNode` (i.e.
        // `Rc<RefCell<NodeData>>`). Downcast target on retrieval
        // is the same `WgpuNode` type alias.
        runtime_core::primitives::graphics::GraphicsHandle::new(
            Rc::new(node.clone()) as Rc<dyn std::any::Any>,
            &WgpuGraphicsOps,
        )
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let parent_layout = parent.borrow().layout;
        let child_layout = child.borrow().layout;
        let parent_is_scroll =
            matches!(parent.borrow().kind, NodeKind::ScrollView { .. });
        // Portal nodes are taken out of normal flow at the
        // Taffy level so the parent's flex layout doesn't
        // reserve inline space for them. The actual screen
        // position is computed in the renderer's top-z pass.
        let child_is_portal =
            matches!(child.borrow().kind, NodeKind::Portal { .. });
        self.layout.add_child(parent_layout, child_layout);
        parent.borrow_mut().children.push(child.clone());
        // The child is no longer orphaned — drop it from `roots`.
        self.roots.retain(|n| !Rc::ptr_eq(n, &child));

        if child_is_portal {
            // `position: absolute` removes the node from flex
            // flow; the renderer's portal pass places it
            // against the viewport directly, so we don't need
            // explicit insets here. Taffy still lays the
            // portal's *children* out within whatever size we
            // compute for the portal node itself.
            let floating = StyleRules {
                position: Some(runtime_core::Position::Absolute),
                ..Default::default()
            };
            self.layout.set_style(child_layout, &floating);
        }

        // ScrollView children must not shrink. Taffy defaults
        // `flex_shrink: 1.0`; when the scrollview's frame is
        // constrained by its parent (typically `flex_grow: 1` to
        // fill remaining space), Taffy compresses children to fit
        // — so the content never overflows the viewport and
        // there's nothing to scroll. Pinning shrink to 0 keeps
        // children at their natural sizes; the overflow is what
        // makes the scroll machinery actually do something.
        //
        // The author can still override via an explicit
        // `flex_shrink` in their stylesheet — this only sets the
        // Taffy default for children of scrollviews.
        if parent_is_scroll {
            let no_shrink = StyleRules {
                flex_shrink: Some(Tokenized::Literal(0.0)),
                ..Default::default()
            };
            self.layout.set_style(child_layout, &no_shrink);
        }

        request_redraw();
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        let layout = node.borrow().layout;
        {
            let mut data = node.borrow_mut();
            match &mut data.kind {
                NodeKind::Text { content: existing } => *existing = content.to_string(),
                NodeKind::Button { label, .. } => *label = content.to_string(),
                _ => {}
            }
        }
        {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.set_text(&mut fs, layout, content);
        }
        self.layout.mark_dirty(layout);
        request_redraw();
    }

    fn clear_children(&mut self, node: &Self::Node) {
        let parent_layout = node.borrow().layout;
        let children: Vec<WgpuNode> = node.borrow_mut().children.drain(..).collect();
        for child in &children {
            let child_layout = child.borrow().layout;
            self.layout.remove_child(parent_layout, child_layout);
            // Recursively drop layout entries — the framework drops
            // the WgpuNode Rcs, but Taffy doesn't reference-count
            // its slots; we clean them up here so the tree doesn't
            // leak after a `when` flip.
            drop_subtree(
                &mut self.layout,
                &self.text,
                &mut self.animator,
                &mut self.active_spinner_count,
                &mut self.sticky_registry,
                child,
            );
        }
        request_redraw();
    }

    fn finish(&mut self, root: Self::Node) {
        // The framework hands us the root once the build walker has
        // emitted every create/insert/apply_style for this tree.
        // Make sure it's the only entry in `roots` (every other
        // node has either been inserted as a child or is stale).
        self.roots.retain(|n| Rc::ptr_eq(n, &root));
        if self.roots.is_empty() {
            self.roots.push(root);
        }
        request_redraw();
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        node.borrow_mut().state_setter = Some(setter);
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        runtime_core::ViewHandle::new(Rc::new(node.clone()), &crate::handles::WGPU_VIEW_OPS)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        runtime_core::TextHandle::new(Rc::new(node.clone()), &crate::handles::WGPU_TEXT_OPS)
    }

    fn set_animated_f32(
        &mut self,
        node: &Self::Node,
        prop: runtime_core::animation::AnimProp,
        value: f32,
    ) {
        use runtime_core::animation::AnimProp;
        {
            let mut data = node.borrow_mut();
            let ov = data
                .animated
                .get_or_insert_with(|| Box::new(crate::node::AnimatedOverrides::default()));
            match prop {
                AnimProp::Opacity => ov.opacity = Some(value),
                AnimProp::TranslateX => ov.translate_x = Some(value),
                AnimProp::TranslateY => ov.translate_y = Some(value),
                AnimProp::Scale => {
                    ov.scale_x = Some(value);
                    ov.scale_y = Some(value);
                }
                AnimProp::ScaleX => ov.scale_x = Some(value),
                AnimProp::ScaleY => ov.scale_y = Some(value),
                AnimProp::RotateZ => ov.rotate_z = Some(value),
                AnimProp::ZIndex => ov.z_index = Some(value),
                // No layout-affecting animation on gpu-backend yet —
                // snap-only (the property value lands on the next
                // layout pass via normal style application).
                AnimProp::MaxHeight => {}
                // Wrong family. Same posture as the iOS / web f32
                // path — silently ignored; misrouting is a
                // diagnostic concern, not a runtime crash.
                AnimProp::BackgroundColor
                | AnimProp::ForegroundColor
                | AnimProp::GradientStopColor(_) => {}
            }
        }
        request_redraw();
    }

    fn register_asset(
        &mut self,
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
        source: &runtime_core::AssetSource,
    ) {
        // Two paths: Font assets go into cosmic-text's font db so
        // the text shaper can resolve them. Image assets go into
        // our local byte cache so the renderer's `decode_and_upload`
        // can resolve `asset://{id}` srcs against the same `image`
        // crate decode path it uses for filesystem srcs. Other asset
        // kinds (Audio, Blob) flow through their own pipelines or
        // aren't supported yet — silently ignored.
        match kind {
            runtime_core::AssetTag::Font => {
                match source {
                    // Bytes-in-binary path (native, `embed-font-bytes`
                    // on): `face!` emits `BundledEmbedded` (path +
                    // bytes); `Embedded` covers a hand-rolled
                    // `embed_asset!` font. Load synchronously — the
                    // bytes are `'static`, so `to_vec()` is a cheap
                    // one-time-per-font clone.
                    runtime_core::AssetSource::Embedded { bytes, .. }
                    | runtime_core::AssetSource::BundledEmbedded { bytes, .. } => {
                        self.font_system
                            .borrow_mut()
                            .db_mut()
                            .load_font_data(bytes.to_vec());
                    }
                    // Bytes-free path. Two consumers:
                    //
                    // - **web** (`embed-font-bytes` off): the font lives
                    //   as a served file at root-absolute `/{path}` the
                    //   DOM backend links via `@font-face`. The renderer
                    //   can't issue an async fetch, so queue the URL for
                    //   the host shell (`host-web`) to fetch +
                    //   `load_font_data` after mount, before the first
                    //   frame. See `drain_pending_font_urls`.
                    //
                    // - **native, notably the headless screenshot
                    //   backend**: in runtime-server mode every font
                    //   arrives over the wire as `Bundled { path }`
                    //   (the recorder strips bytes for transport), and
                    //   there is no async fetch hook. Load the file
                    //   straight from disk so registered weights (e.g.
                    //   `Inter-Bold`) actually shape — without this the
                    //   shaper only has the bundled default
                    //   (`Inter-Regular`) and bold/italic/etc. silently
                    //   fall back to a wrong font, so a screenshot
                    //   diverges from the real iOS/Android render.
                    //   `path` is the app-relative path the recorder
                    //   captured; it resolves against the process CWD
                    //   (the project dir for a dev-server sidecar).
                    runtime_core::AssetSource::Bundled { path } => {
                        #[cfg(target_arch = "wasm32")]
                        {
                            self.pending_font_urls.push(format!("/{path}"));
                        }
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            match std::fs::read(path) {
                                Ok(bytes) => {
                                    self.font_system.borrow_mut().db_mut().load_font_data(bytes);
                                }
                                Err(e) => {
                                    // Keep the served-URL hook populated
                                    // too — harmless on native, and lets
                                    // any future fetch-capable native
                                    // host still resolve it.
                                    self.pending_font_urls.push(format!("/{path}"));
                                    eprintln!(
                                        "[render-wgpu] bundled font {path:?} not loadable from \
                                         disk ({e}); text in this family/weight will fall back \
                                         to the default font"
                                    );
                                }
                            }
                        }
                    }
                    // Arbitrary remote URL — same deferred-fetch hook.
                    runtime_core::AssetSource::Remote { url } => {
                        self.pending_font_urls.push((*url).to_string());
                    }
                }
            }
            runtime_core::AssetTag::Image => {
                if let runtime_core::AssetSource::Embedded { bytes, .. }
                | runtime_core::AssetSource::BundledEmbedded { bytes, .. } = source
                {
                    // Store the raw bytes; the renderer decodes
                    // lazily on first `asset://{id}` reference. The
                    // image crate auto-detects PNG/JPEG/WebP/etc
                    // from the bytes' magic, so no per-format
                    // dispatch is needed here.
                    self.image_asset_bytes.insert(id, bytes.to_vec());
                }
            }
            _ => {}
        }
    }

    fn unregister_asset(
        &mut self,
        id: runtime_core::AssetId,
        kind: runtime_core::AssetTag,
    ) {
        // Remove the cached bytes when the framework hot-reloads or
        // explicitly retires an asset. Font removal can't be undone
        // through cosmic-text's `load_font_data` API (the database
        // is append-only), so we just drop our image-side entry.
        if matches!(kind, runtime_core::AssetTag::Image) {
            self.image_asset_bytes.remove(&id);
        }
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: runtime_core::primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        let node_key = Rc::as_ptr(node) as usize;

        // The framework calls apply_presence with the new target on
        // top of a possibly-in-flight previous animation. Capture the
        // node's CURRENT rendered values (which the renderer reads
        // from AnimatedOverrides) so a mid-animation reversal lerps
        // from where we visually are, not from the previous start.
        let current = read_presence_state(node);
        let target = resolve_presence_target(state, current);

        // Cancel any prior tween on this node — apply_presence is
        // authoritative: the framework just decided where this node
        // is going, and any prior interpolation toward a different
        // target would visibly stutter.
        self.presence_tweens.remove(&node_key);

        match transition {
            None => {
                // Snap. Write the target values directly to the
                // node's AnimatedOverrides; the renderer composites
                // them with the stylesheet on the next frame.
                write_presence_overrides(node, &state, target);
                request_redraw();
            }
            Some((duration_ms, _easing)) if duration_ms == 0 => {
                // Zero-duration transition is just a snap with
                // overrides cleared/written. Don't insert a tween.
                write_presence_overrides(node, &state, target);
                request_redraw();
            }
            Some((duration_ms, easing)) => {
                // Start a tween. The interpolator (driven by
                // `tick_presence_tweens` via the host's per-frame
                // tick) writes intermediate values until elapsed
                // ≥ duration, then `tick_presence_tweens` removes
                // the entry.
                //
                // Important: write the FROM state immediately so
                // the next frame paints the captured starting
                // values rather than whatever stale ones might
                // have leaked through. The framework's enter
                // sequence relies on this — it calls
                // `apply_presence(state=enter, None)` to snap, then
                // `apply_presence(state=rest, Some((ms, ease)))` one
                // frame later to animate toward rest. Our snap
                // already wrote the enter values; here we just
                // queue the tween + redraw.
                self.presence_tweens.insert(
                    node_key,
                    PresenceTween {
                        node: node.clone(),
                        from: current,
                        to: target,
                        started: Instant::now(),
                        duration: std::time::Duration::from_millis(
                            duration_ms as u64,
                        ),
                        easing,
                    },
                );
                request_redraw();
            }
        }
    }

    fn set_animated_color(
        &mut self,
        node: &Self::Node,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        use runtime_core::animation::AnimProp;
        {
            let mut data = node.borrow_mut();
            let ov = data
                .animated
                .get_or_insert_with(|| Box::new(crate::node::AnimatedOverrides::default()));
            match prop {
                AnimProp::BackgroundColor => ov.background_color = Some(value),
                AnimProp::ForegroundColor => ov.foreground_color = Some(value),
                AnimProp::GradientStopColor(idx) => {
                    // Per-stop override: replace if the stop is already
                    // tracked, else append. Linear scan is fine — a
                    // gradient typically has < 8 stops.
                    if let Some(slot) =
                        ov.gradient_stops.iter_mut().find(|(i, _)| *i == idx)
                    {
                        slot.1 = value;
                    } else {
                        ov.gradient_stops.push((idx, value));
                    }
                }
                // Wrong family — silently ignored.
                AnimProp::Opacity
                | AnimProp::TranslateX
                | AnimProp::TranslateY
                | AnimProp::Scale
                | AnimProp::ScaleX
                | AnimProp::ScaleY
                | AnimProp::RotateZ
                | AnimProp::ZIndex
                | AnimProp::MaxHeight => {}
            }
        }
        request_redraw();
    }


    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        // The framework's state-overlay system handles the
        // visual side — any stylesheet with a
        // `state { disabled, … }` overlay re-resolves and
        // pushes a fresh style through `apply_style` once the
        // state bit flips. Our job is just to flip the bit via
        // the setter that `attach_states` cached on the node.
        let setter = node.borrow().state_setter.clone();
        if let Some(setter) = setter {
            setter(StateBits::DISABLED, disabled);
            request_redraw();
        }
    }

    fn frame(
        &self,
        node: &Self::Node,
    ) -> Option<runtime_core::primitives::portal::ViewportRect> {
        // Local frame (relative to the parent's content box) —
        // straight out of Taffy's computed layout.
        let frame = self.layout.frame_of(node.borrow().layout);
        Some(runtime_core::primitives::portal::ViewportRect {
            x: frame.x,
            y: frame.y,
            width: frame.width,
            height: frame.height,
        })
    }

    fn absolute_frame(
        &self,
        node: &Self::Node,
    ) -> Option<runtime_core::primitives::portal::ViewportRect> {
        // Walk down from each root accumulating origins until we
        // hit `node`. `absolute_origin` already does this for the
        // host's pointer dispatch; the rect's size is just the
        // Taffy frame at the node.
        let origin = crate::host::absolute_origin(self, node);
        let size = self.layout.frame_of(node.borrow().layout);
        Some(runtime_core::primitives::portal::ViewportRect {
            x: origin.0,
            y: origin.1,
            width: size.width,
            height: size.height,
        })
    }

    // -----------------------------------------------------------------
    // Accessibility — wgpu is a GPU/canvas backend so there is no
    // platform widget for the AX walker to find. The strategy is the
    // **parallel semantics tree** described in
    // `docs/accessibility-design.md` §5: every `create_*` stashes the
    // author's `AccessibilityProps` onto the node, the layout pass
    // updates each node's bounds, and the host shell pulls a snapshot
    // via `dump_accessibility_tree` once per layout commit and
    // projects it into the platform AX layer (NSAccessibility on
    // macOS, UIAccessibilityElement[] on iOS, AT-SPI on Linux).
    //
    // No host shell consumer exists yet — this is GPU-backend prep
    // work. The future winit / AppKit / iOS-shell wgpu hosts are the
    // intended consumers; see `docs/accessibility-design.md` §5 for
    // the projection contract those hosts must follow.
    // -----------------------------------------------------------------

    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &AccessibilityProps,
        inferred_role: Option<Role>,
    ) {
        // Replace the prop bag wholesale — the framework's reactive
        // a11y Effect re-fires this on every change to any field. No
        // caching: `dump_accessibility_tree` rebuilds from scratch
        // each call (wgpu re-renders every frame anyway), so the new
        // props are visible to the host on its next AX pull. We
        // refresh `inferred_role` too in case the primitive's kind
        // changed via a When/Switch swap — the walker passes the
        // currently-mounted primitive's kind regardless of whether
        // it differs from the original `create_*` call.
        let mut data = node.borrow_mut();
        data.accessibility = a11y.clone();
        data.inferred_role = inferred_role;
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        // Append to the one-shot queue; the host shell drains via
        // `drain_pending_announcements()` and posts each entry to
        // the platform announcement API. We don't dedupe — two
        // identical announcements queued in the same frame really
        // are two announcements (matching the contract on
        // UIAccessibility / NSAccessibilityAnnouncement requests).
        self.pending_announcements.push((msg.to_string(), priority));
    }

    fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
        // Build the parallel semantics tree from the active root.
        // Returns `None` if nothing has been mounted yet — matches
        // the `roots` invariant ("`root()` is the last entry, or
        // `None` if empty").
        let root = self.roots.last()?;
        Some(AccessibilityTree {
            root: build_a11y_node(&self.layout, root),
        })
    }

    fn apply_safe_area_padding(
        &mut self,
        node: &Self::Node,
        sides: runtime_core::SafeAreaSides,
    ) {
        // Read the current insets and stamp them onto the
        // node's Taffy style as padding. The framework's walker
        // calls this from a reactive Effect that re-fires on
        // every change to the insets signal, so we just need to
        // produce the new padding rules from the current value.
        apply_safe_area_to_node(&mut self.layout, node, sides, /*as_padding*/ true);
        request_redraw();
    }

    fn apply_scroll_view_safe_area_inset(
        &mut self,
        node: &Self::Node,
        sides: runtime_core::SafeAreaSides,
    ) {
        // For the wgpu sim the two paths produce the same
        // visual: padding on the scrollview node. Real native
        // backends distinguish so the scroll surface (track,
        // scrollbar) bleeds edge-to-edge while only the content
        // origin insets. Our renderer paints the scrollbar
        // against the scrollview's frame regardless of padding,
        // so a plain padding push gives the right look here.
        apply_safe_area_to_node(&mut self.layout, node, sides, /*as_padding*/ true);
        request_redraw();
    }


    fn on_node_unstyled(&mut self, node: &Self::Node) {
        // Clear the setter so a stale closure can't fire on a
        // node whose style scope has dropped.
        node.borrow_mut().state_setter = None;
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let layout = node.borrow().layout;

        // Merge skin-supplied platform defaults *under* the author
        // style for primitives that ship with a native look (Button
        // today; more primitives can opt in by extending the
        // `Painter` trait). Author rules win on any field they set;
        // the default fills in the rest so an unstyled `button(...)`
        // renders as iOS-tinted text or an M3 filled-pill without
        // the author writing a stylesheet at all.
        let is_button = matches!(node.borrow().kind, NodeKind::Button { .. });
        let effective_style: Rc<StyleRules> = if is_button {
            let defaults = self.skin.button_defaults();
            // `merge` semantics: any field set in `style` overrides
            // the corresponding field in `defaults`. Allocate a new
            // Rc only when defaults actually contribute something —
            // skins that don't opinion buttons (default impl returns
            // empty rules) pay zero cost.
            if defaults_are_empty(&defaults) {
                style.clone()
            } else {
                Rc::new(defaults.merge(style.as_ref()))
            }
        } else {
            style.clone()
        };
        let style: &Rc<StyleRules> = &effective_style;

        self.layout.set_style(layout, style);

        // Snapshot the colors that have transitions declared
        // *before* applying the new style. If the property's old
        // value differs from the new one, start a color tween via
        // the animator. Otherwise the value snaps as it did before.
        //
        // `had_prior_style` distinguishes the very first apply on
        // a node (no "old" value to lerp from — snap to initial)
        // from later re-applies driven by theme swap or state
        // overlay flip.
        let had_prior_style = node.borrow().style.is_some();
        let old_render = node.borrow().render.clone();

        let (is_text, font_size, new_render) = {
            let mut data = node.borrow_mut();
            data.render.apply(style);
            data.style = Some(style.clone());
            (
                matches!(data.kind, NodeKind::Text { .. } | NodeKind::Button { .. }),
                data.render.font_size,
                data.render.clone(),
            )
        };

        if had_prior_style {
            let now = Instant::now();
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::BackgroundColor,
                old_render.background.unwrap_or([0.0; 4]),
                new_render.background.unwrap_or([0.0; 4]),
                style.background_transition.as_ref(),
                now,
            );
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::TextColor,
                old_render.color,
                new_render.color,
                style.color_transition.as_ref(),
                now,
            );
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::BorderTopColor,
                old_render.border_color[0],
                new_render.border_color[0],
                style.border_top_color_transition.as_ref(),
                now,
            );
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::BorderRightColor,
                old_render.border_color[1],
                new_render.border_color[1],
                style.border_right_color_transition.as_ref(),
                now,
            );
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::BorderBottomColor,
                old_render.border_color[2],
                new_render.border_color[2],
                style.border_bottom_color_transition.as_ref(),
                now,
            );
            maybe_animate_color(
                &mut self.animator,
                layout,
                AnimProperty::BorderLeftColor,
                old_render.border_color[3],
                new_render.border_color[3],
                style.border_left_color_transition.as_ref(),
                now,
            );
        }

        // Position::Sticky → register against the enclosing
        // ScrollView so the render walker pins this node when
        // scrolled past the threshold. Any other Position value
        // (or `None`) deregisters so a previous Sticky →
        // {Relative, Absolute} transition cleans up its registry
        // entry. Mirrors iOS's `apply_style` sticky branch (see
        // `backend/ios/mobile/src/imp/mod.rs`). Idempotent — the
        // registry's `insert` replaces any existing entry, so a
        // re-apply with a new `top` value updates the threshold.
        //
        // We register here (post-`set_style`) rather than at
        // create-time so the lookup walks the freshly-applied
        // style. The framework's walker fires `apply_style`
        // BEFORE the parent's `insert(parent, child)` call, so on
        // a first mount the ancestor walk won't yet find a
        // scrolling parent — the registry entry's
        // `scroll_layout` is `None`, the render walker no-ops,
        // and once the tree settles a subsequent style re-apply
        // (state overlay, theme swap, hot patch) re-registers
        // with a populated `scroll_layout`. The fall-back-to-
        // relative behaviour matches CSS in the meantime.
        match style.position {
            Some(runtime_core::Position::Sticky) => {
                let threshold_top = crate::sticky::threshold_top_from_style(style);
                // Split-borrow the registry against `layout` and
                // `roots` — `register` walks Taffy parents to
                // resolve the enclosing scroll view, which would
                // otherwise alias `&mut self`.
                let WgpuBackend { sticky_registry, layout, roots, .. } = self;
                crate::sticky::register(
                    sticky_registry,
                    layout,
                    roots,
                    node,
                    threshold_top,
                );
            }
            _ => {
                crate::sticky::deregister(&mut self.sticky_registry, node);
            }
        }

        if is_text {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.set_font_size(&mut fs, layout, font_size);
            // Re-shape with the resolved font attributes. Cheap when
            // attrs haven't changed (the store's set_attrs returns
            // early on equal-attrs); the welcome's stylesheet picks
            // up `INTER` here so the headline / subtitle stop falling
            // back to cosmic-text's SansSerif.
            let attrs = crate::text::TextAttrs {
                family: new_render.font_family.clone(),
                weight: new_render.font_weight,
                style: new_render.font_style,
                align: new_render.text_align,
            };
            text.set_attrs(&mut fs, layout, attrs);
            drop(text);
            drop(fs);
            self.layout.mark_dirty(layout);
        }
        request_redraw();
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Consult the registry. SDK leaves (a future
        // `webview-wgpu`, `maps-wgpu`, etc.) call
        // `register_external::<TheirProps, _>(handler)` at bootstrap;
        // when one matches, the handler renders via the engine's
        // existing primitives (the overlay-per-host path is the
        // separate `project_wgpu_external_strategy` follow-up that
        // would mount real WebKit / MapKit views via native
        // overlays — not needed for SDKs that draw their own
        // visuals through wgpu).
        //
        // No-match: visible "kind X not registered" text so author
        // code that mounted an external sees the missing wiring
        // at runtime instead of an empty rect.
        if let Some(handler) = self.external_handlers.get(type_id) {
            let node = handler(payload, self);
            init_node_a11y(&node, a11y, PrimitiveKind::External);
            return node;
        }
        let msg = format!(
            "External \"{type_name}\" not registered on wgpu \
             — SDK leaf needs `register_external(&mut backend)` \
             on wgpu targets"
        );
        self.create_text(&msg, a11y)
    }

    fn release_external(&mut self, _node: &Self::Node) {
        // No per-external bookkeeping today. Future SDKs that hold
        // GPU resources (custom render targets, sampler caches) can
        // clean up here keyed by the node's layout id.
    }

    fn create_navigator(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        presentation: Rc<dyn std::any::Any>,
        host: runtime_core::primitives::navigator::NavigatorHost<Self::Node>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Same registry shape as macOS — `register_navigator`
        // installs a factory keyed by presentation TypeId; we run
        // `init`, stash the handler under the resolved node's
        // pointer for follow-up dispatch.
        if let Some(factory) = self.navigator_handlers.get(type_id) {
            let mut handler: Box<dyn runtime_core::NavigatorHandler<WgpuBackend>> =
                (factory)();
            let node = handler.init(self, host, presentation);
            let key = Rc::as_ptr(&node) as usize;
            self.nav_handler_instances.insert(
                key,
                std::rc::Rc::new(std::cell::RefCell::new(handler)),
            );
            init_node_a11y(&node, a11y, PrimitiveKind::View);
            return node;
        }
        let msg = format!(
            "Navigator kind \"{type_name}\" not registered on wgpu \
             — SDK leaf needs `register_navigator(&mut backend)` \
             on wgpu targets"
        );
        self.create_text(&msg, a11y)
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        let key = Rc::as_ptr(node) as usize;
        if let Some(handler_cell) = self.nav_handler_instances.remove(&key) {
            handler_cell.borrow_mut().release(self);
        }
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        let key = Rc::as_ptr(navigator) as usize;
        if let Some(handler_cell) = self.nav_handler_instances.get(&key).cloned() {
            handler_cell
                .borrow_mut()
                .attach_initial(self, screen, scope_id, options);
        }
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let key = Rc::as_ptr(node) as usize;
        if let Some(handler_cell) = self.nav_handler_instances.get(&key).cloned() {
            handler_cell
                .borrow_mut()
                .apply_slot_style(self, slot, style);
        }
    }

    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::navigator::NavigatorHandle {
        let key = Rc::as_ptr(node) as usize;
        if let Some(handler_cell) = self.nav_handler_instances.get(&key) {
            return handler_cell.borrow().make_handle();
        }
        runtime_core::primitives::navigator::NavigatorHandle::new(
            std::rc::Rc::new(()),
            &NOOP_WGPU_NAV_OPS,
        )
    }
}

/// Inert `NavigatorOps` for `make_navigator_handle` calls that land
/// on a navigator container with no registered handler. Empty trait,
/// so the impl is just a marker; handles built from this ignore all
/// dispatch attempts.
struct NoopWgpuNavOps;
impl runtime_core::primitives::navigator::NavigatorOps for NoopWgpuNavOps {}
static NOOP_WGPU_NAV_OPS: NoopWgpuNavOps = NoopWgpuNavOps;

// =========================================================================
// Accessibility — node-side stash + semantics-tree construction.
// =========================================================================

/// Stash the framework's `AccessibilityProps` (and the primitive's
/// inferred default role) on a freshly-created wgpu node. Called from
/// every `Backend::create_*` immediately after the `new_node` ctor so
/// the node carries its a11y state from the moment it enters the
/// tree.
///
/// The wgpu backend has no platform widget to attach a11y to, so the
/// data is kept verbatim on `NodeData` and surfaced via
/// [`Backend::dump_accessibility_tree`] later. See
/// `docs/accessibility-design.md` §5.
pub(crate) fn init_node_a11y(node: &WgpuNode, a11y: &AccessibilityProps, kind: PrimitiveKind) {
    let mut data = node.borrow_mut();
    data.accessibility = a11y.clone();
    data.inferred_role = default_role(kind);
}

/// Build an [`AccessibilityNode`] subtree rooted at `node`. Walks
/// `NodeData.children` in insertion order — wgpu has no z-index
/// reordering yet, so insertion order matches the visual top-to-
/// bottom, left-to-right traversal a screen-reader expects. When
/// z-ordering lands the walk should switch to a layout-coord
/// traversal-order pass per the design doc.
///
/// Bounds are pulled fresh from Taffy on every call. `frame_of`
/// returns the **parent-relative** rect; the host shell is
/// responsible for accumulating origins (it owns the
/// surface-to-platform coordinate transform). Returning local rects
/// matches what `Backend::frame` already exposes.
///
/// Node id is the pointer address of the `Rc<RefCell<NodeData>>`. It
/// is stable for the node's lifetime (the Rc is never reallocated
/// once `create_*` returns) and unique across live nodes (no two
/// distinct `Rc`s share an address). Drops + new allocations may
/// reuse the address, but the host shell diffs per layout-commit
/// against the **current** tree; an id collision can only happen
/// when the old node is already gone from the tree.
fn build_a11y_node(layout: &LayoutTree, node: &WgpuNode) -> AccessibilityNode {
    let frame = layout.frame_of(node.borrow().layout);
    let (props, role, children) = {
        let data = node.borrow();
        let role = data.accessibility.role.or(data.inferred_role).unwrap_or(Role::Group);
        let children: Vec<WgpuNode> = data.children.clone();
        (data.accessibility.clone(), role, children)
    };
    let id = Rc::as_ptr(node) as usize as u64;
    AccessibilityNode {
        id,
        props,
        role,
        bounds: AccessibilityRect {
            x: frame.x,
            y: frame.y,
            width: frame.width,
            height: frame.height,
        },
        children: children
            .iter()
            .map(|child| build_a11y_node(layout, child))
            .collect(),
    }
}

// =========================================================================
// Global self-handle — lets the framework's animation subscribers
// (welcome's `drive_av`, etc.) reach the backend from outside the
// `Backend` borrow window. Same shape as iOS's `IOS_BACKEND_SELF`:
// thread-local Weak to the outer `Rc<RefCell<WgpuBackend>>` set once
// at backend construction.
// =========================================================================

thread_local! {
    static WGPU_BACKEND_SELF: std::cell::RefCell<Option<std::rc::Weak<RefCell<WgpuBackend>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the backend's self-reference. The wgpu host (`Host::new`)
/// calls this once after wrapping the backend in `Rc<RefCell<>>`.
/// Subsequent calls overwrite the previous install — the most recent
/// host wins, which matches the single-active-host assumption the
/// renderer already makes (one global scheduler hook, one global skin).
pub fn install_global_self(weak: std::rc::Weak<RefCell<WgpuBackend>>) {
    WGPU_BACKEND_SELF.with(|s| {
        *s.borrow_mut() = Some(weak);
    });
}

/// Clone-out the currently-installed Weak, if any. Used internally by
/// the `*Ops` handles to reach the backend's Taffy state from the
/// type-erased side.
pub(crate) fn global_self() -> Option<std::rc::Weak<RefCell<WgpuBackend>>> {
    WGPU_BACKEND_SELF.with(|s| s.borrow().clone())
}

/// Push a scalar animation property update to `node` through the
/// installed global backend. Cross-platform animation subscribers
/// call this when they detect a wgpu node handle. Same shape as
/// `backend_ios::set_animated_f32`.
///
/// Quietly no-ops if no backend has been installed (pre-render), the
/// install has been dropped (post-teardown), or the backend is
/// already borrowed (an in-flight Rust call will see the new value
/// at its next frame — no harm done).
pub fn set_animated_f32(
    node: &crate::node::WgpuNode,
    prop: runtime_core::animation::AnimProp,
    value: f32,
) {
    let Some(weak) = global_self() else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        use runtime_core::Backend;
        b.set_animated_f32(node, prop, value);
    };
}

/// Color-family counterpart of [`set_animated_f32`].
pub fn set_animated_color(
    node: &crate::node::WgpuNode,
    prop: runtime_core::animation::AnimProp,
    value: [f32; 4],
) {
    let Some(weak) = global_self() else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        use runtime_core::Backend;
        b.set_animated_color(node, prop, value);
    };
}

/// `GraphicsOps` impl for wgpu. Unit struct that lets
/// `make_graphics_handle` hand a `&'static dyn GraphicsOps`
/// reference back through the framework's `GraphicsHandle`. No
/// imperative ops today; future host→author commands (resize
/// hints, capture-frame) would land here.
struct WgpuGraphicsOps;
impl runtime_core::primitives::graphics::GraphicsOps for WgpuGraphicsOps {}

/// Install a per-frame draw closure on a `GraphicsHandle`'s
/// node. The handle must be obtained from
/// `runtime_core::primitives::graphics::graphics(...).bind(ref)`
/// + the framework's `Ref<GraphicsHandle>::get()` after mount.
///
/// The closure is invoked from the renderer's pre-pass each
/// frame with a [`GraphicsFrame`] holding the shared
/// `wgpu::Device` / `Queue`, the node's offscreen texture
/// view, and the elapsed time since the node was created. The
/// closure encodes draw calls against `frame.encoder` and
/// returns — the host owns the queue submit and composites the
/// resulting texture into the main UI walk.
///
/// Calling this on a non-`Graphics` handle (or a `GraphicsHandle`
/// produced by a different backend's `make_graphics_handle`) is a
/// no-op — the downcast silently fails. Calling it twice
/// replaces the previously-installed drawer (the old closure
/// drops at end-of-statement).
pub fn register_graphics_drawer(
    handle: &runtime_core::primitives::graphics::GraphicsHandle,
    drawer: crate::node::GraphicsDrawer,
) {
    let Some(wgpu_node) = handle.node().downcast_ref::<WgpuNode>() else {
        return;
    };
    if let NodeKind::Graphics { drawer: slot, .. } = &wgpu_node.borrow().kind {
        *slot.borrow_mut() = Some(drawer);
        request_redraw();
    }
}

/// Convenience builder: construct a `Graphics` primitive whose
/// drawer is wired up automatically when the node mounts. Hides
/// the boilerplate of creating a `Ref<GraphicsHandle>`,
/// `.bind(...)`-ing it, and threading a second closure through
/// to `register_graphics_drawer` from an Effect. Authors who
/// need the live `GraphicsHandle` for other imperative ops can
/// still go through the framework's `graphics(...).bind(r)`
/// path and call [`register_graphics_drawer`] manually.
pub fn graphics_with_drawer<D>(
    drawer: D,
) -> runtime_core::Bound<runtime_core::primitives::graphics::GraphicsHandle>
where
    D: FnMut(&mut crate::node::GraphicsFrame) + 'static,
{
    let mut prim = runtime_core::primitives::graphics::graphics(|_| {});
    // Re-encode the drawer as a `RefFill::Graphics` closure: the
    // framework fires that closure during mount with the
    // backend-built `GraphicsHandle`. We hand it straight to
    // `register_graphics_drawer` so the per-frame pre-pass picks
    // it up starting from the next render. Bypasses `.bind(r)` —
    // the author doesn't need a `Ref` for this case.
    let drawer_box: crate::node::GraphicsDrawer = Box::new(drawer);
    if let runtime_core::Element::Graphics { ref_fill, .. } = prim.primitive_mut() {
        *ref_fill = Some(runtime_core::RefFill::Graphics(Box::new(
            move |h: runtime_core::primitives::graphics::GraphicsHandle| {
                register_graphics_drawer(&h, drawer_box);
            },
        )));
    }
    prim
}

/// Start a color tween for `property` on `node` if the supplied
/// transition spec exists and the value actually changed. No-op
/// otherwise (the new value already lives in `RenderStyle` and
/// will be sampled as the fallback).
fn maybe_animate_color(
    animator: &mut Animator,
    node: LayoutNode,
    property: AnimProperty,
    old_value: [f32; 4],
    new_value: [f32; 4],
    transition: Option<&runtime_core::Transition>,
    now: Instant,
) {
    let Some(t) = transition else { return };
    animator.animate_color(
        TweenKey::new(node, property),
        new_value,
        old_value,
        t.duration_ms,
        t.easing,
        now,
    );
}

/// Cheap check used to short-circuit the `defaults.merge(...)` +
/// `Rc::new` allocation when the skin returns no defaults. A
/// proper `StyleRules::is_empty()` would be nicer but every field
/// is `Option<_>` so a per-field scan would balloon; covering the
/// handful of fields skins actually set keeps this hot path tight.
fn defaults_are_empty(r: &StyleRules) -> bool {
    r.background.is_none()
        && r.color.is_none()
        && r.font_size.is_none()
        && r.font_weight.is_none()
        && r.padding_top.is_none()
        && r.padding_right.is_none()
        && r.padding_bottom.is_none()
        && r.padding_left.is_none()
        && r.border_top_left_radius.is_none()
        && r.border_top_right_radius.is_none()
        && r.border_bottom_left_radius.is_none()
        && r.border_bottom_right_radius.is_none()
}

fn drop_subtree(
    layout: &mut LayoutTree,
    text: &Rc<RefCell<TextStore>>,
    animator: &mut Animator,
    spinner_count: &mut u32,
    sticky_registry: &mut crate::sticky::StickyRegistry,
    node: &WgpuNode,
) {
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in &children {
        drop_subtree(layout, text, animator, spinner_count, sticky_registry, child);
    }
    let id = node.borrow().layout;
    if matches!(node.borrow().kind, NodeKind::ActivityIndicator { .. }) {
        *spinner_count = spinner_count.saturating_sub(1);
    }
    // Drop the sticky registry entry (if any). Sticky entries
    // hold a layout-id pair (`child_layout` + `scroll_layout`)
    // that are about to be removed from Taffy below; leaving the
    // entry behind would have `refresh_layout_positions` read
    // stale slots on the next layout pass.
    let node_key = Rc::as_ptr(node) as usize;
    crate::sticky::deregister_by_ptr(sticky_registry, node_key);
    text.borrow_mut().remove(id);
    animator.drop_node(id);
    layout.remove_node(id);
    // Header title buffer + its free-standing layout key live
    // outside the Taffy parent chain — clean them up explicitly
    // so dropping a popped screen doesn't leak a glyph buffer.
    if let Some(title_id) = node.borrow().screen_title_layout {
        text.borrow_mut().remove(title_id);
        layout.remove_node(title_id);
    }
}

/// `NodeData.style` is held even though only `render` is read on
/// the hot path — future state-overlay / transition passes will
/// re-derive from `style` without re-allocating.
const _: fn() = || {
    let _: fn(&NodeData) -> Option<&Rc<StyleRules>> = |n| n.style.as_ref();
};

/// Stamp the framework's current safe-area insets onto `node`'s
/// Taffy padding rules. Combines with the author's most-recently-
/// applied padding (matches iOS `contentInset` semantics — combine,
/// don't clobber).
fn apply_safe_area_to_node(
    layout: &mut LayoutTree,
    node: &WgpuNode,
    sides: runtime_core::SafeAreaSides,
    _as_padding: bool,
) {
    use runtime_core::{Length, SafeAreaSides};
    let insets = runtime_core::safe_area_insets().get();
    let author = node.borrow().style.clone();
    let author_padding = |t: Option<&Tokenized<Length>>| -> f32 {
        t.and_then(|t| match t.resolve() {
            Length::Px(v) => Some(v),
            _ => None,
        })
        .unwrap_or(0.0)
    };
    let (ap_top, ap_right, ap_bottom, ap_left) = if let Some(s) = author.as_ref() {
        (
            author_padding(s.padding_top.as_ref()),
            author_padding(s.padding_right.as_ref()),
            author_padding(s.padding_bottom.as_ref()),
            author_padding(s.padding_left.as_ref()),
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };
    let combine = |flag: SafeAreaSides, base: f32, inset: f32| -> Tokenized<Length> {
        let total = if sides.contains(flag) { base + inset } else { base };
        Tokenized::Literal(Length::Px(total))
    };
    let rules = StyleRules {
        padding_top: Some(combine(SafeAreaSides::TOP, ap_top, insets.top)),
        padding_right: Some(combine(SafeAreaSides::RIGHT, ap_right, insets.right)),
        padding_bottom: Some(combine(SafeAreaSides::BOTTOM, ap_bottom, insets.bottom)),
        padding_left: Some(combine(SafeAreaSides::LEFT, ap_left, insets.left)),
        ..Default::default()
    };
    let id = node.borrow().layout;
    layout.set_style(id, &rules);
    layout.mark_dirty(id);
}

/// Stub: navigator transitions are not supported on this backend
/// yet (legacy nav substrate was removed; per-kind SDK paths will
/// repopulate this when wired up). Returns `false` so the host's
/// tick loop doesn't keep redrawing.
pub(crate) fn tick_nav_transitions(
    _backend: &Rc<RefCell<WgpuBackend>>,
    _now: Instant,
) -> bool {
    false
}

/// Stub: drawer animations are not supported on this backend yet.
/// See [`tick_nav_transitions`] for context.
pub(crate) fn drawer_anim_alive(_backend: &Rc<RefCell<WgpuBackend>>) -> bool {
    false
}

/// Capture the four presence-animatable values currently rendered
/// for `node`. Reads from `AnimatedOverrides` and falls back to each
/// property's identity (1.0 / 0.0 / 1.0) when no override is active.
///
/// Used by `apply_presence` to establish the `from` end of a tween,
/// so a mid-animation reversal lerps from the currently-visible
/// state rather than the start state of the cancelled tween.
fn read_presence_state(node: &WgpuNode) -> PresenceSnapshot {
    let data = node.borrow();
    let av = data.animated.as_deref();
    PresenceSnapshot {
        opacity: av.and_then(|a| a.opacity).unwrap_or(1.0),
        translate_x: av.and_then(|a| a.translate_x).unwrap_or(0.0),
        translate_y: av.and_then(|a| a.translate_y).unwrap_or(0.0),
        // Read scale from the X axis — `apply_presence` always
        // writes both axes from a single `state.scale`, so reading
        // either is equivalent. Authors who set scale_x/scale_y
        // independently via `set_animated_f32` will see the X axis
        // as "the" presence scale at the next reversal; that's
        // acceptable because the presence API exposes only uniform
        // scale (per `PresenceState`'s field shape).
        scale: av.and_then(|a| a.scale_x).unwrap_or(1.0),
    }
}

/// Resolve a `PresenceState` against the current rendered state to
/// produce a concrete `PresenceSnapshot` target. `None` fields in
/// the input pull from the identity values, NOT from `current` —
/// this matches the web leaf's behavior of "fields not declared on
/// `state` snap back to rest" (web clears the inline `style`
/// property, which reveals the stylesheet rest value).
fn resolve_presence_target(
    state: runtime_core::primitives::presence::PresenceState,
    _current: PresenceSnapshot,
) -> PresenceSnapshot {
    let rest = PresenceSnapshot::rest();
    PresenceSnapshot {
        opacity: state.opacity.unwrap_or(rest.opacity),
        translate_x: state.translate_x.unwrap_or(rest.translate_x),
        translate_y: state.translate_y.unwrap_or(rest.translate_y),
        scale: state.scale.unwrap_or(rest.scale),
    }
}

/// Write a presence target snapshot to `node`'s AnimatedOverrides.
/// Honors the original `PresenceState`'s `Option` shape: fields
/// declared by the author get the resolved target value; fields
/// the author left as `None` are cleared back to `None` on the
/// override so the renderer falls through to the stylesheet rest
/// value (identical to the web leaf's `style.remove_property`).
fn write_presence_overrides(
    node: &WgpuNode,
    state: &runtime_core::primitives::presence::PresenceState,
    target: PresenceSnapshot,
) {
    let mut data = node.borrow_mut();
    let ov = data
        .animated
        .get_or_insert_with(|| Box::new(crate::node::AnimatedOverrides::default()));
    ov.opacity = state.opacity.map(|_| target.opacity);
    ov.translate_x = state.translate_x.map(|_| target.translate_x);
    ov.translate_y = state.translate_y.map(|_| target.translate_y);
    if state.scale.is_some() {
        ov.scale_x = Some(target.scale);
        ov.scale_y = Some(target.scale);
    } else {
        ov.scale_x = None;
        ov.scale_y = None;
    }
}

/// Per-frame interpolation step for active presence tweens. Returns
/// `true` while any tween is still alive so the host's render loop
/// keeps requesting redraws.
///
/// Called from [`crate::host::Host::tick`] before navigator
/// transitions and momentum scrolling, so the per-tween writes are
/// composited into the frame the host is about to submit.
pub(crate) fn tick_presence_tweens(
    backend: &Rc<RefCell<WgpuBackend>>,
    now: Instant,
) -> bool {
    let b = backend.borrow();
    if b.presence_tweens.is_empty() {
        return false;
    }
    let mut done: Vec<usize> = Vec::new();
    // Collect node + interpolated values first; we need to release
    // the outer borrow on `b.presence_tweens` before reaching into
    // each node's RefCell (a node can't be borrowed mutably while we
    // also hold an immutable borrow on the tween map's iterator).
    let mut updates: Vec<(WgpuNode, PresenceSnapshot, bool)> =
        Vec::with_capacity(b.presence_tweens.len());
    for (key, tween) in b.presence_tweens.iter() {
        let elapsed = now.saturating_duration_since(tween.started);
        let raw_t = if tween.duration.is_zero() {
            1.0
        } else {
            (elapsed.as_secs_f32() / tween.duration.as_secs_f32()).clamp(0.0, 1.0)
        };
        let t = runtime_core::animation::apply_easing(raw_t, tween.easing);
        let sample = PresenceSnapshot {
            opacity: lerp(tween.from.opacity, tween.to.opacity, t),
            translate_x: lerp(tween.from.translate_x, tween.to.translate_x, t),
            translate_y: lerp(tween.from.translate_y, tween.to.translate_y, t),
            scale: lerp(tween.from.scale, tween.to.scale, t),
        };
        let finished = elapsed >= tween.duration;
        if finished {
            done.push(*key);
        }
        updates.push((tween.node.clone(), sample, finished));
    }
    drop(b);

    for (node, sample, finished) in updates {
        let mut data = node.borrow_mut();
        let ov = data
            .animated
            .get_or_insert_with(|| Box::new(crate::node::AnimatedOverrides::default()));
        ov.opacity = Some(sample.opacity);
        ov.translate_x = Some(sample.translate_x);
        ov.translate_y = Some(sample.translate_y);
        ov.scale_x = Some(sample.scale);
        ov.scale_y = Some(sample.scale);
        // On the final tick, clear overrides whose `to` matches the
        // rest identity. Without this, a presence-driven exit would
        // leave permanent `opacity = 1.0` / `translate = 0` writes
        // that block subsequent stylesheet-driven changes from
        // taking effect (the override always wins). Match what the
        // web leaf does: at rest, remove the inline property so the
        // node falls through to the stylesheet.
        if finished {
            if (sample.opacity - 1.0).abs() < f32::EPSILON {
                ov.opacity = None;
            }
            if sample.translate_x.abs() < f32::EPSILON {
                ov.translate_x = None;
            }
            if sample.translate_y.abs() < f32::EPSILON {
                ov.translate_y = None;
            }
            if (sample.scale - 1.0).abs() < f32::EPSILON {
                ov.scale_x = None;
                ov.scale_y = None;
            }
        }
    }

    let mut b = backend.borrow_mut();
    for key in done {
        b.presence_tweens.remove(&key);
    }
    !b.presence_tweens.is_empty()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ---------------------------------------------------------------------------
// Accessibility tests — `docs/accessibility-design.md` §5.
//
// Exercise the parallel-semantics-tree contract end-to-end at the
// backend layer:
//
// 1. `create_*` stashes `AccessibilityProps` + the inferred role on
//    each node.
// 2. `insert` keeps the parent→child relationship in sync.
// 3. `dump_accessibility_tree` materialises an `AccessibilityTree`
//    that matches the visual tree (root + children, roles, labels,
//    bounds from Taffy).
// 4. `update_accessibility` swaps a node's prop bag and the next
//    dump reflects the change.
// 5. `announce_for_accessibility` queues one-shot announcements that
//    `drain_pending_announcements` returns in order, exactly once.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod a11y_tests {
    use super::*;
    use runtime_core::accessibility::{
        AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
    };
    use runtime_core::ColorScheme;

    /// Standalone `Painter` for headless accessibility tests. Implements
    /// the full trait surface with no-op paints — the a11y tests
    /// never enter the renderer, so the visual paths are dead code
    /// at this scope. Kept local rather than pulled into a shared
    /// helper because `host::tests` carries its own (stale) test-skin
    /// and unifying them would entangle two unrelated changes.
    struct TestPainter;
    impl crate::painter::Painter for TestPainter {
        fn paint_toggle(
            &self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _t: f32,
            _tint: Option<[f32; 4]>,
            _rects: &mut Vec<crate::pipeline::Instance>,
        ) {
        }
        fn paint_slider(
            &self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _value: f32,
            _min: f32,
            _max: f32,
            _tint: Option<[f32; 4]>,
            _rects: &mut Vec<crate::pipeline::Instance>,
        ) {
        }
        fn paint_text_input<'a>(
            &self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _is_focused: bool,
            _draw_caret: bool,
            _is_placeholder: bool,
            _buffer: &'a glyphon::Buffer,
            _caret_x_local: f32,
            _text_color: [f32; 4],
            _field_bg: Option<[f32; 4]>,
            _rects: &mut Vec<crate::pipeline::Instance>,
            _texts: &mut Vec<crate::text::StagedText<'a>>,
        ) {
        }
        fn paint_activity_indicator(
            &self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _phase: f32,
            _tint: Option<[f32; 4]>,
            _rects: &mut Vec<crate::pipeline::Instance>,
        ) {
        }
        fn keyboard_rows(&self) -> Vec<Vec<crate::keyboard::KeySpec>> {
            Vec::new()
        }
        fn keyboard_layout_metrics(&self) -> crate::keyboard::LayoutMetrics {
            crate::keyboard::LayoutMetrics {
                key_gap: 0.0,
                row_gap: 0.0,
                side_margin: 0.0,
                vert_margin: 0.0,
            }
        }
        fn paint_keyboard<'a>(
            &self,
            _keyboard_rect: (f32, f32, f32, f32),
            _laid_keys: &[crate::keyboard::LaidKey],
            _pressed_label: Option<&'static str>,
            _glyphs: &'a std::collections::HashMap<&'static str, glyphon::Buffer>,
            _rects: &mut Vec<crate::pipeline::Instance>,
            _texts: &mut Vec<crate::text::StagedText<'a>>,
        ) {
        }
        fn paint_navigator_header<'a, 'b>(
            &self,
            _rect: (f32, f32, f32, f32),
            _chrome: crate::painter::NavigatorHeaderChrome<'a, 'b>,
            _rects: &mut Vec<crate::pipeline::Instance>,
            _texts: &mut Vec<crate::text::StagedText<'a>>,
            _hit_regions: &mut Vec<crate::painter::NavigatorHeaderHit>,
        ) {
        }
    }

    fn make_backend() -> WgpuBackend {
        let text = Rc::new(RefCell::new(crate::text::TextStore::new()));
        let fs = Rc::new(RefCell::new(glyphon::FontSystem::new()));
        WgpuBackend::new(text, fs, ColorScheme::Light, Rc::new(TestPainter))
    }

    /// Regression test: a `Bundled { path }` font asset must be loaded
    /// from disk on native. In runtime-server mode every font reaches
    /// the (headless) wgpu backend over the wire as `Bundled { path }`
    /// — bytes stripped for transport — so if this arm only queued a
    /// web-fetch URL (the pre-fix behavior), the shaper would be left
    /// with just the bundled default weight and bold/italic text would
    /// fall back to a wrong font, making a headless screenshot diverge
    /// from the real iOS/Android render.
    ///
    /// `cargo test` runs with the crate dir as CWD, so the bundled
    /// Inter-Regular under `assets/fonts/` resolves via the same
    /// app-relative path the recorder would emit.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn bundled_font_is_loaded_from_disk_on_native() {
        use runtime_core::{AssetId, AssetSource, AssetTag, Backend};

        let mut b = make_backend();
        let before = b.font_system.borrow().db().len();
        b.register_asset(
            AssetId(1),
            AssetTag::Font,
            &AssetSource::Bundled {
                path: "assets/fonts/Inter-Regular.ttf",
            },
        );
        let after = b.font_system.borrow().db().len();
        assert!(
            after > before,
            "a Bundled font must be loaded into the shaper's DB on native \
             (faces before={before}, after={after}); otherwise headless \
             screenshots render registered weights with a fallback font"
        );
    }

    /// Build a small View(Text) tree directly through the Backend
    /// trait, populate Taffy intrinsic sizes so layout produces
    /// non-zero rects, and compute layout against a 200×100 box.
    /// Returns (backend, root) — root is the View; root.children[0]
    /// is the Text.
    fn build_view_with_text() -> (WgpuBackend, WgpuNode) {
        let mut b = make_backend();
        let view_a11y = AccessibilityProps {
            label: Some("greeting card".into()),
            identifier: Some("greeting-card".into()),
            ..Default::default()
        };
        let text_a11y = AccessibilityProps {
            label: Some("Hello world".into()),
            ..Default::default()
        };
        let mut root = b.create_view(&view_a11y);
        let text = b.create_text("Hello world", &text_a11y);
        b.insert(&mut root, text);
        // Stamp intrinsic sizes so the layout pass produces
        // non-degenerate rects — `frame_of` returns the zero rect
        // when no compute has run. View fills, text takes 80×20.
        let view_layout = root.borrow().layout;
        let text_layout = root.borrow().children[0].borrow().layout;
        b.layout.set_intrinsic_size(text_layout, 80.0, 20.0);
        b.layout.compute(view_layout, 200.0, 100.0);
        b.finish(root.clone());
        (b, root)
    }

    #[test]
    fn dump_tree_reflects_view_with_text_child() {
        let (b, root) = build_view_with_text();
        let tree = b.dump_accessibility_tree().expect("tree present after mount");

        // Root: View carries the custom label / identifier and the
        // walker's `default_role(View)` returns `None` → resolved
        // role falls back to `Group` per `build_a11y_node`.
        assert_eq!(tree.root.props.label.as_deref(), Some("greeting card"));
        assert_eq!(tree.root.props.identifier.as_deref(), Some("greeting-card"));
        assert_eq!(tree.root.role, Role::Group);
        assert_eq!(tree.root.id, Rc::as_ptr(&root) as usize as u64);
        // Root bounds match the 200×100 compute box.
        assert_eq!(tree.root.bounds.width, 200.0);
        assert_eq!(tree.root.bounds.height, 100.0);
        // Origin is in the parent's coord space — root is its own
        // parent, so (0, 0).
        assert_eq!(tree.root.bounds.x, 0.0);
        assert_eq!(tree.root.bounds.y, 0.0);

        // Child: Text node carries its label + the inferred Text role.
        assert_eq!(tree.root.children.len(), 1);
        let text_node = &tree.root.children[0];
        assert_eq!(text_node.props.label.as_deref(), Some("Hello world"));
        assert_eq!(text_node.role, Role::Text);
        // Bounds come straight from Taffy's computed frame. We don't
        // pin exact dimensions — flex stretch interacts with
        // `set_intrinsic_size`'s `min_size`-only effect in ways
        // orthogonal to the a11y data path. What matters is:
        //   - non-zero size (Taffy ran and the rect made it through),
        //   - height respects the intrinsic minimum (>= 20),
        //   - rect lives inside the parent's 200×100 box.
        assert!(text_node.bounds.height >= 20.0);
        assert!(text_node.bounds.width > 0.0);
        assert!(text_node.bounds.x >= 0.0);
        assert!(text_node.bounds.y >= 0.0);
        assert!(text_node.bounds.x + text_node.bounds.width <= tree.root.bounds.width);
        assert!(text_node.bounds.y + text_node.bounds.height <= tree.root.bounds.height);
        // Distinct id from the parent — the pointer-address scheme
        // guarantees this whenever the two nodes are alive together.
        assert_ne!(text_node.id, tree.root.id);
        // Text is a leaf in this tree.
        assert!(text_node.children.is_empty());
    }

    #[test]
    fn dump_tree_is_none_before_mount() {
        let b = make_backend();
        assert!(b.dump_accessibility_tree().is_none());
    }

    #[test]
    fn announce_for_accessibility_drains_in_order() {
        let (mut b, _root) = build_view_with_text();
        // Drain before any announce — empty.
        assert!(b.drain_pending_announcements().is_empty());

        b.announce_for_accessibility("loading", LiveRegionPriority::Polite);
        b.announce_for_accessibility("complete", LiveRegionPriority::Assertive);

        let drained = b.drain_pending_announcements();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].0, "loading");
        assert_eq!(drained[0].1, LiveRegionPriority::Polite);
        assert_eq!(drained[1].0, "complete");
        assert_eq!(drained[1].1, LiveRegionPriority::Assertive);

        // Drain is one-shot — the queue is empty next call. This
        // matches the contract documented on
        // `drain_pending_announcements`: each entry fires once and
        // is gone, mirroring how platform AX announcement APIs work.
        let drained_again = b.drain_pending_announcements();
        assert!(drained_again.is_empty());

        // The semantics tree itself does NOT carry announcements —
        // they're separate concerns (tree is persistent, announcements
        // are transient one-shots). Sanity-check that announcing
        // doesn't accidentally mutate the tree shape.
        let tree = b.dump_accessibility_tree().expect("tree still present");
        assert_eq!(tree.root.children.len(), 1);
    }

    #[test]
    fn update_accessibility_replaces_prop_bag_on_next_dump() {
        let (mut b, root) = build_view_with_text();
        let text = root.borrow().children[0].clone();

        // Before: text role inferred, label "Hello world".
        {
            let tree = b.dump_accessibility_tree().expect("tree");
            let text_node = &tree.root.children[0];
            assert_eq!(text_node.props.label.as_deref(), Some("Hello world"));
            assert!(text_node.props.traits.is_empty());
        }

        // Patch via the Backend trait method that the framework's
        // reactive a11y Effect would call. The walker would pass
        // `PrimitiveKind::Text`'s default role here; we replicate
        // that for the test.
        let new_props = AccessibilityProps {
            label: Some("Greetings, world".into()),
            traits: AccessibilityTraits::SELECTED,
            ..Default::default()
        };
        b.update_accessibility(&text, &new_props, Some(Role::Text));

        // After: the next dump must reflect the swap.
        let tree = b.dump_accessibility_tree().expect("tree");
        let text_node = &tree.root.children[0];
        assert_eq!(text_node.props.label.as_deref(), Some("Greetings, world"));
        assert!(text_node.props.traits.contains(AccessibilityTraits::SELECTED));
        assert_eq!(text_node.role, Role::Text);

        // Re-dumping after a no-op call still produces the same
        // tree — no caching means stale data can never lag.
        let tree2 = b.dump_accessibility_tree().expect("tree");
        assert_eq!(tree2.root.children[0].props.label.as_deref(), Some("Greetings, world"));
    }

    /// Regression: before this landed, `apply_presence` was the
    /// `Backend` trait default no-op, so presence's enter/exit
    /// state writes (opacity / translate / scale) silently did
    /// nothing on the wgpu backend. The exit animation's snap to
    /// the exit state, the rest-target tween — all dropped.
    /// This test verifies that `apply_presence(None)` writes the
    /// declared properties to `AnimatedOverrides`, and that the
    /// Regression: before this landed, the wgpu backend's
    /// `register_asset` only handled `AssetTag::Font`. Image
    /// assets from `image_asset!()` (which the framework emits as
    /// `asset://{id}` URLs) silently dropped on the floor and the
    /// renderer's filesystem resolver fell through to "not found"
    /// → placeholder paint. This test verifies that
    /// `register_asset` for an Image asset stashes the bytes and
    /// `image_asset_bytes(id)` returns them. The renderer's
    /// integration with `decode_and_upload` can't be exercised
    /// without a real wgpu Device + Queue, so the visual decode
    /// path remains a manual on-device check.
    #[test]
    fn regression_wgpu_register_asset_caches_image_bytes() {
        use runtime_core::{AssetId, AssetSource, AssetTag};
        let mut b = make_backend();
        let id = AssetId(42);
        const BYTES: &[u8] = b"hello-image-bytes";
        b.register_asset(
            id,
            AssetTag::Image,
            &AssetSource::Embedded { bytes: BYTES, extension: "png" },
        );
        assert_eq!(
            b.image_asset_bytes(id),
            Some(BYTES),
            "register_asset(Image, Embedded) must populate the byte cache"
        );

        b.unregister_asset(id, AssetTag::Image);
        assert!(
            b.image_asset_bytes(id).is_none(),
            "unregister_asset(Image) must clear the byte cache so hot-reload re-decodes"
        );
    }

    /// Regression: with `embed-font-bytes` off (the web host path),
    /// `face!` emits a bytes-free `Bundled { path }` font. The old
    /// `register_asset` dropped those on the floor — the wgpu
    /// simulator then shaped every glyph against its single embedded
    /// default face, so the website's Bold/Medium/etc. weights never
    /// rendered. Now the backend queues each font's served-file URL
    /// (`/{path}`, matching the DOM backend's `@font-face` URL) for the
    /// async host shell to fetch + load. This test pins the URL
    /// collection + drain contract; the fetch itself lives in
    /// `host-web` and is exercised on-device.
    #[test]
    fn regression_wgpu_bundled_font_queues_served_url() {
        use runtime_core::{AssetId, AssetSource, AssetTag};
        let mut b = make_backend();

        // A bytes-free Bundled font (embed-font-bytes off) must be
        // queued as a root-absolute served URL, not dropped.
        b.register_asset(
            AssetId(1),
            AssetTag::Font,
            &AssetSource::Bundled { path: "fonts/Inter-Bold.ttf" },
        );
        // A Remote font is queued by its absolute URL verbatim.
        b.register_asset(
            AssetId(2),
            AssetTag::Font,
            &AssetSource::Remote { url: "https://cdn.example/Roboto.ttf" },
        );
        // A BundledEmbedded font (native, bytes inline) is loaded
        // synchronously and must NOT add to the fetch queue.
        b.register_asset(
            AssetId(3),
            AssetTag::Font,
            &AssetSource::BundledEmbedded {
                path: "fonts/Inter-Regular.ttf",
                bytes: b"not-a-real-font-but-load_font_data-tolerates-it",
                extension: "ttf",
            },
        );

        let urls = b.drain_pending_font_urls();
        assert_eq!(
            urls,
            vec![
                "/fonts/Inter-Bold.ttf".to_string(),
                "https://cdn.example/Roboto.ttf".to_string(),
            ],
            "Bundled fonts must queue `/{{path}}` and Remote its URL; \
             BundledEmbedded carries bytes and must not enqueue a fetch"
        );
        assert!(
            b.drain_pending_font_urls().is_empty(),
            "drain must empty the queue so a second host pass doesn't refetch"
        );
    }

    /// Regression: before this landed, `create_external` fell
    /// through to the framework's default `unimplemented!()`.
    /// Author code mounting Maps / WebView / any other external
    /// on a wgpu app crashed at mount. The fix is a visible
    /// "External X not yet implemented on wgpu" text placeholder
    /// so mounting succeeds even though the SDK rendering is
    /// pending — overlay-per-host strategy lands later. This test
    /// verifies the method produces a `NodeKind::Text` placeholder
    /// without panicking.
    ///
    /// `create_navigator` has the same fix but isn't tested here
    /// because constructing a real `NavigatorHost` requires
    /// supplying ~10 fake `Rc<dyn Fn>` callbacks; we'd test the
    /// scaffolding more than the behavior. The placeholder path
    /// is shape-identical to External's, so a future Navigator-
    /// substrate test can cover both at once.
    #[test]
    fn regression_wgpu_external_renders_placeholder_text() {
        use std::any::TypeId;
        use std::rc::Rc;

        let mut b = make_backend();
        let payload: Rc<dyn std::any::Any> = Rc::new(());
        let ext_node = b.create_external(
            TypeId::of::<()>(),
            "test_external",
            &payload,
            &AccessibilityProps::default(),
        );
        assert!(
            matches!(ext_node.borrow().kind, NodeKind::Text { .. }),
            "wgpu create_external must produce a Text placeholder, not panic"
        );
    }

    /// `Some(transition)` path enrolls a tween.
    #[test]
    fn regression_wgpu_apply_presence_writes_overrides_and_enrolls_tween() {
        use runtime_core::primitives::presence::PresenceState;

        let mut b = make_backend();
        let node = b.create_view(&AccessibilityProps::default());

        // Snap path. `state.opacity = Some(0.0)` should land on
        // `node.animated.opacity`; untouched fields stay None.
        b.apply_presence(
            &node,
            PresenceState::rest().opacity(0.0),
            None,
        );
        {
            let data = node.borrow();
            let ov = data
                .animated
                .as_ref()
                .expect("snap must allocate AnimatedOverrides");
            assert_eq!(
                ov.opacity,
                Some(0.0),
                "apply_presence(None) must write opacity to the override"
            );
            assert!(
                ov.translate_x.is_none(),
                "fields the author didn't declare on PresenceState must stay None"
            );
        }

        // Tween path. Should enroll an entry in `presence_tweens`,
        // keyed by the node's pointer.
        let key = Rc::as_ptr(&node) as usize;
        b.apply_presence(
            &node,
            PresenceState::rest(),
            Some((150, runtime_core::Easing::EaseInOut)),
        );
        assert!(
            b.presence_tweens.contains_key(&key),
            "Some(transition) must enroll a presence tween for this node"
        );
    }

    /// Regression: before this landed, `create_text_area` hit the
    /// framework's default `unimplemented!()`, so any author code
    /// that mounted a `TextArea` on the wgpu backend crashed at
    /// mount time. The minimal test is: call the method, verify it
    /// returns a `NodeKind::TextArea` node without panicking, then
    /// confirm `update_text_area_value` mutates the stored value
    /// (proving the update path lines up with the create path's
    /// NodeKind variant).
    #[test]
    fn regression_wgpu_text_area_creates_and_updates_without_panic() {
        let mut b = make_backend();
        let on_change_called: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let on_change_called_clone = on_change_called.clone();
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |s| {
            on_change_called_clone.borrow_mut().push(s);
        });
        let node = b.create_text_area(
            "initial",
            Some("placeholder"),
            true,
            on_change,
            None,
            &AccessibilityProps::default(),
        );
        assert!(
            matches!(node.borrow().kind, NodeKind::TextArea { .. }),
            "create_text_area must produce a NodeKind::TextArea, not a fallback"
        );
        b.update_text_area_value(&node, "updated");
        match &node.borrow().kind {
            NodeKind::TextArea { value, .. } => assert_eq!(value, "updated"),
            other => panic!("expected NodeKind::TextArea after update, got {other:?}"),
        }
        // on_change is invoked from the host's key dispatch, not from
        // update_text_area_value (which is the framework pushing
        // authoritative value INTO the backend). So the change
        // callback must NOT have been called by this code path —
        // that's the whole point of the controlled-component pattern.
        assert!(
            on_change_called.borrow().is_empty(),
            "update_text_area_value must not fire on_change (framework → backend pump only)"
        );
    }

    #[test]
    fn role_falls_back_to_inferred_when_props_role_is_none() {
        // A `Button` primitive with author label but no `role` override
        // should resolve to `Role::Button` via `default_role`, not the
        // `Group` ultimate-fallback.
        let mut b = make_backend();
        let a11y = AccessibilityProps {
            label: Some("Submit".into()),
            ..Default::default()
        };
        // Build an `Action` from a bare closure via `IntoAction`. The
        // closure path produces an Action with empty `method` /
        // `inputs` — fine for this test, which never fires the button.
        let action = runtime_core::IntoAction::into_action(|| {});
        let btn = b.create_button("Submit", &action, None, None, &a11y);
        let btn_layout = btn.borrow().layout;
        b.layout.set_intrinsic_size(btn_layout, 100.0, 30.0);
        b.layout.compute(btn_layout, 100.0, 30.0);
        b.finish(btn);

        let tree = b.dump_accessibility_tree().expect("tree");
        assert_eq!(tree.root.role, Role::Button);
        assert_eq!(tree.root.props.label.as_deref(), Some("Submit"));
    }
}

