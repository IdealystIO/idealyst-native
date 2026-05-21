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

use framework_core::primitives::activity_indicator::ActivityIndicatorSize;
use framework_core::{Action, Backend, Color, ColorScheme, Easing, StateBits, StyleRules, Tokenized};
use glyphon::FontSystem;
use native_layout::{AvailableSpace, LayoutNode, LayoutTree, Size as TaffySize};

use crate::animation::{AnimProperty, Animator, TweenKey};
use crate::node::{
    new_node, NodeData, NodeKind, WgpuNode, ACTIVITY_INDICATOR_LARGE_SIZE,
    ACTIVITY_INDICATOR_SMALL_SIZE, ICON_DEFAULT_SIZE, IMAGE_DEFAULT_SIZE,
    SLIDER_DEFAULT_WIDTH, SLIDER_HEIGHT, TEXT_INPUT_DEFAULT_HEIGHT, TOGGLE_ANIM_MS,
    TOGGLE_HEIGHT, TOGGLE_WIDTH, UNSUPPORTED_DEFAULT_HEIGHT,
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
    pub(crate) skin: Rc<dyn crate::skin::Skin>,
    /// Weak reference to *this* backend's outer `Rc<RefCell<Self>>`.
    /// Set once by `Host::new` immediately after the backend Rc
    /// is constructed. Lets navigator + tab + drawer command
    /// dispatchers (which run from user code outside the
    /// framework's borrow window) re-acquire a mutable borrow
    /// to insert / remove screens without re-entering the
    /// framework's build walker.
    pub(crate) self_weak: std::cell::OnceCell<std::rc::Weak<RefCell<WgpuBackend>>>,
}

impl WgpuBackend {
    pub fn new(
        text: Rc<RefCell<TextStore>>,
        font_system: Rc<RefCell<FontSystem>>,
        color_scheme: ColorScheme,
        skin: Rc<dyn crate::skin::Skin>,
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
        }
    }

    /// Snapshot of the active root, or `None` if nothing has been
    /// mounted yet. The renderer reads this on each frame.
    pub fn root(&self) -> Option<WgpuNode> {
        self.roots.last().cloned()
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

    fn color_scheme(&self) -> ColorScheme {
        self.color_scheme
    }

    fn create_view(&mut self) -> Self::Node {
        let layout = self.layout.new_node();
        let node = new_node(NodeKind::View, layout);
        self.roots.push(node.clone());
        node
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
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
        self.roots.push(node.clone());
        node
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&framework_core::primitives::icon::IconData>,
        _trailing_icon: Option<&framework_core::primitives::icon::IconData>,
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

    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>) -> Self::Node {
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
        self.roots.push(node.clone());
        node
    }

    fn install_touch_handler(
        &mut self,
        node: &Self::Node,
        handler: framework_core::TouchHandler,
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
        _on_key_down: Option<framework_core::primitives::key::KeyDownHandler>,
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

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        self.layout
            .set_intrinsic_size(layout, TOGGLE_WIDTH, TOGGLE_HEIGHT);
        let node = new_node(
            NodeKind::Toggle { value: initial_value, on_change },
            layout,
        );
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
        self.active_spinner_count = self.active_spinner_count.saturating_add(1);
        self.roots.push(node.clone());
        request_redraw();
        node
    }

    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
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
            },
            layout,
        );
        self.roots.push(node.clone());
        node
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        let layout = self.layout.new_node();
        let node = new_node(NodeKind::ReactiveAnchor, layout);
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
        config: framework_core::primitives::link::LinkConfig,
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
        self.roots.push(node.clone());
        node
    }

    // -----------------------------------------------------------
    // Image — placeholder for now. Stores src + alt so the
    // renderer can paint a labeled placeholder rect. A real
    // textured-quad pipeline is future work.
    // -----------------------------------------------------------

    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node {
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
        data: &framework_core::primitives::icon::IconData,
        color: Option<&Color>,
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
            },
            layout,
        );
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
        target: framework_core::primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
    ) -> Self::Node {
        if matches!(target, framework_core::primitives::portal::PortalTarget::Named(_)) {
            unimplemented!(
                "PortalTarget::Named is not supported by the wgpu backend"
            );
        }
        let layout = self.layout.new_node();
        let node = new_node(NodeKind::Portal { target, on_dismiss }, layout);
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
        callbacks: framework_core::VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        horizontal: bool,
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

    fn create_navigator(
        &mut self,
        callbacks: framework_core::primitives::navigator::NavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        // The navigator's container fills whatever box its parent
        // hands it. Its screen children are `position: Absolute`
        // with zero insets (see `mark_as_navigator_screen`), so
        // they resolve their fill against this container's box —
        // which needs definite dimensions for the percentage
        // insets to mean "fill the navigator". As the framework's
        // root this resolves against the viewport; wrapped inside
        // another View, it fills that View.
        self.layout.set_style(layout, &navigator_container_fill_rules());
        // Consume any per-call animator override installed via
        // `nav_anim::with_transition`. The override is one-shot
        // per `create_navigator` so nested navigators inside the
        // initial-screen build don't accidentally inherit it.
        let transition_anim = crate::nav_anim::take_transition_override()
            .unwrap_or_else(crate::nav_anim::default_transition);
        let node = new_node(
            NodeKind::Navigator {
                scope_ids: std::cell::RefCell::new(Vec::new()),
                control: control.clone(),
                transition: std::cell::RefCell::new(None),
                transition_anim,
                header_style: std::cell::RefCell::new(None),
                title_style: std::cell::RefCell::new(None),
                button_style: std::cell::RefCell::new(None),
                body_style: std::cell::RefCell::new(None),
            },
            layout,
        );
        install_navigator_dispatcher(
            &node,
            callbacks,
            control,
            self.self_weak.get().expect("self_weak set in Host::new").clone(),
        );
        self.roots.push(node.clone());
        node
    }

    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::navigator::NavigatorHandle {
        use framework_core::primitives::navigator::NavigatorHandle;
        if let NodeKind::Navigator { control, .. } = &node.borrow().kind {
            // Empty `()` userdata — the trait default carries the
            // same. The control is what makes `push` / `pop` /
            // `replace` / `reset` reach our installed dispatcher;
            // without it the handle returned here would be a
            // silent no-op (the cause of the prior "push does
            // nothing" symptom).
            return NavigatorHandle::with_control(
                Rc::new(()),
                &WgpuNavigatorOps,
                control.clone(),
            );
        }
        NavigatorHandle::new(Rc::new(()), &WgpuNavigatorOps)
    }

    fn make_tab_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::navigator::TabsHandle {
        use framework_core::primitives::navigator::{NavigatorHandle, TabsHandle};
        if let NodeKind::TabNavigator { control, .. } = &node.borrow().kind {
            return TabsHandle::from_inner(NavigatorHandle::with_control(
                Rc::new(()),
                &WgpuNavigatorOps,
                control.clone(),
            ));
        }
        TabsHandle::from_inner(NavigatorHandle::new(Rc::new(()), &WgpuNavigatorOps))
    }

    fn make_drawer_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::navigator::DrawerHandle {
        use framework_core::primitives::navigator::{DrawerHandle, NavigatorHandle};
        if let NodeKind::DrawerNavigator { control, is_open, .. } = &node.borrow().kind {
            return DrawerHandle::from_inner(
                NavigatorHandle::with_control(
                    Rc::new(()),
                    &WgpuNavigatorOps,
                    control.clone(),
                ),
                is_open.clone(),
            );
        }
        DrawerHandle::from_inner(
            NavigatorHandle::new(Rc::new(()), &WgpuNavigatorOps),
            Rc::new(std::cell::Cell::new(false)),
        )
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        let parent_layout = navigator.borrow().layout;
        let child_layout = screen.borrow().layout;
        self.layout.add_child(parent_layout, child_layout);
        navigator.borrow_mut().children.push(screen.clone());
        if let NodeKind::Navigator { scope_ids, .. } = &navigator.borrow().kind {
            scope_ids.borrow_mut().push(scope_id);
        }
        self.roots.retain(|n| !Rc::ptr_eq(n, &screen));
        attach_screen_metadata(self, &screen, navigator, options);
        mark_as_navigator_screen(&mut self.layout, &screen);
        request_redraw();
    }

    fn create_tab_navigator(
        &mut self,
        callbacks: framework_core::primitives::navigator::TabNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        // Seed `routes[0]` with the initial route's name and a
        // placeholder scope_id. The framework calls
        // `tab_navigator_attach_initial` immediately after this
        // returns; that path patches the scope_id in-place. Storing
        // the name up-front means the dispatcher can match
        // `Select { name }` against routes without us having to
        // thread the initial-route name through the attach call.
        let initial_route = callbacks.navigator.initial_route;
        let node = new_node(
            NodeKind::TabNavigator {
                active_tab: std::cell::Cell::new(0),
                tab_count: std::cell::Cell::new(0),
                routes: std::cell::RefCell::new(vec![crate::node::TabRoute {
                    name: initial_route,
                    scope_id: 0,
                }]),
                control: control.clone(),
                bar_style: std::cell::RefCell::new(None),
                icon_style: std::cell::RefCell::new(None),
                label_style: std::cell::RefCell::new(None),
            },
            layout,
        );
        install_tab_dispatcher(
            &node,
            callbacks,
            control,
            self.self_weak.get().expect("self_weak set in Host::new").clone(),
        );
        self.roots.push(node.clone());
        node
    }

    fn tab_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        let parent_layout = navigator.borrow().layout;
        let child_layout = screen.borrow().layout;
        self.layout.add_child(parent_layout, child_layout);
        navigator.borrow_mut().children.push(screen.clone());
        if let NodeKind::TabNavigator { tab_count, routes, .. } = &navigator.borrow().kind
        {
            tab_count.set(tab_count.get() + 1);
            // Patch the placeholder scope_id we seeded in
            // `create_tab_navigator`. The dispatcher's `Select`
            // path needs an accurate scope_id so a future
            // mount-policy that disposes the previously-active tab
            // can call `release_screen` with the right id.
            if let Some(first) = routes.borrow_mut().first_mut() {
                first.scope_id = scope_id;
            }
        }
        self.roots.retain(|n| !Rc::ptr_eq(n, &screen));
        attach_screen_metadata(self, &screen, navigator, options);
        mark_as_navigator_screen(&mut self.layout, &screen);
        request_redraw();
    }

    fn create_drawer_navigator(
        &mut self,
        callbacks: framework_core::primitives::navigator::DrawerNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::primitives::navigator::NavigatorControl>,
    ) -> Self::Node {
        let layout = self.layout.new_node();
        let initial_route = callbacks.navigator.initial_route;
        let node = new_node(
            NodeKind::DrawerNavigator {
                is_open: Rc::new(std::cell::Cell::new(false)),
                active_screen: std::cell::Cell::new(0),
                routes: std::cell::RefCell::new(vec![crate::node::TabRoute {
                    name: initial_route,
                    scope_id: 0,
                }]),
                sidebar: std::cell::RefCell::new(None),
                control: control.clone(),
                anim_started_at: std::cell::Cell::new(None),
                scrim_style: std::cell::RefCell::new(None),
                sidebar_style: std::cell::RefCell::new(None),
            },
            layout,
        );
        install_drawer_dispatcher(
            &node,
            callbacks,
            control,
            self.self_weak.get().expect("self_weak set in Host::new").clone(),
        );
        self.roots.push(node.clone());
        node
    }

    fn drawer_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        let parent_layout = navigator.borrow().layout;
        let child_layout = screen.borrow().layout;
        self.layout.add_child(parent_layout, child_layout);
        navigator.borrow_mut().children.push(screen.clone());
        if let NodeKind::DrawerNavigator { routes, .. } = &navigator.borrow().kind {
            // Same scope-id patch as tabs — see
            // `tab_navigator_attach_initial`.
            if let Some(first) = routes.borrow_mut().first_mut() {
                first.scope_id = scope_id;
            }
        }
        self.roots.retain(|n| !Rc::ptr_eq(n, &screen));
        attach_screen_metadata(self, &screen, navigator, options);
        mark_as_navigator_screen(&mut self.layout, &screen);
        request_redraw();
    }

    fn drawer_navigator_attach_sidebar(
        &mut self,
        navigator: &Self::Node,
        sidebar: Self::Node,
    ) {
        // Attach the sidebar as a Taffy child so its layout
        // gets computed alongside the body, but pin it to
        // absolute positioning + the drawer width so it stacks
        // above the body. The renderer's drawer-aware walk
        // filters it out of the in-flow paint and lifts it to
        // the top-z overlay pass with the slide transform.
        let parent_layout = navigator.borrow().layout;
        let sidebar_layout = sidebar.borrow().layout;
        self.layout.add_child(parent_layout, sidebar_layout);
        let absolute_sidebar = StyleRules {
            position: Some(framework_core::Position::Absolute),
            top: Some(Tokenized::Literal(framework_core::Length::Px(0.0))),
            bottom: Some(Tokenized::Literal(framework_core::Length::Px(0.0))),
            left: Some(Tokenized::Literal(framework_core::Length::Px(0.0))),
            ..Default::default()
        };
        self.layout.set_style(sidebar_layout, &absolute_sidebar);
        self.roots.retain(|n| !Rc::ptr_eq(n, &sidebar));
        // Append at the *end* of the children list so the
        // drawer's stored "child index 0 = body, last = sidebar"
        // convention holds.
        navigator.borrow_mut().children.push(sidebar.clone());
        if let NodeKind::DrawerNavigator { sidebar: slot, .. } = &navigator.borrow().kind {
            *slot.borrow_mut() = Some(sidebar);
        }
        request_redraw();
    }

    // -----------------------------------------------------------
    // Unsupported primitives — render a "not supported" panel.
    // -----------------------------------------------------------

    #[cfg(blitz_active)]
    fn create_web_view(&mut self, url: &str) -> Self::Node {
        // Default render size — the author will almost always
        // override via `.with_style(...)` width/height. We size
        // the Blitz output to logical-px-equivalent at 1.0 scale;
        // a HiDPI follow-up would thread the device scale here.
        let layout = self.layout.new_node();
        self.layout.set_intrinsic_size(layout, 320.0, 480.0);
        let view = std::rc::Rc::new(crate::web_view::WebView::spawn(
            url.to_string(),
            320,
            480,
        ));
        let node = new_node(
            NodeKind::WebView {
                view,
                last_uploaded_paint: std::cell::Cell::new(0),
            },
            layout,
        );
        self.roots.push(node.clone());
        node
    }

    // Web target: same node shape, but the `WebView` struct is the
    // tiny URL-holder stub from `web_view_wasm.rs`. The actual
    // iframe is mounted by the host shell through the renderer's
    // `DomOverlay` hook — no GPU upload here.
    #[cfg(target_arch = "wasm32")]
    fn create_web_view(&mut self, url: &str) -> Self::Node {
        let layout = self.layout.new_node();
        self.layout.set_intrinsic_size(layout, 320.0, 480.0);
        let view = std::rc::Rc::new(crate::web_view::WebView::spawn(
            url.to_string(),
            320,
            480,
        ));
        let node = new_node(NodeKind::WebView { view }, layout);
        self.roots.push(node.clone());
        node
    }

    #[cfg(not(webview_node))]
    fn create_web_view(&mut self, _url: &str) -> Self::Node {
        make_unsupported(&mut self.layout, &mut self.roots, "WebView")
    }

    #[cfg(webview_node)]
    fn make_web_view_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::web_view::WebViewHandle {
        framework_core::primitives::web_view::WebViewHandle::new(
            Rc::new(node.clone()) as Rc<dyn std::any::Any>,
            &WgpuWebViewOps,
        )
    }

    fn create_video(
        &mut self,
        src: &str,
        autoplay: bool,
        controls: bool,
        loop_playback: bool,
    ) -> Self::Node {
        let decoder = std::rc::Rc::new(crate::video::VideoDecoder::spawn(
            src.to_string(),
            autoplay,
            loop_playback,
        ));
        let layout = self.layout.new_node();
        // Default intrinsic size — the author almost always sets
        // width/height via stylesheet, but a non-zero default
        // keeps unsized Video visible until they do.
        self.layout.set_intrinsic_size(layout, 320.0, 180.0);
        // Start with the controls visible at mount so it's
        // obvious they're available; the hover-fade then takes
        // over from there. Equivalent to a synthetic "the user
        // just landed on the video" hover at creation time.
        let initial_hover = if controls {
            Some(web_time::Instant::now())
        } else {
            None
        };
        let node = new_node(
            NodeKind::Video {
                decoder,
                controls,
                last_hover: std::cell::Cell::new(initial_hover),
                play_btn_rect: std::cell::Cell::new((0.0, 0.0, 0.0, 0.0)),
                scrubber_rect: std::cell::Cell::new((0.0, 0.0, 0.0, 0.0)),
                mute_btn_rect: std::cell::Cell::new((0.0, 0.0, 0.0, 0.0)),
                frame_rect: std::cell::Cell::new((0.0, 0.0, 0.0, 0.0)),
            },
            layout,
        );
        self.roots.push(node.clone());
        node
    }

    fn make_video_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::video::VideoHandle {
        // Wrap the `WgpuNode` itself as the handle's userdata so
        // `WgpuVideoOps` can downcast back to reach the decoder.
        // Same shape as `make_graphics_handle`.
        framework_core::primitives::video::VideoHandle::new(
            Rc::new(node.clone()) as Rc<dyn std::any::Any>,
            &WgpuVideoOps,
        )
    }

    fn make_graphics_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::graphics::GraphicsHandle {
        // Wrap the `WgpuNode` itself as the handle's userdata so
        // `register_graphics_drawer` can downcast back to recover
        // it. `WgpuNode = Rc<RefCell<NodeData>>`; the `Rc<dyn Any>`
        // GraphicsHandle holds therefore points at a fresh Rc whose
        // inner concrete type is `WgpuNode` (i.e.
        // `Rc<RefCell<NodeData>>`). Downcast target on retrieval
        // is the same `WgpuNode` type alias.
        framework_core::primitives::graphics::GraphicsHandle::new(
            Rc::new(node.clone()) as Rc<dyn std::any::Any>,
            &WgpuGraphicsOps,
        )
    }

    fn create_graphics(
        &mut self,
        _on_ready: framework_core::primitives::graphics::OnReady,
        _on_resize: framework_core::primitives::graphics::OnResize,
        _on_lost: framework_core::primitives::graphics::OnLost,
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
        self.roots.push(node.clone());
        node
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
                position: Some(framework_core::Position::Absolute),
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

    fn release_navigator(&mut self, node: &Self::Node) {
        // Drop every mounted screen's Taffy state + animator
        // tweens + text store entries. The per-screen framework
        // Scopes are owned by the dispatcher closure on the
        // NavigatorControl, which drops alongside the user-
        // facing scope — so we don't need to invoke release
        // callbacks here. The backend's job is to clean the
        // Taffy + render state.
        self.clear_children(node);
    }

    fn release_tab_navigator(&mut self, node: &Self::Node) {
        self.clear_children(node);
    }

    fn release_drawer_navigator(&mut self, node: &Self::Node) {
        // Drawer's sidebar is also in `children`, so
        // clear_children handles it. The framework's drawer
        // open-state signal lives on the dispatcher closure and
        // drops along with the navigator's enclosing scope.
        self.clear_children(node);
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
    ) -> Option<framework_core::primitives::portal::ViewportRect> {
        // Local frame (relative to the parent's content box) —
        // straight out of Taffy's computed layout.
        let frame = self.layout.frame_of(node.borrow().layout);
        Some(framework_core::primitives::portal::ViewportRect {
            x: frame.x,
            y: frame.y,
            width: frame.width,
            height: frame.height,
        })
    }

    fn absolute_frame(
        &self,
        node: &Self::Node,
    ) -> Option<framework_core::primitives::portal::ViewportRect> {
        // Walk down from each root accumulating origins until we
        // hit `node`. `absolute_origin` already does this for the
        // host's pointer dispatch; the rect's size is just the
        // Taffy frame at the node.
        let origin = crate::host::absolute_origin(self, node);
        let size = self.layout.frame_of(node.borrow().layout);
        Some(framework_core::primitives::portal::ViewportRect {
            x: origin.0,
            y: origin.1,
            width: size.width,
            height: size.height,
        })
    }

    fn apply_safe_area_padding(
        &mut self,
        node: &Self::Node,
        sides: framework_core::SafeAreaSides,
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
        sides: framework_core::SafeAreaSides,
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

    fn apply_navigator_header_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<StyleRules>,
    ) {
        if let NodeKind::Navigator { header_style, .. } = &navigator.borrow().kind {
            *header_style.borrow_mut() = Some(style.clone());
            request_redraw();
        }
    }

    fn apply_navigator_title_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<StyleRules>,
    ) {
        if let NodeKind::Navigator { title_style, .. } = &navigator.borrow().kind {
            *title_style.borrow_mut() = Some(style.clone());
            request_redraw();
        }
    }

    fn apply_navigator_button_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<StyleRules>,
    ) {
        if let NodeKind::Navigator { button_style, .. } = &navigator.borrow().kind {
            *button_style.borrow_mut() = Some(style.clone());
            request_redraw();
        }
    }

    fn apply_navigator_body_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<StyleRules>,
    ) {
        if let NodeKind::Navigator { body_style, .. } = &navigator.borrow().kind {
            *body_style.borrow_mut() = Some(style.clone());
            request_redraw();
        }
    }

    fn apply_drawer_sidebar_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<StyleRules>,
    ) {
        if let NodeKind::DrawerNavigator { sidebar_style, .. } = &navigator.borrow().kind {
            *sidebar_style.borrow_mut() = Some(style.clone());
            request_redraw();
        }
    }

    fn apply_drawer_scrim_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<StyleRules>,
    ) {
        if let NodeKind::DrawerNavigator { scrim_style, .. } = &navigator.borrow().kind {
            *scrim_style.borrow_mut() = Some(style.clone());
            request_redraw();
        }
    }

    fn apply_tab_bar_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<StyleRules>,
    ) {
        if let NodeKind::TabNavigator { bar_style, .. } = &navigator.borrow().kind {
            *bar_style.borrow_mut() = Some(style.clone());
            request_redraw();
        }
    }

    fn apply_tab_icon_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<StyleRules>,
    ) {
        if let NodeKind::TabNavigator { icon_style, .. } = &navigator.borrow().kind {
            *icon_style.borrow_mut() = Some(style.clone());
            request_redraw();
        }
    }

    fn apply_tab_label_style(
        &mut self,
        navigator: &Self::Node,
        style: &Rc<StyleRules>,
    ) {
        if let NodeKind::TabNavigator { label_style, .. } = &navigator.borrow().kind {
            *label_style.borrow_mut() = Some(style.clone());
            request_redraw();
        }
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
        // `Skin` trait). Author rules win on any field they set;
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

        // Navigator screens must stay absolute + full-bleed
        // regardless of the author's own style on the screen
        // root. Re-stamp the fill rules after each apply so a
        // theme swap (or any other reactive re-style) doesn't
        // revert the screen to `position: Relative` and collapse
        // it down to its natural size. The top inset depends on
        // whether this particular screen has its header shown,
        // so re-derive it from the per-screen options each time.
        if node.borrow().navigator_screen {
            let inset = screen_top_inset(node);
            self.layout
                .set_style(layout, &navigator_screen_fill_rules(inset));
        }

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

        if is_text {
            let mut text = self.text.borrow_mut();
            let mut fs = self.font_system.borrow_mut();
            text.set_font_size(&mut fs, layout, font_size);
            drop(text);
            drop(fs);
            self.layout.mark_dirty(layout);
        }
        request_redraw();
    }
}

/// `NavigatorOps` impl for wgpu. All callbacks are default no-ops
/// — the renderer doesn't need to do anything special on push /
/// pop notifications (the dispatcher already re-attached / detached
/// the child, and `request_redraw` was already pinged). Held as a
/// unit struct so `NavigatorHandle::with_control` has a stable
/// `&'static dyn NavigatorOps` to reference.
struct WgpuNavigatorOps;
impl framework_core::primitives::navigator::NavigatorOps for WgpuNavigatorOps {}

/// `GraphicsOps` impl for wgpu. Same shape as `WgpuNavigatorOps`
/// — a unit struct that lets `make_graphics_handle` hand a
/// `&'static dyn GraphicsOps` reference back through the
/// framework's `GraphicsHandle`. No imperative ops today; future
/// host→author commands (resize hints, capture-frame) would
/// land here.
struct WgpuGraphicsOps;
impl framework_core::primitives::graphics::GraphicsOps for WgpuGraphicsOps {}

/// `VideoOps` impl for the wgpu preview. Routes `play` / `pause`
/// / `seek` to the per-node `VideoDecoder` by downcasting the
/// `VideoHandle.node` Rc back to our `WgpuNode` (which carries
/// the decoder's `Arc` to its shared playback state).
struct WgpuVideoOps;
impl framework_core::primitives::video::VideoOps for WgpuVideoOps {
    fn play(&self, node: &dyn std::any::Any) {
        if let Some(n) = node.downcast_ref::<WgpuNode>() {
            if let NodeKind::Video { decoder, .. } = &n.borrow().kind {
                decoder.set_playing(true);
                request_redraw();
            }
        }
    }
    fn pause(&self, node: &dyn std::any::Any) {
        if let Some(n) = node.downcast_ref::<WgpuNode>() {
            if let NodeKind::Video { decoder, .. } = &n.borrow().kind {
                decoder.set_playing(false);
            }
        }
    }
    fn seek(&self, node: &dyn std::any::Any, seconds: f32) {
        if let Some(n) = node.downcast_ref::<WgpuNode>() {
            if let NodeKind::Video { decoder, .. } = &n.borrow().kind {
                decoder.seek(seconds as f64);
                request_redraw();
            }
        }
    }
}

/// `WebViewOps` impl for the wgpu preview, backed by Blitz on
/// native or an `<iframe>` on the web. Only `reload` is wired in
/// Phase 1 — `post_message` / `execute_js` require JS execution,
/// which Blitz doesn't ship yet, so we leave them as the no-op
/// default. `reload` re-triggers a fetch via the worker's
/// navigate hook on native; on wasm it forces an iframe `src`
/// re-set (handled host-side).
#[cfg(webview_node)]
struct WgpuWebViewOps;

#[cfg(webview_node)]
impl framework_core::primitives::web_view::WebViewOps for WgpuWebViewOps {
    fn reload(&self, node: &dyn std::any::Any) {
        if let Some(n) = node.downcast_ref::<WgpuNode>() {
            // We don't carry the original URL on the node — the
            // worker keeps that — so "reload" is signaled via a
            // navigate request set to the empty string. The
            // worker treats `Some("")` as "re-load the current
            // URL"; for now this is a no-op until we extend the
            // protocol. Keeping the hook here so authors can
            // wire up state-tracking without a follow-up
            // breaking change.
            if let NodeKind::WebView { view, .. } = &n.borrow().kind {
                // Empty navigate request is currently inert; the
                // hook is here so future "reload" logic only
                // needs a worker-side change.
                let _ = view;
            }
        }
    }
}

/// Install a per-frame draw closure on a `GraphicsHandle`'s
/// node. The handle must be obtained from
/// `framework_core::primitives::graphics::graphics(...).bind(ref)`
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
    handle: &framework_core::primitives::graphics::GraphicsHandle,
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
) -> framework_core::Bound<framework_core::primitives::graphics::GraphicsHandle>
where
    D: FnMut(&mut crate::node::GraphicsFrame) + 'static,
{
    let mut prim = framework_core::primitives::graphics::graphics(|_| {});
    // Re-encode the drawer as a `RefFill::Graphics` closure: the
    // framework fires that closure during mount with the
    // backend-built `GraphicsHandle`. We hand it straight to
    // `register_graphics_drawer` so the per-frame pre-pass picks
    // it up starting from the next render. Bypasses `.bind(r)` —
    // the author doesn't need a `Ref` for this case.
    let drawer_box: crate::node::GraphicsDrawer = Box::new(drawer);
    if let framework_core::Primitive::Graphics { ref_fill, .. } = prim.primitive_mut() {
        *ref_fill = Some(framework_core::RefFill::Graphics(Box::new(
            move |h: framework_core::primitives::graphics::GraphicsHandle| {
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
    transition: Option<&framework_core::Transition>,
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
    node: &WgpuNode,
) {
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in &children {
        drop_subtree(layout, text, animator, spinner_count, child);
    }
    let id = node.borrow().layout;
    if matches!(node.borrow().kind, NodeKind::ActivityIndicator { .. }) {
        *spinner_count = spinner_count.saturating_sub(1);
    }
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

/// Build an `Unsupported` placeholder node — used by primitives
/// that the simulator chooses not to implement (WebView, Video,
/// Graphics). The placeholder gets an intrinsic height so it's
/// visible even with no explicit sizing from the author.
fn make_unsupported(
    layout: &mut LayoutTree,
    roots: &mut Vec<WgpuNode>,
    label: &'static str,
) -> WgpuNode {
    let id = layout.new_node();
    layout.set_intrinsic_size(id, -1.0, UNSUPPORTED_DEFAULT_HEIGHT);
    let node = new_node(NodeKind::Unsupported { label }, id);
    roots.push(node.clone());
    node
}

/// Install the framework's command dispatcher for a stack
/// `Navigator`. Captures a weak handle to the backend so the
/// dispatcher closure can re-borrow the layout tree from user
/// code (e.g. `handle.push(...)`) without conflicting with the
/// framework's own borrow window — those user calls always
/// happen after `create_navigator` has returned.
///
/// The closure is `Fn` (per [`NavigatorControl::install`]); all
/// state mutation goes through interior `RefCell`s — the
/// backend's, the node's, and the per-kind tracking cells.
fn install_navigator_dispatcher(
    nav_node: &WgpuNode,
    callbacks: framework_core::primitives::navigator::NavigatorCallbacks<WgpuNode>,
    control: Rc<framework_core::primitives::navigator::NavigatorControl>,
    backend_weak: Weak<RefCell<WgpuBackend>>,
) {
    use framework_core::primitives::navigator::NavCommand;
    let nav_weak = Rc::downgrade(nav_node);
    // Clone out the Rc-shared callback closures so each command
    // can call them across multiple dispatcher invocations
    // (NavigatorCallbacks itself is not Clone).
    let mount = callbacks.mount_screen.clone();
    let release = callbacks.release_screen.clone();
    let depth_changed = callbacks.depth_changed.clone();
    control.install(Box::new(move |cmd| {
        match cmd {
            NavCommand::Push { name, params, .. } => {
                // Mount FIRST, without holding any backend borrow.
                // The build walker inside `mount` calls back into
                // `backend.borrow_mut()` per-create; holding a
                // borrow across this would deadlock.
                let result = mount(name, params);
                let Some(backend) = backend_weak.upgrade() else { return };
                let Some(nav) = nav_weak.upgrade() else { return };
                let new_depth = attach_navigator_child(
                    &backend, &nav, &result.node, result.scope_id, result.options,
                );
                // Start the push slide. The renderer will pick up
                // the transition on the next frame and translate
                // the new top child from the right edge inward.
                start_nav_transition(&nav, crate::node::NavTransitionKind::Push);
                depth_changed(new_depth);
                crate::scheduler::request_redraw();
            }
            NavCommand::Pop => {
                let Some(nav) = nav_weak.upgrade() else { return };
                // Snapshot top scope_id without unmounting — the
                // popping subtree has to stay on-screen for the
                // duration of the slide. `tick_nav_transitions`
                // does the actual unmount when the animation
                // completes.
                let Some(top_scope) = peek_top_navigator_scope_id(&nav) else { return };
                let new_depth = nav.borrow().children.len().saturating_sub(1);
                start_nav_transition(
                    &nav,
                    crate::node::NavTransitionKind::Pop {
                        popping_scope_id: top_scope,
                        release_screen: release.clone(),
                    },
                );
                // Notify the framework the pop happened — semantically
                // it has, even though the visual transition is still
                // running. Layout chrome that reads `handle.depth()`
                // updates immediately.
                depth_changed(new_depth);
                crate::scheduler::request_redraw();
            }
            NavCommand::Replace { name, params, .. } => {
                let result = mount(name, params);
                let Some(backend) = backend_weak.upgrade() else { return };
                let Some(nav) = nav_weak.upgrade() else { return };
                let popped = detach_top_navigator_child(&backend, &nav);
                let new_depth = attach_navigator_child(
                    &backend, &nav, &result.node, result.scope_id, result.options,
                );
                if let Some((old_node, old_scope, _)) = popped {
                    // Release the framework scope FIRST so its
                    // `StyleHandle`s unregister their cohort entries
                    // before we free the Taffy slots they referenced.
                    // See `detach_top_navigator_child` doc comment.
                    release(old_scope);
                    let mut guard = backend.borrow_mut();
                    let b: &mut WgpuBackend = &mut guard;
                    drop_subtree(
                        &mut b.layout,
                        &b.text,
                        &mut b.animator,
                        &mut b.active_spinner_count,
                        &old_node,
                    );
                }
                depth_changed(new_depth);
                crate::scheduler::request_redraw();
            }
            NavCommand::Reset { name, params, .. } => {
                let result = mount(name, params);
                let Some(backend) = backend_weak.upgrade() else { return };
                let Some(nav) = nav_weak.upgrade() else { return };
                let detached = clear_navigator_children(&backend, &nav);
                let new_depth = attach_navigator_child(
                    &backend, &nav, &result.node, result.scope_id, result.options,
                );
                // Release each scope BEFORE dropping its subtree —
                // same ordering rationale as in `Replace`. Doing it
                // in two passes (all releases, then all drops) keeps
                // the cohort consistent for the duration of the
                // teardown, in case any release_screen triggers a
                // cohort-iterating effect.
                for (_, scope_id) in &detached {
                    release(*scope_id);
                }
                let mut guard = backend.borrow_mut();
                let b: &mut WgpuBackend = &mut guard;
                for (node, _) in detached {
                    drop_subtree(
                        &mut b.layout,
                        &b.text,
                        &mut b.animator,
                        &mut b.active_spinner_count,
                        &node,
                    );
                }
                depth_changed(new_depth);
                crate::scheduler::request_redraw();
            }
            // Tab / drawer commands are a programmer error against
            // a stack navigator. The per-kind handles enforce this
            // statically, so reaching this arm means something
            // dispatched directly via NavigatorControl — log and
            // drop rather than panic so a misconfigured layout
            // doesn't crash the whole app.
            NavCommand::Select { .. }
            | NavCommand::OpenDrawer
            | NavCommand::CloseDrawer
            | NavCommand::ToggleDrawer => {}
        }
    }));
}

/// Install the dispatcher for a `TabNavigator`. `Select` either
/// flips `active_tab` (route already mounted) or mounts the
/// requested route on-demand and appends it to the tab list.
fn install_tab_dispatcher(
    nav_node: &WgpuNode,
    callbacks: framework_core::primitives::navigator::TabNavigatorCallbacks<WgpuNode>,
    control: Rc<framework_core::primitives::navigator::NavigatorControl>,
    backend_weak: Weak<RefCell<WgpuBackend>>,
) {
    use framework_core::primitives::navigator::NavCommand;
    let nav_weak = Rc::downgrade(nav_node);
    let mount = callbacks.navigator.mount_screen.clone();
    let active_changed = callbacks.active_changed.clone();
    control.install(Box::new(move |cmd| {
        if let NavCommand::Select { name, params, .. } = cmd {
            let Some(nav) = nav_weak.upgrade() else { return };
            if let Some(idx) = find_route_index(&nav, name) {
                set_tab_active(&nav, idx);
                active_changed(name);
                crate::scheduler::request_redraw();
                return;
            }
            // Lazy mount: tab not yet present, build it and append.
            let result = mount(name, params);
            let Some(backend) = backend_weak.upgrade() else { return };
            let new_idx = attach_tab_or_drawer_child(
                &backend, &nav, &result.node, name, result.scope_id, result.options,
            );
            set_tab_active(&nav, new_idx);
            active_changed(name);
            crate::scheduler::request_redraw();
        }
    }));
}

/// Install the dispatcher for a `DrawerNavigator`. Open/close
/// commands flip the node's `is_open` cell; `Select` swaps the
/// active body screen via the same name→index lookup as tabs.
fn install_drawer_dispatcher(
    nav_node: &WgpuNode,
    callbacks: framework_core::primitives::navigator::DrawerNavigatorCallbacks<WgpuNode>,
    control: Rc<framework_core::primitives::navigator::NavigatorControl>,
    backend_weak: Weak<RefCell<WgpuBackend>>,
) {
    use framework_core::primitives::navigator::NavCommand;
    let nav_weak = Rc::downgrade(nav_node);
    let mount = callbacks.navigator.mount_screen.clone();
    let active_changed = callbacks.active_changed.clone();
    let open_changed = callbacks.open_changed.clone();
    let is_open_signal = callbacks.is_open;
    control.install(Box::new(move |cmd| {
        let Some(nav) = nav_weak.upgrade() else { return };
        match cmd {
            NavCommand::OpenDrawer => {
                if !drawer_is_open(&nav) {
                    start_drawer_anim(&nav);
                }
                set_drawer_open(&nav, true);
                is_open_signal.set(true);
                open_changed(true);
                crate::scheduler::request_redraw();
            }
            NavCommand::CloseDrawer => {
                if drawer_is_open(&nav) {
                    start_drawer_anim(&nav);
                }
                set_drawer_open(&nav, false);
                is_open_signal.set(false);
                open_changed(false);
                crate::scheduler::request_redraw();
            }
            NavCommand::ToggleDrawer => {
                let next = !drawer_is_open(&nav);
                start_drawer_anim(&nav);
                set_drawer_open(&nav, next);
                is_open_signal.set(next);
                open_changed(next);
                crate::scheduler::request_redraw();
            }
            NavCommand::Select { name, params, .. } => {
                if let Some(idx) = find_route_index(&nav, name) {
                    set_drawer_active(&nav, idx);
                } else {
                    let result = mount(name, params);
                    let Some(backend) = backend_weak.upgrade() else { return };
                    let new_idx = attach_tab_or_drawer_child(
                        &backend, &nav, &result.node, name, result.scope_id, result.options,
                    );
                    set_drawer_active(&nav, new_idx);
                }
                // Selecting a drawer item conventionally collapses
                // the panel — matches React Navigation's drawer
                // default ("tap an item, drawer closes").
                set_drawer_open(&nav, false);
                is_open_signal.set(false);
                open_changed(false);
                active_changed(name);
                crate::scheduler::request_redraw();
            }
            _ => {}
        }
    }));
}

// ---------------------------------------------------------------
// Navigator dispatch helpers
//
// Each helper takes `&Rc<RefCell<WgpuBackend>>` and runs its
// borrow_mut → mutate → drop in a single unit, so callers don't
// have to manage interleaved borrows. Calling them between a
// `mount_screen` invocation and node-graph patching is sound
// because the build walker only holds `backend.borrow_mut()`
// per individual create call.
// ---------------------------------------------------------------

/// Insert a freshly-mounted screen as a child of a stack
/// `Navigator`. Returns the new stack depth.
fn attach_navigator_child(
    backend: &Rc<RefCell<WgpuBackend>>,
    nav: &WgpuNode,
    screen: &WgpuNode,
    scope_id: u64,
    options: framework_core::primitives::navigator::ScreenOptions,
) -> usize {
    let nav_layout = nav.borrow().layout;
    let screen_layout = screen.borrow().layout;
    {
        let mut b = backend.borrow_mut();
        b.layout.add_child(nav_layout, screen_layout);
        b.roots.retain(|n| !Rc::ptr_eq(n, screen));
        attach_screen_metadata(&mut *b, screen, nav, options);
        mark_as_navigator_screen(&mut b.layout, screen);
    }
    nav.borrow_mut().children.push(screen.clone());
    if let NodeKind::Navigator { scope_ids, .. } = &nav.borrow().kind {
        scope_ids.borrow_mut().push(scope_id);
        scope_ids.borrow().len()
    } else {
        0
    }
}

/// Pop the top screen off a stack `Navigator`. Returns
/// `Some((popped_node, scope_id, new_depth))` or `None` if only
/// the root screen remains — bottoming out a stack is a no-op
/// (matches iOS `UINavigationController`).
///
/// The popped node has been detached from the Taffy parent but
/// **its subtree has not been dropped yet**. The caller must
/// first invoke the framework's `release_screen(scope_id)` (so
/// the screen's `Scope` drops, unregistering its theme-cohort
/// entries and any per-node reactive style effects), and only
/// then call [`drop_subtree`] on the returned node. Reversing
/// that order leaves cohort entries pointing at freed Taffy
/// slots — the next `set_theme` would panic with "invalid
/// SlotMap key used".
#[must_use = "caller must release(scope_id) then drop_subtree(&node) — in that order"]
fn detach_top_navigator_child(
    backend: &Rc<RefCell<WgpuBackend>>,
    nav: &WgpuNode,
) -> Option<(WgpuNode, u64, usize)> {
    if nav.borrow().children.len() <= 1 {
        return None;
    }
    let top = nav.borrow_mut().children.pop()?;
    let scope_id = if let NodeKind::Navigator { scope_ids, .. } = &nav.borrow().kind {
        scope_ids.borrow_mut().pop().unwrap_or(0)
    } else {
        0
    };
    let nav_layout = nav.borrow().layout;
    let top_layout = top.borrow().layout;
    backend
        .borrow_mut()
        .layout
        .remove_child(nav_layout, top_layout);
    Some((top, scope_id, nav.borrow().children.len()))
}

/// Detach every screen from a stack `Navigator`. Returns each
/// detached subtree paired with its `scope_id`, in mount order.
///
/// As with [`detach_top_navigator_child`], the children have
/// been removed from the Taffy parent but **not** dropped. The
/// caller must walk the returned vec calling `release(scope_id)`
/// first, then `drop_subtree(&node)` per item — see that function's
/// note about cohort-entry / Taffy-slot ordering.
#[must_use = "caller must release(scope_id) then drop_subtree(&node) per item — in that order"]
fn clear_navigator_children(
    backend: &Rc<RefCell<WgpuBackend>>,
    nav: &WgpuNode,
) -> Vec<(WgpuNode, u64)> {
    let children: Vec<WgpuNode> = nav.borrow_mut().children.drain(..).collect();
    let scope_ids: Vec<u64> = if let NodeKind::Navigator { scope_ids, .. } = &nav.borrow().kind {
        scope_ids.borrow_mut().drain(..).collect()
    } else {
        Vec::new()
    };
    let nav_layout = nav.borrow().layout;
    {
        let mut guard = backend.borrow_mut();
        let b: &mut WgpuBackend = &mut guard;
        for child in &children {
            let child_layout = child.borrow().layout;
            b.layout.remove_child(nav_layout, child_layout);
        }
    }
    children.into_iter().zip(scope_ids).collect()
}

/// Append a freshly-mounted screen to a tab or drawer
/// navigator. Returns the new tab/screen index.
fn attach_tab_or_drawer_child(
    backend: &Rc<RefCell<WgpuBackend>>,
    nav: &WgpuNode,
    screen: &WgpuNode,
    name: &'static str,
    scope_id: u64,
    options: framework_core::primitives::navigator::ScreenOptions,
) -> usize {
    let nav_layout = nav.borrow().layout;
    let screen_layout = screen.borrow().layout;
    {
        let mut b = backend.borrow_mut();
        b.layout.add_child(nav_layout, screen_layout);
        b.roots.retain(|n| !Rc::ptr_eq(n, screen));
        attach_screen_metadata(&mut *b, screen, nav, options);
        mark_as_navigator_screen(&mut b.layout, screen);
    }

    // Drawer navigators keep the sidebar as the *last* entry in
    // `children` (see `drawer_navigator_attach_sidebar`). Splice
    // body screens in *before* the sidebar so the renderer's
    // "filter out sidebar" walk doesn't accidentally treat the
    // new screen as sidebar chrome.
    let (kind_is_drawer, sidebar_present) = match &nav.borrow().kind {
        NodeKind::DrawerNavigator { sidebar, .. } => (true, sidebar.borrow().is_some()),
        _ => (false, false),
    };
    if kind_is_drawer && sidebar_present {
        let insert_at = nav.borrow().children.len().saturating_sub(1);
        nav.borrow_mut().children.insert(insert_at, screen.clone());
    } else {
        nav.borrow_mut().children.push(screen.clone());
    }

    match &nav.borrow().kind {
        NodeKind::TabNavigator { tab_count, routes, .. } => {
            tab_count.set(tab_count.get() + 1);
            let mut r = routes.borrow_mut();
            r.push(crate::node::TabRoute { name, scope_id });
            r.len() - 1
        }
        NodeKind::DrawerNavigator { routes, .. } => {
            let mut r = routes.borrow_mut();
            r.push(crate::node::TabRoute { name, scope_id });
            r.len() - 1
        }
        _ => 0,
    }
}

fn find_route_index(nav: &WgpuNode, name: &'static str) -> Option<usize> {
    match &nav.borrow().kind {
        NodeKind::TabNavigator { routes, .. } | NodeKind::DrawerNavigator { routes, .. } => {
            routes.borrow().iter().position(|r| r.name == name)
        }
        _ => None,
    }
}

fn set_tab_active(nav: &WgpuNode, idx: usize) {
    if let NodeKind::TabNavigator { active_tab, .. } = &nav.borrow().kind {
        active_tab.set(idx);
    }
}

fn set_drawer_active(nav: &WgpuNode, idx: usize) {
    if let NodeKind::DrawerNavigator { active_screen, .. } = &nav.borrow().kind {
        active_screen.set(idx);
    }
}

fn set_drawer_open(nav: &WgpuNode, open: bool) {
    if let NodeKind::DrawerNavigator { is_open, .. } = &nav.borrow().kind {
        is_open.set(open);
    }
}

/// Tag `screen` as the root of a navigator-mounted screen and
/// stamp its Taffy style with absolute positioning + insets so
/// the screen always fills the navigator's rect (minus the
/// header strip when one's visible). Called once at attach
/// time; the sticky behavior across later `apply_style`
/// re-applies is enforced inside `Backend::apply_style` itself
/// by re-stamping whenever the `navigator_screen` flag is set.
fn mark_as_navigator_screen(layout: &mut LayoutTree, screen: &WgpuNode) {
    screen.borrow_mut().navigator_screen = true;
    let id = screen.borrow().layout;
    let top_inset = screen_top_inset(screen);
    layout.set_style(id, &navigator_screen_fill_rules(top_inset));
}

/// Record per-screen metadata that the renderer + host need
/// (header options, owning-navigator handle, pre-shaped title
/// buffer). Done before `mark_as_navigator_screen` so the
/// latter can read `screen_options.header_shown` to pick the
/// right top inset.
///
/// `b` is the backend; we need it for the shared `text` /
/// `font_system` so the title's glyph buffer can be allocated +
/// shaped at attach time (one-time cost; the renderer fetches
/// the pre-shaped buffer each frame without reshaping).
///
/// Apps that nest a stack navigator inside a `DrawerNavigator`
/// can drive the drawer from a header button by passing a
/// hamburger [`HeaderButton`] in `ScreenOptions::header_left`
/// whose `on_press` calls the drawer's
/// `NavigatorHandle::toggle_drawer` — see the docs for the
/// drawer navigator. The simulator doesn't auto-inject the
/// hamburger because that would require an upwards-tree walk
/// at every attach (WgpuNodes have no parent pointer).
fn attach_screen_metadata(
    b: &mut WgpuBackend,
    screen: &WgpuNode,
    navigator: &WgpuNode,
    options: framework_core::primitives::navigator::ScreenOptions,
) {

    // Allocate + shape the title buffer first (while we still
    // own the options struct), then move options onto the node.
    let title_layout = options.title.as_ref().map(|title| {
        let id = b.layout.new_node();
        let mut text = b.text.borrow_mut();
        let mut fs = b.font_system.borrow_mut();
        // 17pt is the iOS inline-title default; Material runs a
        // tick larger (~18-20pt) but the skin can pick its own
        // visual sizing from this shared buffer at paint time
        // (centering still works regardless of skin-specific
        // tweaks).
        text.create(&mut fs, id, title, 17.0);
        id
    });
    let mut data = screen.borrow_mut();
    data.screen_title_layout = title_layout;
    data.screen_options = Some(Box::new(options));
    data.owning_navigator = Some(Rc::downgrade(navigator));
}

/// How far below the navigator's top edge a screen's content
/// should start, in logical px. `safe_area.top + NAV_HEADER_HEIGHT`
/// when the screen wants a header; 0 when it opts out (the
/// screen is responsible for using `.safe_area(...)` itself
/// in that case). Drives both the Taffy inset and the
/// renderer's header-strip rect.
///
/// Known limitation: nested navigators that both show headers
/// will leave a `safe_area.top`-tall empty strip between the
/// outer and inner header — the inner nav's screen also
/// reserves safe-area-top, but it's stacked inside the outer
/// screen which already did so. Real apps rarely show two
/// navigator headers stacked; a future pass can detect the
/// nested case and skip the inner inset.
fn screen_top_inset(screen: &WgpuNode) -> f32 {
    let header_shown = screen
        .borrow()
        .screen_options
        .as_ref()
        .and_then(|o| o.header_shown)
        .unwrap_or(true);
    if header_shown {
        let safe_top = framework_core::safe_area_insets().get().top;
        safe_top + crate::node::NAV_HEADER_HEIGHT
    } else {
        0.0
    }
}

/// Style rules that make a navigator screen full-bleed: absolute
/// position with insets that lift the content below the header
/// strip when one is shown. The author's own style on the
/// screen's outer view is merged on top by Taffy (`set_style` is
/// field-by-field), so flex_direction / padding / gap /
/// background from the user's sheet are preserved while we pin
/// position + insets.
fn navigator_screen_fill_rules(top_inset: f32) -> StyleRules {
    use framework_core::{Length, Position};
    StyleRules {
        position: Some(Position::Absolute),
        top: Some(Tokenized::Literal(Length::Px(top_inset))),
        right: Some(Tokenized::Literal(Length::Px(0.0))),
        bottom: Some(Tokenized::Literal(Length::Px(0.0))),
        left: Some(Tokenized::Literal(Length::Px(0.0))),
        ..Default::default()
    }
}

/// Style rules for a Navigator's own Taffy container — width
/// and height pinned to 100% so the navigator fills its parent
/// (which is the viewport when the navigator is the framework's
/// root). Without this, a Navigator wrapped in another View
/// would collapse to its content's intrinsic size (zero,
/// because every screen child is `position: Absolute` and
/// contributes nothing to flex sizing).
fn navigator_container_fill_rules() -> StyleRules {
    use framework_core::Length;
    StyleRules {
        width: Some(Tokenized::Literal(Length::Percent(100.0))),
        height: Some(Tokenized::Literal(Length::Percent(100.0))),
        ..Default::default()
    }
}

/// Mark the start of a drawer open/close slide. The renderer
/// samples this against the wall clock each frame to compute
/// the sidebar's slide-in / slide-out offset; the host's `tick`
/// keeps redrawing while the animation runs by reading
/// `drawer_anim_alive`.
fn start_drawer_anim(nav: &WgpuNode) {
    if let NodeKind::DrawerNavigator { anim_started_at, .. } = &nav.borrow().kind {
        anim_started_at.set(Some(web_time::Instant::now()));
    }
}

/// Stamp safe-area insets as padding onto a node. Called from
/// `apply_safe_area_padding` (View) and
/// `apply_scroll_view_safe_area_inset` (ScrollView); the wgpu
/// sim uses the same padding-mutation path for both since the
/// renderer paints scrollbars against the scrollview's outer
/// frame regardless of inner padding.
///
/// `as_padding` is kept as a parameter so a future split can
/// give scroll views a different model (e.g. content insets
/// via a wrapper node) without disturbing the View path's
/// signature.
fn apply_safe_area_to_node(
    layout: &mut LayoutTree,
    node: &WgpuNode,
    sides: framework_core::SafeAreaSides,
    _as_padding: bool,
) {
    use framework_core::{Length, SafeAreaSides};
    let insets = framework_core::safe_area_insets().get();
    // Read the author's most-recently-applied padding so the
    // safe-area inset *adds* on top instead of clobbering. The
    // framework's spec is "combine with author padding, don't
    // clobber it" — matching iOS's `contentInset` semantics.
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
    // `set_style` always writes `position` from the rules
    // (None → Relative). For navigator screens that were marked
    // absolute-and-full-bleed, that flips them back to Relative
    // and breaks the layout. Re-stamp the navigator-screen fill
    // rules right after so the screen stays absolute and inset
    // for its header.
    if node.borrow().navigator_screen {
        let inset = screen_top_inset(node);
        layout.set_style(id, &navigator_screen_fill_rules(inset));
    }
    layout.mark_dirty(id);
}

fn drawer_is_open(nav: &WgpuNode) -> bool {
    if let NodeKind::DrawerNavigator { is_open, .. } = &nav.borrow().kind {
        is_open.get()
    } else {
        false
    }
}

/// Read the scope_id of the top screen on a stack navigator
/// without unmounting it. Used by the Pop dispatcher so the
/// popping subtree can stay on-screen during the slide while
/// the framework's depth tracking advances immediately.
fn peek_top_navigator_scope_id(nav: &WgpuNode) -> Option<u64> {
    // Refuse to "pop" the root — matches iOS
    // UINavigationController behavior. The framework's per-kind
    // handle already guards against this, but a direct
    // NavigatorControl::dispatch from layout chrome could still
    // reach this path.
    if nav.borrow().children.len() <= 1 {
        return None;
    }
    if let NodeKind::Navigator { scope_ids, .. } = &nav.borrow().kind {
        scope_ids.borrow().last().copied()
    } else {
        None
    }
}

fn start_nav_transition(nav: &WgpuNode, kind: crate::node::NavTransitionKind) {
    if let NodeKind::Navigator { transition, .. } = &nav.borrow().kind {
        *transition.borrow_mut() = Some(crate::node::NavTransition {
            kind,
            start: web_time::Instant::now(),
        });
    }
}

/// Walk the tree and advance any in-flight `NavTransition`.
/// Push transitions just clear on completion; Pop transitions
/// run the deferred unmount + `release_screen` then clear.
/// Returns true while at least one transition is still in
/// flight (the host's tick uses this to keep redrawing).
pub(crate) fn tick_nav_transitions(
    backend: &Rc<RefCell<WgpuBackend>>,
    now: web_time::Instant,
) -> bool {
    // Collect navigator nodes up front so we don't recurse with
    // the backend borrowed. Mutations against the layout tree
    // happen below outside the read-phase borrow.
    let mut navs: Vec<WgpuNode> = Vec::new();
    {
        let b = backend.borrow();
        if let Some(root) = b.root() {
            collect_navigators(&root, &mut navs);
        }
    }
    let mut any_in_flight = false;
    for nav in navs {
        // Inspect the transition; if not yet complete, mark
        // alive and move on. Otherwise extract the kind so we
        // can release the borrow before mutating the tree.
        // Duration comes from the navigator's animator —
        // different `ScreenTransition` impls (slide, modal,
        // instant) decide their own length.
        let elapsed_ratio = match &nav.borrow().kind {
            NodeKind::Navigator { transition, transition_anim, .. } => {
                let duration_ms = transition_anim.duration_ms().max(1) as f32;
                transition.borrow().as_ref().map(|t| {
                    now.saturating_duration_since(t.start).as_millis() as f32 / duration_ms
                })
            }
            _ => None,
        };
        let Some(ratio) = elapsed_ratio else { continue };
        if ratio < 1.0 {
            any_in_flight = true;
            continue;
        }
        // Animation done — pull the transition out so we can act
        // on its kind without holding any borrow on the node.
        let taken = if let NodeKind::Navigator { transition, .. } = &nav.borrow().kind {
            transition.borrow_mut().take()
        } else {
            None
        };
        let Some(t) = taken else { continue };
        match t.kind {
            crate::node::NavTransitionKind::Push => {
                // Nothing else to do — the new screen is already
                // the navigator's top child and at its resting
                // position once the transition is cleared.
            }
            crate::node::NavTransitionKind::Pop { popping_scope_id, release_screen } => {
                // The popping subtree is still the last child;
                // run the unmount we deferred at dispatch time.
                if let Some((popped_node, scope_id, _new_depth)) =
                    detach_top_navigator_child(backend, &nav)
                {
                    // Defensive: prefer the captured scope_id —
                    // detach returns the same value modulo a
                    // mid-animation Replace/Reset reshuffling
                    // the stack (which is rare but possible if
                    // user code dispatches commands during the
                    // slide). Either way, fire release once.
                    let id = if scope_id != 0 { scope_id } else { popping_scope_id };
                    // Release the framework scope before freeing
                    // the Taffy slots — keeps theme-cohort entries
                    // from outliving their backing layout nodes.
                    release_screen(id);
                    let mut guard = backend.borrow_mut();
                    let b: &mut WgpuBackend = &mut guard;
                    drop_subtree(
                        &mut b.layout,
                        &b.text,
                        &mut b.animator,
                        &mut b.active_spinner_count,
                        &popped_node,
                    );
                }
            }
        }
        crate::scheduler::request_redraw();
    }
    any_in_flight
}

/// Depth-first walk that collects every `Navigator` node under
/// `root`. Used by `tick_nav_transitions` to find what to
/// advance without holding a backend borrow.
fn collect_navigators(node: &WgpuNode, out: &mut Vec<WgpuNode>) {
    if matches!(&node.borrow().kind, NodeKind::Navigator { .. }) {
        out.push(node.clone());
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in children {
        collect_navigators(&child, out);
    }
}

/// Return `true` while any `Video` node has a running, playing
/// decoder. The host's tick loops `request_redraw` while this
/// holds so the renderer's pre-pass keeps uploading fresh
/// decoded frames as they arrive. Cheap walk — Video nodes are
/// rare; the alternative (cross-thread `request_redraw` from
/// the decoder thread) would need a wgpu-side event-loop proxy
/// we don't currently expose.
pub(crate) fn any_video_playing(backend: &Rc<RefCell<WgpuBackend>>) -> bool {
    let b = backend.borrow();
    let Some(root) = b.root() else { return false };
    walk_for_playing_video(&root)
}

/// Walk every Video node and call `decoder.shutdown()` on each.
/// Called from the platform shell on window-close to proactively
/// silence audio and stop decoder threads without waiting for
/// the `Rc<VideoDecoder>` to drop — Rc cycles or long-lived
/// reactive scopes can keep the decoder alive long past the
/// window's lifetime, which is why dropping audio reactively
/// (via the decoder's `Drop` impl alone) isn't sufficient.
pub(crate) fn shutdown_all_videos(backend: &Rc<RefCell<WgpuBackend>>) {
    let b = backend.borrow();
    let Some(root) = b.root() else {
        eprintln!("[shutdown] no root, nothing to tear down");
        return;
    };
    let mut count = 0;
    walk_shutdown_videos(&root, &mut count);
    eprintln!("[shutdown] tore down {count} video decoder(s)");
}

fn walk_shutdown_videos(node: &WgpuNode, count: &mut usize) {
    if let NodeKind::Video { decoder, .. } = &node.borrow().kind {
        decoder.shutdown();
        *count += 1;
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in children {
        walk_shutdown_videos(&child, count);
    }
}

/// Walk every WebView node and call `WebView::shutdown()`. Same
/// rationale as `shutdown_all_videos`: drop the worker thread
/// proactively rather than waiting for `Rc` unwinding (which
/// reactive scopes can delay past `event_loop.exit()`).
#[cfg(blitz_active)]
pub(crate) fn shutdown_all_web_views(backend: &Rc<RefCell<WgpuBackend>>) {
    let b = backend.borrow();
    let Some(root) = b.root() else {
        return;
    };
    let mut count = 0;
    walk_shutdown_web_views(&root, &mut count);
    if count > 0 {
        eprintln!("[shutdown] tore down {count} web view(s)");
    }
}

#[cfg(blitz_active)]
fn walk_shutdown_web_views(node: &WgpuNode, count: &mut usize) {
    if let NodeKind::WebView { view, .. } = &node.borrow().kind {
        view.shutdown();
        *count += 1;
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in children {
        walk_shutdown_web_views(&child, count);
    }
}

fn walk_for_playing_video(node: &WgpuNode) -> bool {
    if let NodeKind::Video { decoder, .. } = &node.borrow().kind {
        if decoder.shared.playing.load(std::sync::atomic::Ordering::Acquire) {
            return true;
        }
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in children {
        if walk_for_playing_video(&child) {
            return true;
        }
    }
    false
}

/// Update each `Video` node's hover state against a pointer
/// position in world space. Called from `pointer_move`. Returns
/// `true` if any Video's hover state changed (so the host can
/// trigger a redraw to play the fade-in animation).
pub(crate) fn update_video_hover(
    backend: &Rc<RefCell<WgpuBackend>>,
    point: (f32, f32),
) -> bool {
    let b = backend.borrow();
    let Some(root) = b.root() else { return false };
    let mut changed = false;
    let now = web_time::Instant::now();
    walk_update_hover(&root, point, now, &mut changed);
    changed
}

fn walk_update_hover(node: &WgpuNode, point: (f32, f32), now: web_time::Instant, changed: &mut bool) {
    if let NodeKind::Video { controls, last_hover, frame_rect, .. } = &node.borrow().kind {
        if *controls {
            let (rx, ry, rw, rh) = frame_rect.get();
            if rw > 0.0 && rh > 0.0 {
                let inside = point.0 >= rx
                    && point.0 <= rx + rw
                    && point.1 >= ry
                    && point.1 <= ry + rh;
                if inside {
                    let prev = last_hover.get();
                    last_hover.set(Some(now));
                    // Trigger redraw if we just entered (was None
                    // or stale). Mid-hover updates don't need a
                    // redraw — paint already runs every frame
                    // while the fade is alive.
                    if prev.is_none() {
                        *changed = true;
                    }
                }
            }
        }
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in children {
        walk_update_hover(&child, point, now, changed);
    }
}

/// Outcome of resolving a pointer-down against the video
/// controls. The host translates this into either an immediate
/// state change (toggle play) or a `VideoScrub` drag capture.
pub(crate) enum VideoControlPress {
    /// No control was hit.
    Miss,
    /// Play/pause icon — already toggled; nothing for host to do.
    Toggled,
    /// Scrubber pressed; host should capture the gesture as a
    /// drag. The seek for the initial press position has already
    /// been issued.
    ScrubStart {
        node: WgpuNode,
        prior_muted: bool,
    },
}

pub(crate) fn dispatch_video_control_press(
    backend: &Rc<RefCell<WgpuBackend>>,
    point: (f32, f32),
) -> VideoControlPress {
    let b = backend.borrow();
    let Some(root) = b.root() else { return VideoControlPress::Miss };
    walk_dispatch_press(&root, point)
}

fn walk_dispatch_press(node: &WgpuNode, point: (f32, f32)) -> VideoControlPress {
    let local = {
        let data = node.borrow();
        if let NodeKind::Video {
            decoder,
            controls,
            play_btn_rect,
            scrubber_rect,
            mute_btn_rect,
            ..
        } = &data.kind
        {
            if !*controls {
                None
            } else if hit(play_btn_rect.get(), point) {
                let was_playing = decoder.shared.playing.load(std::sync::atomic::Ordering::Acquire);
                decoder.set_playing(!was_playing);
                Some(VideoControlPress::Toggled)
            } else if hit(mute_btn_rect.get(), point) {
                let was_muted = decoder.is_audio_muted().unwrap_or(false);
                decoder.set_muted(!was_muted);
                Some(VideoControlPress::Toggled)
            } else if hit(scrubber_rect.get(), point) {
                // Seek immediately to the press location so the
                // first frame the drag shows matches the cursor.
                scrub_to(decoder, scrubber_rect.get(), point);
                // Mute audio for the drag so the user doesn't
                // hear chopped-up audio across rapid re-seeks.
                // We restore the prior state on release.
                let prior_muted = audio_muted_state(decoder);
                decoder.set_muted(true);
                Some(VideoControlPress::ScrubStart {
                    node: node.clone(),
                    prior_muted,
                })
            } else {
                None
            }
        } else {
            None
        }
    };
    if let Some(result) = local {
        return result;
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in children {
        match walk_dispatch_press(&child, point) {
            VideoControlPress::Miss => continue,
            hit => return hit,
        }
    }
    VideoControlPress::Miss
}

/// Apply a scrub-position update for the given pointer over the
/// given video. Public so the host's `pointer_move` can call it
/// while an active `VideoScrub` drag is in flight.
pub(crate) fn scrub_video(node: &WgpuNode, point: (f32, f32)) {
    if let NodeKind::Video { decoder, scrubber_rect, .. } = &node.borrow().kind {
        scrub_to(decoder, scrubber_rect.get(), point);
    }
}

/// Finalize a scrub drag — restore the audio-muted state. Run
/// from `pointer_up` for the `VideoScrub` press variant.
pub(crate) fn end_scrub(node: &WgpuNode, prior_muted: bool) {
    if let NodeKind::Video { decoder, .. } = &node.borrow().kind {
        decoder.set_muted(prior_muted);
    }
}

fn scrub_to(
    decoder: &std::rc::Rc<crate::video::VideoDecoder>,
    scrubber_rect: (f32, f32, f32, f32),
    point: (f32, f32),
) {
    let (sx, _, sw, _) = scrubber_rect;
    if sw <= 0.0 {
        return;
    }
    let progress = ((point.0 - sx) / sw).clamp(0.0, 1.0);
    let dur_us = decoder
        .shared
        .duration_micros
        .load(std::sync::atomic::Ordering::Acquire);
    if dur_us == 0 {
        return;
    }
    let target = (dur_us as f64 * progress as f64) / 1_000_000.0;
    decoder.seek(target);
}

/// Read the audio handle's current muted flag. Returns `false`
/// when there's no audio handle (silent video).
fn audio_muted_state(decoder: &std::rc::Rc<crate::video::VideoDecoder>) -> bool {
    decoder.is_audio_muted().unwrap_or(false)
}

fn hit(rect: (f32, f32, f32, f32), p: (f32, f32)) -> bool {
    let (x, y, w, h) = rect;
    w > 0.0 && h > 0.0 && p.0 >= x && p.0 <= x + w && p.1 >= y && p.1 <= y + h
}

/// Return `true` while any Video's controls are visible (in the
/// 2 s post-hover window OR while the decoder is paused). Hosts
/// the redraw loop so the fade-out animation completes smoothly.
pub(crate) fn any_video_controls_alive(backend: &Rc<RefCell<WgpuBackend>>) -> bool {
    const VISIBLE_BUDGET_SECS: f32 = 2.3;
    let b = backend.borrow();
    let Some(root) = b.root() else { return false };
    walk_controls_alive(&root, VISIBLE_BUDGET_SECS)
}

fn walk_controls_alive(node: &WgpuNode, budget_secs: f32) -> bool {
    if let NodeKind::Video { controls, last_hover, decoder, .. } = &node.borrow().kind {
        if *controls {
            // Paused → always alive (controls stay shown).
            if !decoder.shared.playing.load(std::sync::atomic::Ordering::Acquire) {
                return true;
            }
            if let Some(t) = last_hover.get() {
                if t.elapsed().as_secs_f32() < budget_secs {
                    return true;
                }
            }
        }
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in children {
        if walk_controls_alive(&child, budget_secs) {
            return true;
        }
    }
    false
}

/// Return `true` while any `DrawerNavigator` in the tree has
/// an in-flight slide animation. The renderer clears the
/// per-node `anim_started_at` once the slide settles; until
/// then this signal keeps the host's tick redrawing.
pub(crate) fn drawer_anim_alive(backend: &Rc<RefCell<WgpuBackend>>) -> bool {
    let b = backend.borrow();
    let Some(root) = b.root() else { return false };
    let mut drawers: Vec<WgpuNode> = Vec::new();
    collect_drawers(&root, &mut drawers);
    let now = web_time::Instant::now();
    for nav in drawers {
        let alive = match &nav.borrow().kind {
            NodeKind::DrawerNavigator { anim_started_at, .. } => {
                if let Some(start) = anim_started_at.get() {
                    let elapsed = now.saturating_duration_since(start).as_millis() as u32;
                    elapsed < crate::node::DRAWER_ANIM_MS
                } else {
                    false
                }
            }
            _ => false,
        };
        if alive {
            return true;
        }
    }
    false
}

fn collect_drawers(node: &WgpuNode, out: &mut Vec<WgpuNode>) {
    if matches!(&node.borrow().kind, NodeKind::DrawerNavigator { .. }) {
        out.push(node.clone());
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in children {
        collect_drawers(&child, out);
    }
}
