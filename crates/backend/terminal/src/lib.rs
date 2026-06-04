//! ASCII / terminal backend for `runtime_core::Backend`.
//!
//! Renders the framework's primitive tree into a character grid. The
//! companion `host-terminal` crate paints the grid to stdout (ANSI
//! escapes for color) and forwards mouse / keyboard events back into
//! the backend's hit-tester so `Pressable` / `Button` `on_click`
//! callbacks fire.
//!
//! Layout is delegated to `runtime-layout` (Taffy), same as iOS /
//! Android / macOS — flex containers, gap, padding, width/height,
//! `position: absolute` all work. The unit on this backend is
//! **terminal cell** (1 col x 1 row), not pixel — author stylesheets
//! that say `width: 40` get 40 columns wide.

mod handles;
mod node;
mod render;

pub use node::{NodeKind, TermNode};
pub use render::{Cell, Grid};

/// Outcome of [`TerminalBackend::dispatch_click`]. The host pattern-
/// matches this and is responsible for invoking `HandlerFired`'s
/// closure *after* it releases its `&mut self` borrow on the
/// backend — otherwise the closure's `Signal::set` → effect →
/// `update_text` chain re-enters and panics with "RefCell already
/// borrowed".
pub enum ClickOutcome {
    /// Click landed on a clickable node. The handler is returned so
    /// the host can fire it once it's released its backend borrow.
    HandlerFired(Rc<dyn Fn()>),
    /// Click landed on a TextInput; focus is now set on it.
    FocusedInput,
    /// Click landed somewhere with no handler / no input. Focus
    /// (if any) has been cleared.
    Unhandled,
}

impl std::fmt::Debug for ClickOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClickOutcome::HandlerFired(_) => f.write_str("HandlerFired"),
            ClickOutcome::FocusedInput => f.write_str("FocusedInput"),
            ClickOutcome::Unhandled => f.write_str("Unhandled"),
        }
    }
}

use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::animation::AnimProp;
use runtime_core::color::{parse_or, Rgba};
use runtime_core::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_core::{Action, Backend, Color as FwColor, ColorScheme, Platform, StyleRules};
use runtime_layout::{AvailableSpace, LayoutNode, LayoutTree, Size as TaffySize};

use node::NodeData;

/// ASCII backend. One per host. Owns the parallel layout tree and
/// every node's data (kind, content, style, click handler).
pub struct TerminalBackend {
    pub(crate) layout: LayoutTree,
    /// Per-node storage. `TermNode { id }` is the public handle;
    /// every backend op looks up data here.
    pub(crate) nodes: HashMap<u32, NodeData>,
    /// Monotonically increasing node id allocator.
    pub(crate) next_id: u32,
    /// Reverse map: layout-tree node → backend node id. Used by the
    /// reverse-lookup helpers, and during compute to mark every
    /// node's frame.
    pub(crate) layout_to_id: HashMap<LayoutNode, u32>,
    /// Last-known terminal viewport size in cells. The host updates
    /// this whenever it observes a resize.
    pub(crate) viewport: (u16, u16),
    /// Node id of the currently focused input, if any. Set by
    /// [`dispatch_click`] on a TextInput hit and cleared on
    /// clicks elsewhere or on `Escape` / `Tab` / `Enter`.
    pub(crate) focused_id: Option<u32>,
    /// Conversion factor between layout px (what Taffy + author
    /// stylesheets speak) and terminal cells (what we paint to).
    /// Default `(1.0, 1.0)` — author px values land in cells 1:1
    /// (works for terminal-native UIs like `hello-terminal`).
    /// For layouts targeting mobile/desktop pixel densities (the
    /// `welcome` example uses ~390pt-wide mobile viewports), the
    /// host sets it via [`set_cell_size`] to something like
    /// `(8.0, 16.0)` so `width: px(14)` lands at a sane ~2 cells
    /// instead of overflowing the viewport.
    pub(crate) cell_size: (f32, f32),
    /// Registry of `Element::Navigator` handler factories,
    /// populated by SDK leaf crates calling `register_navigator::<P, _>(...)`
    /// during app bootstrap. `create_navigator` looks the factory up
    /// by presentation TypeId.
    pub(crate) navigator_handlers:
        runtime_core::NavigatorRegistry<TerminalBackend>,
    /// Per-navigator-instance SDK handler. Keyed by the outlet/root
    /// node id the handler returned from `init`. Subsequent navigator
    /// dispatch routes through the handler stored here.
    pub(crate) nav_handler_instances: HashMap<
        u32,
        Rc<std::cell::RefCell<Box<dyn runtime_core::NavigatorHandler<TerminalBackend>>>>,
    >,
}

impl Default for TerminalBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// An inventory-collected navigator registrar. A navigator SDK's terminal
/// module `inventory::submit!`s one (carrying a `fn(&mut TerminalBackend)`);
/// [`TerminalBackend::new`] drains them so the app needn't call
/// `<nav>::register` per platform. See [[project_inventory_self_registration]].
pub struct TerminalNavigatorRegistrar(pub fn(&mut TerminalBackend));
inventory::collect!(TerminalNavigatorRegistrar);

impl TerminalBackend {
    /// Install every SDK-submitted navigator handler. Native (non-wasm) so
    /// inventory's link-time ctors populate the slice before construction.
    fn drain_self_registrars(&mut self) {
        for r in inventory::iter::<TerminalNavigatorRegistrar> {
            (r.0)(self);
        }
    }

    pub fn new() -> Self {
        let mut backend = Self {
            layout: LayoutTree::new(),
            nodes: HashMap::new(),
            next_id: 1,
            layout_to_id: HashMap::new(),
            viewport: (80, 24),
            focused_id: None,
            cell_size: (1.0, 1.0),
            navigator_handlers: runtime_core::NavigatorRegistry::new(),
            nav_handler_instances: HashMap::new(),
        };
        backend.drain_self_registrars();
        backend
    }

    /// Register a `NavigatorHandler` factory for the SDK-defined
    /// presentation type `P`. SDK leaf crates call this once per app
    /// bootstrap; on subsequent `Element::Navigator { type_id =
    /// TypeId::of::<P>(), .. }` builds the framework invokes the
    /// factory to produce a fresh handler.
    pub fn register_navigator<P, F>(&mut self, factory: F)
    where
        P: 'static,
        F: Fn() -> Box<dyn runtime_core::NavigatorHandler<TerminalBackend>> + 'static,
    {
        self.navigator_handlers.register::<P, _>(factory);
    }

    /// Read the laid-out frame `(x, y, w, h)` of `node` in layout-px
    /// space. Helper for ad-hoc logging from SDK code that needs to
    /// inspect the resolved layout. Returns `(0, 0, 0, 0)` for an
    /// unknown node.
    pub fn frame_of_for_log(&self, node: TermNode) -> (f32, f32, f32, f32) {
        let Some(layout) = self.nodes.get(&node.id).map(|d| d.layout) else {
            return (0.0, 0.0, 0.0, 0.0);
        };
        let f = self.layout.frame_of(layout);
        (f.x, f.y, f.width, f.height)
    }

    /// Run Taffy's layout compute against the current root + viewport
    /// without actually painting. SDK code uses this to force the
    /// frame cache up-to-date before reading it for diagnostics —
    /// the normal cache update happens inside `render_to_grid`'s
    /// compute call, which runs AFTER the dispatcher's post-swap
    /// hook.
    pub fn compute_layout_for_log(&mut self) {
        let Some(root_id) = self.find_root() else { return };
        let Some(root_layout) = self.nodes.get(&root_id).map(|d| d.layout) else { return };
        let (cols, rows) = self.viewport;
        let (cw, ch) = self.cell_size;
        self.layout.compute(root_layout, cols as f32 * cw, rows as f32 * ch);
    }

    /// Return the direct child ids of `node`. Used by SDK code that
    /// needs to verify parent/child wiring after a mutation (e.g.
    /// the drawer-navigator's post-swap sidebar-integrity check).
    pub fn children_of_for_log(&self, node: TermNode) -> Vec<u32> {
        self.nodes.get(&node.id).map(|d| d.children.clone()).unwrap_or_default()
    }

    /// Read the static + animated opacity slots of `node`. Returns
    /// `(static, animated)` where `static` is the styled opacity (or
    /// 1.0 default) and `animated` is the in-flight animation override
    /// (or `None` when no animation is driving it). Used by SDK code
    /// to diagnose "paint skipped" symptoms — paint_node short-
    /// circuits when `effective_opacity <= 0.0`, so a stray animation
    /// pinning opacity to 0 is one of the few render-side
    /// invisibility causes.
    pub fn opacity_of_for_log(&self, node: TermNode) -> (f32, Option<f32>) {
        let Some(d) = self.nodes.get(&node.id) else {
            return (0.0, None);
        };
        (d.opacity, d.animated_opacity)
    }

    /// Dump a node + every descendant's frame + kind + content into a
    /// multi-line string. Used by SDK code to inspect what a subtree
    /// actually looks like after a layout pass.
    pub fn dump_subtree_for_log(&self, node: TermNode) -> String {
        let mut out = String::new();
        self.dump_subtree_into(node.id, 0, &mut out);
        out
    }

    fn dump_subtree_into(&self, id: u32, depth: usize, out: &mut String) {
        let Some(d) = self.nodes.get(&id) else {
            return;
        };
        let f = self.layout.frame_of(d.layout);
        let indent = "  ".repeat(depth);
        // Truncate by CHARS, not bytes — the dump fires on every
        // post-swap and the docs example has multi-byte text (box-
        // drawing glyphs in the Simulator section's ASCII art). A
        // byte slice can split inside a `─` (3 bytes) and panic
        // with "not a char boundary".
        let content = if d.content.is_empty() {
            String::new()
        } else {
            let preview: String = d.content.chars().take(40).collect();
            format!(" content={preview:?}")
        };
        let bg = match (d.bg, d.animated_bg) {
            (_, Some(ab)) => format!(" bg-anim=#{:02x}{:02x}{:02x}{:02x}", ab.r, ab.g, ab.b, ab.a),
            (Some(b), None) => format!(" bg=#{:02x}{:02x}{:02x}{:02x}", b.r, b.g, b.b, b.a),
            (None, None) => String::new(),
        };
        let op = if d.opacity != 1.0 || d.animated_opacity.is_some() {
            format!(" op={:.2}/anim={:?}", d.opacity, d.animated_opacity)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "{indent}#{id} {:?} frame=({:.0},{:.0},{:.0},{:.0}){bg}{op}{content} children={}\n",
            d.kind,
            f.x,
            f.y,
            f.width,
            f.height,
            d.children.len(),
        ));
        for &cid in &d.children {
            self.dump_subtree_into(cid, depth + 1, out);
        }
    }

    /// Fully remove `node` and every descendant from the backend —
    /// drops the per-node data AND tears down the corresponding
    /// Taffy nodes. Use this when a navigator screen is released
    /// and won't be reattached. Without this, the screen's root
    /// stays in the Taffy tree as a parentless root, and
    /// [`find_root`] may pick it instead of the actual application
    /// root on subsequent compute / paint passes — which surfaced
    /// as "page content bleeds into sidebar" on the docs example
    /// (the old screen's tree got painted at viewport origin,
    /// overlaying everything including the sidebar).
    pub fn destroy_subtree(&mut self, node: TermNode) {
        if let Some(d) = self.nodes.remove(&node.id) {
            self.layout.remove_node(d.layout);
            self.layout_to_id.remove(&d.layout);
            self.drop_subtree(&d.children);
        }
    }

    /// Non-destructively remove `child` from `parent`. Unlike
    /// [`Backend::clear_children`] (which drops every removed child
    /// and its subtree), this just severs the Taffy edge — the
    /// child's NodeData stays in `self.nodes`, so it can be
    /// re-inserted later. Used by the stack navigator to detach
    /// the current screen on Push and re-attach it on Pop without
    /// re-mounting.
    pub fn detach_child(&mut self, parent: &TermNode, child: &TermNode) {
        let parent_layout = match self.nodes.get(&parent.id) {
            Some(d) => d.layout,
            None => return,
        };
        let child_layout = match self.nodes.get(&child.id) {
            Some(d) => d.layout,
            None => return,
        };
        self.layout.remove_child(parent_layout, child_layout);
        self.layout.mark_dirty(parent_layout);
        if let Some(p) = self.nodes.get_mut(&parent.id) {
            p.children.retain(|c| *c != child.id);
        }
    }

    /// Configure the layout-px-per-cell factor. Default is
    /// `(1.0, 1.0)`. Call BEFORE mounting if the author tree's
    /// stylesheet uses px values calibrated for a higher-DPI
    /// viewport (mobile / desktop). The backend tells Taffy that
    /// the viewport is `(cols * w, rows * h)` layout-px and
    /// divides every rendered frame by this factor on the way out
    /// to cells.
    pub fn set_cell_size(&mut self, w: f32, h: f32) {
        self.cell_size = (w.max(0.001), h.max(0.001));
    }

    /// Update the viewport size in cells. Next `render_to_grid` call
    /// uses the new size for the layout pass.
    pub fn set_viewport(&mut self, cols: u16, rows: u16) {
        self.viewport = (cols, rows);
        // Mirror into the framework's reactive viewport signal. The
        // terminal backend's "logical px" is one cell, so we push raw
        // cell counts. Cell-aware author code can multiply by
        // `cell_size` if it needs sub-cell math; the breakpoint use
        // case ("is the terminal wide enough for a sidebar?")
        // generally only cares about cell counts.
        runtime_core::set_viewport_size(runtime_core::ViewportSize {
            width: cols as f32,
            height: rows as f32,
        });
    }

    pub fn viewport(&self) -> (u16, u16) {
        self.viewport
    }

    /// Allocate a new node id and stash its data + layout-node mapping.
    fn alloc_node(&mut self, kind: NodeKind, content: String) -> TermNode {
        let id = self.next_id;
        self.next_id += 1;
        let layout = self.layout.new_node();
        self.layout_to_id.insert(layout, id);
        self.nodes.insert(
            id,
            NodeData {
                kind,
                content,
                on_click: None,
                style: None,
                layout,
                children: Vec::new(),
                fg: None,
                bg: None,
                gradient: None,
                opacity: 1.0,
                animated_opacity: None,
                translate_x: 0.0,
                translate_y: 0.0,
                animated_bg: None,
                animated_fg: None,
                static_translate_x: None,
                static_translate_y: None,
                toggle_value: false,
                anim_phase: 0.0,
                z_index: 0.0,
                input: None,
                horizontal: false,
                scroll_x: 0.0,
                scroll_y: 0.0,
                on_scroll: None,
            },
        );
        TermNode { id }
    }

    /// Walk the backend's nodes and find the root view (no parent
    /// edge in the layout tree). The application root is always the
    /// first node created (`id == 1`); any other parentless node is
    /// a transient orphan from a mid-flight tree mutation (e.g. a
    /// navigator screen that's been detached but not yet destroyed).
    /// Returns the LOWEST-id root so a stale orphan can't usurp the
    /// real root — `HashMap` iteration order is otherwise
    /// non-deterministic, which surfaced as the docs-on-terminal
    /// bug where alternating renders picked the orphan and painted
    /// it at viewport origin, overlaying the sidebar.
    pub(crate) fn find_root(&self) -> Option<u32> {
        let mut best: Option<u32> = None;
        for (id, data) in &self.nodes {
            if self.layout.is_root(data.layout) {
                best = Some(match best {
                    Some(b) => b.min(*id),
                    None => *id,
                });
            }
        }
        best
    }

    /// Dispatch a mouse-left click at terminal-cell coordinates
    /// `(col, row)`. Walks the laid-out tree deepest-first; the first
    /// hit (a) fires its `on_click` if it has one, OR (b) sets focus
    /// if it's a TextInput. A click that lands somewhere with no
    /// handler clears any active focus (the "click outside to blur"
    /// posture every desktop UI ships).
    ///
    /// Must be called after `render_to_grid` so the frame cache is
    /// populated.
    pub fn dispatch_click(&mut self, col: u16, row: u16) -> ClickOutcome {
        let Some(root) = self.find_root() else { return ClickOutcome::Unhandled };
        let mut hit: Option<HitTarget> = None;
        self.hit_test_walk(root, 0.0, 0.0, col as f32, row as f32, &mut hit);
        match hit {
            Some(HitTarget::Handler(h)) => {
                // Clicking a button blurs any focused input — same
                // posture as a browser focus-blur on outside-click.
                self.focused_id = None;
                // Hand the handler back to the host instead of
                // firing it here: the host holds an `&mut` borrow on
                // the backend across this call, and the handler's
                // `Signal::set` → reactive effect → `update_text`
                // chain would re-enter the same borrow and panic.
                ClickOutcome::HandlerFired(h)
            }
            Some(HitTarget::FocusInput(id)) => {
                self.focused_id = Some(id);
                // Place the cursor at the end of the value on click
                // — best terminal-app default for short text inputs.
                if let Some(d) = self.nodes.get_mut(&id) {
                    if let Some(input) = d.input.as_mut() {
                        input.cursor = input.value.chars().count();
                    }
                }
                ClickOutcome::FocusedInput
            }
            None => {
                self.focused_id = None;
                ClickOutcome::Unhandled
            }
        }
    }

    fn hit_test_walk(
        &self,
        id: u32,
        parent_x: f32,
        parent_y: f32,
        col: f32,
        row: f32,
        out: &mut Option<HitTarget>,
    ) {
        let Some(data) = self.nodes.get(&id) else { return };
        let frame = self.layout.frame_of(data.layout);
        let (cw, ch) = self.cell_size;
        // Static + animated translate compose the same way at hit-
        // test as at paint, otherwise click rects drift away from
        // what the user can see.
        let static_tx = data
            .static_translate_x
            .as_ref()
            .map(|l| render::resolve_length_against(l, frame.width))
            .unwrap_or(0.0);
        let static_ty = data
            .static_translate_y
            .as_ref()
            .map(|l| render::resolve_length_against(l, frame.height))
            .unwrap_or(0.0);
        // Convert frame from layout px to cell space (parent_x/y are
        // already in cells, click coords are in cells).
        let x = parent_x + (frame.x + data.translate_x + static_tx) / cw;
        let y = parent_y + (frame.y + data.translate_y + static_ty) / ch;
        let w = frame.width / cw;
        let h = frame.height / ch;
        // Snap to whole-cell coverage that EXACTLY mirrors the paint
        // pass: `paint_text` uses `y0 = y.floor()` and
        // `max_rows = h.ceil()`, so painted rows are
        // `[floor(y), floor(y) + ceil(h))`. The earlier snap used
        // `ceil(y + h)` for the bottom — equivalent only when `y` is
        // integer-aligned. For a Link at `y=2.25, h=2.0`, that
        // computed `ceil(4.25)=5` and the hit area extended ONE ROW
        // BELOW the visible content. Clicking the bottom-edge cell
        // of one Link then hit-test-matched the NEXT Link (because
        // its top row was inside the phantom row), dispatching the
        // wrong route — which surfaced as "the sidebar disappears
        // when I click the bottom of a NavLink" since the wrong
        // Link's mount path put the navigator into an unexpected
        // state. Reproducing the paint snap closes the gap.
        let cell_left = x.floor();
        let cell_right = x.floor() + w.ceil();
        let cell_top = y.floor();
        let cell_bottom = y.floor() + h.ceil();
        let inside = col >= cell_left
            && col < cell_right
            && row >= cell_top
            && row < cell_bottom;
        if !inside {
            return;
        }
        // Apply the same scroll-offset, clip, AND per-level floor
        // semantics the renderer uses. The renderer floors
        // `parent_x/y` at each recursion so cumulative fractional
        // drift gets snapped per level (see paint_node). Without
        // mirroring that here, a fractional `scroll_y` (which
        // happens whenever `content_h - viewport_h` doesn't land on
        // a cell boundary — e.g. the docs sidebar's max scroll is
        // 64.75) causes the cumulative hit-test y to round up to a
        // different integer than the paint's floored cascade. The
        // symptom was the toggle appearing at row N visually but
        // only firing on a click at row N+1.
        let (child_off_x, child_off_y) = if matches!(data.kind, NodeKind::ScrollView) {
            (data.scroll_x, data.scroll_y)
        } else {
            (0.0, 0.0)
        };
        let next_parent_x = (x - child_off_x).floor();
        let next_parent_y = (y - child_off_y).floor();
        // Children paint on top of the parent; visually-topmost wins
        // the hit. Walk siblings highest-z first so a planet-in-front
        // captures the click instead of a button behind it.
        let mut ordered = self.children_in_z_order(&data.children);
        ordered.reverse();
        for child in ordered {
            self.hit_test_walk(
                child,
                next_parent_x,
                next_parent_y,
                col,
                row,
                out,
            );
            if out.is_some() {
                return;
            }
        }
        if out.is_some() {
            return;
        }
        if let Some(handler) = &data.on_click {
            *out = Some(HitTarget::Handler(handler.clone()));
        } else if matches!(data.kind, NodeKind::TextInput) {
            *out = Some(HitTarget::FocusInput(id));
        }
    }

    /// Dispatch a mouse-wheel scroll at `(col, row)`. `delta_y` is in
    /// cells (positive = scroll content up = reveal content below);
    /// `delta_x` is the horizontal counterpart. Walks the rendered
    /// tree deepest-first and adjusts the innermost
    /// [`NodeKind::ScrollView`] whose frame contains the point,
    /// clamping its `scroll_x` / `scroll_y` against the laid-out
    /// content size.
    ///
    /// Returns `true` if a scroll view consumed the event. The host
    /// can use that to suppress fallback behavior (page-wide scroll
    /// gestures, etc.).
    ///
    /// Must be called after `render_to_grid` so the frame cache is
    /// populated — same precondition as [`Self::dispatch_click`].
    pub fn dispatch_scroll(
        &mut self,
        col: u16,
        row: u16,
        delta_x: f32,
        delta_y: f32,
    ) -> bool {
        let Some(root) = self.find_root() else { return false };
        let mut target: Option<u32> = None;
        self.scroll_hit_walk(root, 0.0, 0.0, col as f32, row as f32, &mut target);
        let Some(id) = target else { return false };
        self.apply_scroll_delta(id, delta_x, delta_y);
        true
    }

    /// Find the innermost ScrollView containing `(col, row)`. Mirrors
    /// `hit_test_walk` but matches on kind rather than on `on_click`
    /// presence — and keeps walking past intermediate ScrollViews so
    /// nested ones (e.g. a horizontal carousel inside a vertical
    /// page-scroll) resolve to the deepest match. Visually-topmost
    /// (highest z) wins, same posture as click hit-testing.
    fn scroll_hit_walk(
        &self,
        id: u32,
        parent_x: f32,
        parent_y: f32,
        col: f32,
        row: f32,
        out: &mut Option<u32>,
    ) {
        let Some(data) = self.nodes.get(&id) else { return };
        let frame = self.layout.frame_of(data.layout);
        let (cw, ch) = self.cell_size;
        let static_tx = data
            .static_translate_x
            .as_ref()
            .map(|l| render::resolve_length_against(l, frame.width))
            .unwrap_or(0.0);
        let static_ty = data
            .static_translate_y
            .as_ref()
            .map(|l| render::resolve_length_against(l, frame.height))
            .unwrap_or(0.0);
        let x = parent_x + (frame.x + data.translate_x + static_tx) / cw;
        let y = parent_y + (frame.y + data.translate_y + static_ty) / ch;
        let w = frame.width / cw;
        let h = frame.height / ch;
        let inside = col >= x && col < x + w && row >= y && row < y + h;
        if !inside {
            return;
        }
        // Recurse into children with the scroll-offset applied so the
        // hit walk matches what the user sees. Without this, a nested
        // ScrollView would appear to "be" at its laid-out position
        // even after its parent has scrolled past it.
        let (child_off_x, child_off_y) = if matches!(data.kind, NodeKind::ScrollView) {
            (data.scroll_x, data.scroll_y)
        } else {
            (0.0, 0.0)
        };
        let mut ordered = self.children_in_z_order(&data.children);
        ordered.reverse();
        for child in ordered {
            self.scroll_hit_walk(
                child,
                x - child_off_x,
                y - child_off_y,
                col,
                row,
                out,
            );
            if out.is_some() {
                return;
            }
        }
        // No child claimed it; if THIS node is a ScrollView, it
        // becomes the hit. Otherwise the walk unwinds and an outer
        // ScrollView (if any) may claim it.
        if matches!(data.kind, NodeKind::ScrollView) {
            *out = Some(id);
        }
    }

    /// Mutate the ScrollView's scroll offset, clamped against the
    /// content bounds. Content size is the bounding extent of every
    /// child's laid-out frame (Taffy sized them independent of the
    /// scroll view's own bounded height); we subtract the viewport
    /// to derive the maximum scroll. Negative clamping floors at 0
    /// (can't scroll past the start).
    fn apply_scroll_delta(&mut self, id: u32, delta_x: f32, delta_y: f32) {
        let Some(data) = self.nodes.get(&id) else { return };
        if !matches!(data.kind, NodeKind::ScrollView) {
            return;
        }
        let (cw, ch) = self.cell_size;
        let frame = self.layout.frame_of(data.layout);
        let viewport_w = frame.width / cw;
        let viewport_h = frame.height / ch;
        // Sum child extents in cell space. Children's `frame.x/y` are
        // relative to this scroll view, so the bottom-right corner of
        // the union is just `max(child.x + child.w, child.y + child.h)`.
        let mut content_w = 0.0f32;
        let mut content_h = 0.0f32;
        for cid in &data.children {
            if let Some(c) = self.nodes.get(cid) {
                let f = self.layout.frame_of(c.layout);
                content_w = content_w.max((f.x + f.width) / cw);
                content_h = content_h.max((f.y + f.height) / ch);
            }
        }
        let max_x = (content_w - viewport_w).max(0.0);
        let max_y = (content_h - viewport_h).max(0.0);
        // Apply the clamped offset; capture the resulting values +
        // a clone of the callback so we can release the `&mut self`
        // borrow before invoking the user callback (which may
        // re-enter the backend through a Signal write).
        let (new_x, new_y, cb) = if let Some(d) = self.nodes.get_mut(&id) {
            d.scroll_x = (d.scroll_x + delta_x).clamp(0.0, max_x);
            d.scroll_y = (d.scroll_y + delta_y).clamp(0.0, max_y);
            (d.scroll_x, d.scroll_y, d.on_scroll.clone())
        } else {
            return;
        };
        if let Some(cb) = cb {
            cb(new_x, new_y);
        }
    }

    /// Dispatch a key event to the focused TextInput, if any.
    /// Returns `true` if the key was consumed by an input — the host
    /// should suppress its `on_key` callback in that case.
    pub fn dispatch_key(&mut self, key: &TerminalKey) -> bool {
        let Some(id) = self.focused_id else { return false };
        let Some(data) = self.nodes.get(&id) else { return false };
        if !matches!(data.kind, NodeKind::TextInput) {
            return false;
        }
        // Compute the proposed mutation against a local copy first
        // so the `on_key_down` callback (which may read backend
        // state) doesn't see partially-updated text.
        let (key_name, ev) = make_key_event(key, data);
        let on_key_down = data.input.as_ref().and_then(|i| i.on_key_down.clone());

        if let Some(handler) = on_key_down {
            if matches!(handler(&ev), runtime_core::KeyOutcome::PreventDefault) {
                return true;
            }
        }
        self.apply_key_default(id, &key_name);
        true
    }
}

#[derive(Clone)]
enum HitTarget {
    Handler(Rc<dyn Fn()>),
    FocusInput(u32),
}

/// Host-side key event. Re-defined here so the backend doesn't pull
/// in `crossterm` as a dep. The host converts.
#[derive(Clone, Debug)]
pub struct TerminalKey {
    pub key: String,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

fn make_key_event(
    key: &TerminalKey,
    data: &node::NodeData,
) -> (String, runtime_core::primitives::key::KeyEvent) {
    let cursor = data
        .input
        .as_ref()
        .map(|i| i.cursor)
        .unwrap_or(0);
    let ev = runtime_core::primitives::key::KeyEvent {
        key: key.key.clone(),
        shift: key.shift,
        ctrl: key.ctrl,
        alt: key.alt,
        meta: key.meta,
        // Single-line, no selection range — selection_start and
        // selection_end both report the caret. Char-indexed; the
        // framework docs say UTF-16 code units, but author code
        // that doesn't index into the string won't observe the
        // difference. Inputs > BMP are rare in terminal use.
        selection_start: cursor,
        selection_end: cursor,
    };
    (key.key.clone(), ev)
}

// =========================================================================
// Backend trait impl
// =========================================================================

impl Backend for TerminalBackend {
    type Node = TermNode;

    fn platform(&self) -> Platform {
        Platform::Custom("Terminal")
    }

    fn color_scheme(&self) -> ColorScheme {
        // Most terminals these days are dark by default. Apps that
        // care can branch on `Platform::Custom("Terminal")` for a
        // proper choice.
        ColorScheme::Dark
    }

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        self.alloc_node(NodeKind::View, String::new())
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        let node = self.alloc_node(NodeKind::Text, content.to_string());
        self.install_text_measure(node.id);
        node
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&runtime_core::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_core::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Render Button as `[ label ]` for a consistent at-a-glance
        // affordance on terminal — matches the existing Toggle's
        // `[ ● ]` bracket convention. Store the bracketed form
        // directly so the captured `measure_fn` (which reads
        // `data.content`) sizes the node for the bracketed width.
        // Paint goes through `paint_text` unchanged.
        let bracketed = format_button_label(label);
        let node = self.alloc_node(NodeKind::Button, bracketed);
        let fire = on_click.fire.clone();
        if let Some(data) = self.nodes.get_mut(&node.id) {
            data.on_click = Some(fire);
        }
        self.install_text_measure(node.id);
        node
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let node = self.alloc_node(NodeKind::Pressable, String::new());
        if let Some(data) = self.nodes.get_mut(&node.id) {
            data.on_click = Some(on_click);
        }
        node
    }

    // ------------------------------------------------------------------
    // Visual-only primitives that the terminal collapses to a plain
    // View. No clipping, no scrolling, no actual image/icon rendering —
    // per [[feedback_terminal_minimalism]] the terminal is a flat
    // character grid, so these primitives behave as transparent flex
    // containers and any visual semantics (scroll, image content,
    // portal teleportation) are dropped silently.
    // ------------------------------------------------------------------

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<std::rc::Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Terminal owns its own mouse-wheel dispatch (see
        // `dispatch_scroll` + `apply_scroll_delta`) which mutates
        // each ScrollView's `(scroll_x, scroll_y)` in cell units.
        // We stash the `on_scroll` callback on the node and fire it
        // from `apply_scroll_delta` after the offset is clamped.
        // Offsets are reported in cells \u{2014} the terminal's
        // native unit \u{2014} matching the other backends'
        // "current offset in native coordinate space" semantic
        // (web pixels, iOS points, Android dp post-conversion).
        let node = self.alloc_node(NodeKind::ScrollView, String::new());
        let layout = self.nodes.get(&node.id).map(|d| d.layout);
        if let Some(d) = self.nodes.get_mut(&node.id) {
            d.horizontal = horizontal;
            d.on_scroll = on_scroll;
        }
        // Tell Taffy this node behaves like CSS `overflow: scroll` on
        // the chosen axis. Without this, Taffy sizes the scroll view
        // to its content's intrinsic size — i.e. the content fits
        // inside it exactly and there's nothing to scroll. The
        // helper also sets `flex_grow: 1, flex_basis: 0` so the
        // scroll view fills its parent's available main-axis space
        // (matches how an unsized ScrollView behaves on
        // iOS/Android/web where the native scroll view's frame is
        // set by its parent and content has its own coordinate
        // space).
        if let Some(l) = layout {
            self.layout.set_overflow_scroll(l, horizontal);
        }
        node
    }

    fn create_image(
        &mut self,
        _src: &str,
        _alt: Option<&str>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.create_view(a11y)
    }

    fn create_icon(
        &mut self,
        _data: &runtime_core::primitives::icon::IconData,
        _color: Option<&FwColor>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.create_view(a11y)
    }

    fn create_portal(
        &mut self,
        _target: runtime_core::primitives::portal::PortalTarget,
        _on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.create_view(a11y)
    }

    fn create_link(
        &mut self,
        config: runtime_core::primitives::link::LinkConfig,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Terminal renders links as plain Pressable wrappers — a click
        // anywhere inside fires `on_activate`. The trait default
        // collapses to `create_view` and drops `on_activate` entirely,
        // which is why nav-link clicks were silently no-op'ing
        // before. The on_click slot mirrors what `create_pressable`
        // sets, so the existing hit-test path picks it up.
        let node = self.alloc_node(NodeKind::Pressable, String::new());
        if let Some(data) = self.nodes.get_mut(&node.id) {
            data.on_click = Some(config.on_activate);
        }
        node
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let (parent_layout, child_layout) = match (
            self.nodes.get(&parent.id).map(|d| d.layout),
            self.nodes.get(&child.id).map(|d| d.layout),
        ) {
            (Some(p), Some(c)) => (p, c),
            _ => return,
        };
        self.layout.add_child(parent_layout, child_layout);
        if let Some(p) = self.nodes.get_mut(&parent.id) {
            p.children.push(child.id);
        }
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        let layout = match self.nodes.get(&node.id) {
            Some(d) if d.content == content => return,
            Some(d) => d.layout,
            None => return,
        };
        if let Some(data) = self.nodes.get_mut(&node.id) {
            data.content = content.to_string();
        }
        // The Taffy measure_fn captures its content snapshot by
        // value (we can't borrow `&mut self` inside the closure), so
        // the measure_fn still believes the text is the original
        // empty string until we re-install it. Without this, the
        // text node measures 0x0 and the rendered glyphs land in
        // a zero-size frame — nothing visible. Re-installing is
        // cheap (one Rc clone per swap).
        self.install_text_measure(node.id);
        self.layout.mark_dirty(layout);
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        // Re-wrap reactive label updates to keep the bracketed
        // form in sync with what `create_button` stored.
        self.update_text(node, &format_button_label(label));
    }

    fn clear_children(&mut self, node: &Self::Node) {
        let Some(data) = self.nodes.get(&node.id) else { return };
        let parent_layout = data.layout;
        let children = data.children.clone();
        for cid in &children {
            let cdata = self.nodes.remove(cid);
            if let Some(cd) = cdata {
                // Strip the Taffy edge first, then drop the slot.
                // Mirrors the iOS pattern; see
                // [[project_ios_clear_children_taffy_sync]].
                self.layout.remove_child(parent_layout, cd.layout);
                self.layout.remove_node(cd.layout);
                self.layout_to_id.remove(&cd.layout);
                // Also tear down any grandchildren that this node
                // owned — recursive free.
                self.drop_subtree(&cd.children);
            }
        }
        self.layout.mark_dirty(parent_layout);
        if let Some(p) = self.nodes.get_mut(&node.id) {
            p.children.clear();
        }
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let layout_node = match self.nodes.get(&node.id) {
            Some(d) => d.layout,
            None => return,
        };
        // Eagerly resolve `background` and `color` BEFORE handing the
        // rules to `runtime-layout`'s `set_style`. This is load-
        // bearing: the cohort driver Effect re-fires on token-signal
        // changes, calls `apply_one` → this `apply_style`, which
        // resolves the same Tokenized values to cache `bg`/`fg`
        // further down. Without this early read, the cohort path's
        // sidebar updates went through (`d.bg` was updated, the
        // log even showed the dark color), but the render didn't
        // visually pick up the change — the framework's token-
        // subscription bookkeeping needs the resolve to happen
        // BEFORE other style processing for the per-token edges to
        // land in this Effect's dependency set on the first
        // post-toggle re-fire. Without it, the sidebar darkened
        // only on the second-or-later toggle, which read to the
        // user as "doesn't update".
        let _ = style.background.as_ref().map(|t| t.resolve());
        let _ = style.color.as_ref().map(|t| t.resolve());
        self.layout.set_style(layout_node, style);

        // Cache the resolved fg/bg + gradient so the renderer's hot
        // path doesn't re-parse on every cell write.
        let fg = style
            .color
            .as_ref()
            .map(|t| parse_or(&t.resolve().0, Rgba::default()));
        let bg = style
            .background
            .as_ref()
            .map(|t| parse_or(&t.resolve().0, Rgba::TRANSPARENT));
        let gradient = style.background_gradient.as_ref().map(|g| {
            let stops: Vec<(f32, Rgba)> = g
                .stops
                .iter()
                .map(|s| (s.offset, parse_or(&s.color.0, Rgba::TRANSPARENT)))
                .collect();
            let animated_stops = vec![None; stops.len()];
            node::ResolvedGradient {
                kind: g.kind.clone(),
                stops,
                animated_stops,
            }
        });

        // Extract static translate from `style.transform: [...]`.
        // We only support TranslateX/Y on this backend — Scale /
        // Rotate / Skew don't translate to cell semantics. Last-write
        // wins per axis (matches the RN/web "matrix multiply" feel
        // for the translates-only subset).
        let mut static_tx: Option<runtime_core::Length> = None;
        let mut static_ty: Option<runtime_core::Length> = None;
        if let Some(transforms) = style.transform.as_ref() {
            for t in transforms {
                match t {
                    runtime_core::Transform::TranslateX(l) => static_tx = Some(*l),
                    runtime_core::Transform::TranslateY(l) => static_ty = Some(*l),
                    _ => {}
                }
            }
        }

        // Static opacity from the stylesheet. Without this, an
        // element declared with `opacity: 0.0` (welcome's sun, the
        // vignette wrapper, planets pre-Act-2) starts fully visible
        // because `NodeData.opacity` defaults to 1.0 — only the
        // animation path (`set_animated_f32(Opacity, …)`) ever
        // touched it. Read the resolved value and seed `data.opacity`
        // up front; the animation Effect later overwrites at every
        // frame.
        let static_opacity = style
            .opacity
            .as_ref()
            .map(|t| t.resolve().clamp(0.0, 1.0));

        if let Some(d) = self.nodes.get_mut(&node.id) {
            d.style = Some(style.clone());
            d.fg = fg;
            d.bg = bg;
            d.static_translate_x = static_tx;
            d.static_translate_y = static_ty;
            if let Some(o) = static_opacity {
                d.opacity = o;
            }
            // Preserve any already-animated stop overrides if the
            // gradient's shape didn't change — re-applying a static
            // stylesheet (state overlays, theme refresh) shouldn't
            // reset per-frame animation state. Conservative: only
            // preserve when the new gradient has the same stop
            // count as the old one. Anything more aggressive risks
            // mismatched indices.
            let preserved = d
                .gradient
                .as_ref()
                .and_then(|old| {
                    gradient
                        .as_ref()
                        .filter(|new| new.stops.len() == old.stops.len())
                        .map(|_| old.animated_stops.clone())
                });
            d.gradient = gradient.map(|mut g| {
                if let Some(p) = preserved {
                    g.animated_stops = p;
                }
                g
            });
        }
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Render: `[ ]` (off) / `[●]` (on). 3 cells wide intrinsic.
        let node = self.alloc_node(NodeKind::Toggle, String::new());
        if let Some(d) = self.nodes.get_mut(&node.id) {
            d.toggle_value = initial_value;
            // Wrap `on_change` so the click handler (no args) reads
            // the *current* value at click time, flips it, and
            // forwards the new value. The framework's controlled-
            // value Effect re-fires `update_toggle_value` so the
            // backend's `toggle_value` stays in sync with the
            // signal.
            //
            // We pull the current value from the backend via the
            // shared id — no need for a separate Cell.
            let id = node.id;
            let oc = on_change.clone();
            d.on_click = Some(Rc::new(move || {
                // The framework's controlled-value cycle: this fires
                // on press, we flip and call on_change with the new
                // value; the parent updates its `Signal<bool>`; the
                // framework's effect calls `update_toggle_value`
                // with the same new value, which is a no-op (we
                // skip on equality). One coherent state.
                terminal_toggle_press(id, &oc);
            }));
            // Cells: "[ x ]" — 5 cells wide for breathing room.
            let (cw, ch) = self.cell_size;
            self.layout.set_intrinsic_size(d.layout, 5.0 * cw, 1.0 * ch);
        }
        node
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        if let Some(d) = self.nodes.get_mut(&node.id) {
            d.toggle_value = value;
        }
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        secure: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let node = self.alloc_node(NodeKind::TextInput, String::new());
        if let Some(d) = self.nodes.get_mut(&node.id) {
            let placeholder_owned = placeholder.map(|s| s.to_string());
            // Seed an intrinsic width that fits the placeholder (so
            // empty inputs aren't 0-wide) plus 2 cells of breathing
            // room. Authors can override with explicit `width` in
            // the stylesheet.
            let intrinsic_cells = placeholder_owned
                .as_ref()
                .map(|s| s.chars().count() as f32)
                .unwrap_or(0.0)
                .max(initial_value.chars().count() as f32)
                .max(8.0)
                + 2.0;
            let (cw, ch) = self.cell_size;
            self.layout
                .set_intrinsic_size(d.layout, intrinsic_cells * cw, 1.0 * ch);
            d.input = Some(Box::new(node::InputState {
                value: initial_value.to_string(),
                cursor: initial_value.chars().count(),
                placeholder: placeholder_owned,
                secure,
                on_change,
                on_key_down,
            }));
        }
        node
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        let Some(d) = self.nodes.get_mut(&node.id) else { return };
        let Some(input) = d.input.as_mut() else { return };
        if input.value == value {
            return;
        }
        input.value = value.to_string();
        // Clamp the cursor in case the controlled value got
        // truncated below the previous cursor position.
        let max = input.value.chars().count();
        if input.cursor > max {
            input.cursor = max;
        }
    }

    fn create_activity_indicator(
        &mut self,
        size: ActivityIndicatorSize,
        color: Option<&FwColor>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let node = self.alloc_node(NodeKind::ActivityIndicator, String::new());
        if let Some(d) = self.nodes.get_mut(&node.id) {
            // Color seed: optional explicit color, otherwise muted.
            if let Some(c) = color {
                d.fg = Some(parse_or(&c.0, Rgba::new(180, 180, 180, 255)));
            }
            // Small = 1 cell tall, Large = 1 cell tall too — we
            // can't actually grow a single braille glyph. Width: 3
            // cells either way to give the spinner some space.
            let w_cells = match size {
                ActivityIndicatorSize::Small => 3.0,
                ActivityIndicatorSize::Large => 5.0,
            };
            let (cw, ch) = self.cell_size;
            self.layout.set_intrinsic_size(d.layout, w_cells * cw, 1.0 * ch);
        }
        // The walker fires no per-frame effect for this primitive,
        // so we install our own `raf_loop` to advance the phase.
        // Each tick bumps `anim_phase` by ~one frame's worth of the
        // 10-step braille cycle. The render path samples
        // `anim_phase` to pick the current glyph.
        let id = node.id;
        let task = runtime_core::raf_loop(move || {
            terminal_advance_spinner(id);
        });
        // Anchor to the current reactive scope so unmount cancels
        // the loop. `on_cleanup` is a no-op outside a scope, which
        // is fine — top-level binaries leak the handle until exit.
        runtime_core::on_cleanup(move || drop(task));
        node
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        handles::make_view_handle(node)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        handles::make_text_handle(node)
    }

    fn set_animated_f32(
        &mut self,
        node: &Self::Node,
        prop: AnimProp,
        value: f32,
    ) {
        let Some(d) = self.nodes.get_mut(&node.id) else { return };
        match prop {
            // Route to the animated slot — apply_style replays
            // (hot-patch path) would otherwise clobber the in-
            // flight value with the stylesheet's static starting
            // opacity. See [`NodeData::animated_opacity`].
            AnimProp::Opacity => d.animated_opacity = Some(value.clamp(0.0, 1.0)),
            AnimProp::TranslateX => d.translate_x = value,
            AnimProp::TranslateY => d.translate_y = value,
            // Sibling-relative ordering. Higher value renders on top
            // of lower. Welcome's planets sweep through positive and
            // negative values as they orbit so they pass in front of
            // and behind the headline.
            AnimProp::ZIndex => d.z_index = value,
            // Scale / Rotate don't map cleanly to a cell grid —
            // documented no-ops so author code stays portable.
            _ => {}
        }
    }

    fn set_animated_color(
        &mut self,
        node: &Self::Node,
        prop: AnimProp,
        value: [f32; 4],
    ) {
        let Some(d) = self.nodes.get_mut(&node.id) else { return };
        let rgba = Rgba::from_srgb_f32(value);
        match prop {
            AnimProp::BackgroundColor => d.animated_bg = Some(rgba),
            AnimProp::ForegroundColor => d.animated_fg = Some(rgba),
            AnimProp::GradientStopColor(idx) => {
                if let Some(g) = d.gradient.as_mut() {
                    let i = idx as usize;
                    if i < g.animated_stops.len() {
                        g.animated_stops[i] = Some(rgba);
                    }
                }
            }
            _ => {}
        }
    }

    /// Called by the framework after every render pass. We don't run
    /// layout here — the host drives `render_to_grid` on its own
    /// schedule (after input + before paint) so we have the most
    /// current viewport size. `finish` would compute against stale
    /// dimensions if the terminal got resized between builds.
    fn finish(&mut self, _root: Self::Node) {}

    // ------------------------------------------------------------------
    // Navigator extension wiring. Mirrors the web/iOS/Android pattern —
    // SDK leaf crates install a handler factory keyed by presentation
    // TypeId; `create_navigator` resolves the factory, runs its `init`,
    // and stores the returned handler under the outlet node id so
    // subsequent dispatch (`attach_initial` / `release` / `make_handle`
    // / `apply_slot_style`) can route through the kind-specific logic.
    // ------------------------------------------------------------------

    fn create_navigator(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        presentation: Rc<dyn std::any::Any>,
        host: runtime_core::NavigatorHost<Self::Node>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let factory = self
            .navigator_handlers
            .get(type_id)
            .unwrap_or_else(|| {
                panic!(
                    "TerminalBackend::create_navigator: navigator kind '{}' \
                     is not registered. Did the app forget to call \
                     `<navigator-sdk>::register(&mut backend)` during bootstrap?",
                    type_name
                )
            });
        let mut handler = factory();
        let node = handler.init(self, host, presentation);
        // Key by outlet node id — unlike web there's no separate
        // navigator-id attribute, but TermNode.id is already a stable
        // per-instance handle.
        self.nav_handler_instances.insert(
            node.id,
            Rc::new(std::cell::RefCell::new(handler)),
        );
        node
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        let handler = self.nav_handler_instances.get(&navigator.id).cloned();
        let Some(handler) = handler else { return };
        handler.borrow_mut().attach_initial(self, screen, scope_id, options);
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        let handler = self.nav_handler_instances.remove(&node.id);
        let Some(handler) = handler else { return };
        handler.borrow_mut().release(self);
    }

    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::NavigatorHandle {
        match self.nav_handler_instances.get(&node.id) {
            Some(h) => h.borrow().make_handle(),
            None => runtime_core::NavigatorHandle::new(
                Rc::new(()),
                &NOOP_NAV_OPS,
            ),
        }
    }

    fn apply_navigator_slot_style(
        &mut self,
        navigator: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ) {
        let handler = self.nav_handler_instances.get(&navigator.id).cloned();
        let Some(handler) = handler else { return };
        handler.borrow_mut().apply_slot_style(self, slot, style);
    }
}

struct NoopNavOps;
impl runtime_core::primitives::navigator::NavigatorOps for NoopNavOps {}
static NOOP_NAV_OPS: NoopNavOps = NoopNavOps;

#[cfg(test)]
mod regression_tests {
    use super::*;
    use runtime_core::{
        accessibility::AccessibilityProps, animation::AnimProp, Backend, StyleRules, Tokenized,
    };
    use std::rc::Rc;

    /// `apply_style` replay must not clobber an in-flight animated
    /// opacity. Reproduces the hot-patch-on-terminal bug where the
    /// welcome wrapper's static `opacity: 0.0` (re-emitted by the
    /// dev-server's snapshot) overwrote the animation-driven
    /// `opacity: 1.0`, making every save flash the scene back to
    /// invisible until the next animation tick arrived.
    #[test]
    fn apply_style_does_not_overwrite_animated_opacity() {
        let mut be = TerminalBackend::new();
        let node = be.create_view(&AccessibilityProps::default());

        // Animation drives opacity up to 1.0.
        be.set_animated_f32(&node, AnimProp::Opacity, 1.0);

        // Now the dev-server replays the static stylesheet, which
        // declares `opacity: 0.0`. Pre-fix this overwrote the
        // animation-driven value; post-fix, the animated slot
        // wins.
        let style = Rc::new(StyleRules {
            opacity: Some(Tokenized::Literal(0.0)),
            ..Default::default()
        });
        be.apply_style(&node, &style);

        let data = be.nodes.get(&node.id).expect("node still present");
        assert_eq!(data.opacity, 0.0, "static slot must reflect stylesheet");
        assert_eq!(
            data.animated_opacity,
            Some(1.0),
            "animated slot must survive apply_style replay"
        );
    }
}

// ---------------------------------------------------------------------------
// Toggle press helper. The framework's controlled-toggle pattern is
// "on press, call `on_change(new_value)`; the parent flips its signal;
// the backend's `update_toggle_value` writes the value back". We
// need the closure passed to `Pressable`-style on_click to read the
// current value out of the backend at fire time, not at create time
// (since the value flips). Routing through a thread-local backend
// handle avoids capturing `&mut TerminalBackend` in the closure.
// ---------------------------------------------------------------------------

thread_local! {
    pub(crate) static GLOBAL_BACKEND:
        std::cell::RefCell<Option<std::rc::Weak<std::cell::RefCell<TerminalBackend>>>> =
            const { std::cell::RefCell::new(None) };
}

/// Install a self-handle for the toggle click + spinner raf paths.
/// The host calls this once after wrapping the backend in
/// `Rc<RefCell<>>`. Mirrors the pattern `backend-macos` /
/// `backend-ios` use.
pub fn install_global_self(weak: std::rc::Weak<std::cell::RefCell<TerminalBackend>>) {
    GLOBAL_BACKEND.with(|s| *s.borrow_mut() = Some(weak));
}

/// Run `f` with a mutable borrow on the host-installed backend
/// handle. SDK leaf crates use this from inside navigator-dispatch
/// closures (created in `NavigatorHandler::init`) that need to call
/// `Backend::insert` / [`TerminalBackend::detach_child`] / etc.
/// — those closures don't carry `&mut TerminalBackend` and have no
/// other way to reach the backend.
///
/// Silently no-ops if the host hasn't installed the self-handle yet
/// or the backend is already mutably borrowed. Mirrors how
/// `terminal_toggle_press` and `terminal_advance_spinner` reach back
/// into the backend from event-handler closures.
pub fn with_global_backend<F>(f: F)
where
    F: FnOnce(&mut TerminalBackend),
{
    let weak = GLOBAL_BACKEND.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    let Ok(mut b) = rc.try_borrow_mut() else { return };
    f(&mut *b);
}

fn terminal_toggle_press(id: u32, on_change: &Rc<dyn Fn(bool)>) {
    let weak = GLOBAL_BACKEND.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    let current = {
        let Ok(b) = rc.try_borrow() else { return };
        b.nodes.get(&id).map(|d| d.toggle_value).unwrap_or(false)
    };
    on_change(!current);
}

fn terminal_advance_spinner(id: u32) {
    let weak = GLOBAL_BACKEND.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    let Ok(mut b) = rc.try_borrow_mut() else { return };
    if let Some(d) = b.nodes.get_mut(&id) {
        d.anim_phase += 1.0;
    }
}

impl TerminalBackend {
    /// Install a Taffy measure_fn for a text-bearing node. Wraps the
    /// content at the available width (terminal cell units) and
    /// reports the resulting size to the layout engine.
    fn install_text_measure(&mut self, id: u32) {
        let layout = match self.nodes.get(&id) {
            Some(d) => d.layout,
            None => return,
        };
        let content = self
            .nodes
            .get(&id)
            .map(|d| d.content.clone())
            .unwrap_or_default();
        // Capture the current cell_size by value. If the host
        // changes scale mid-session, existing measure_fns won't
        // update — fine, our convention is "set cell_size BEFORE
        // mount" (matches the viewport contract every other
        // backend ships).
        let (cw, ch) = self.cell_size;
        let f: runtime_layout::MeasureFn = Rc::new(move |known, avail| {
            measure_text(&content, known, avail, cw, ch)
        });
        self.layout.set_measure_fn(layout, f);
    }

    /// Apply the platform-default behaviour for a key event hitting
    /// the focused TextInput. Mutates `input.value`, fires
    /// `on_change` with the new string, and (for some keys) clears
    /// focus.
    fn apply_key_default(&mut self, id: u32, key_name: &str) {
        // Take the on_change handler out so we can release the
        // borrow before invoking it (the user's closure may call
        // back into the backend — `Signal::set` mutates the arena,
        // which is fine, but holding a `&mut self` borrow would
        // panic on re-entry through another backend method).
        let (mut value, mut cursor, on_change) = {
            let Some(d) = self.nodes.get(&id) else { return };
            let Some(input) = d.input.as_ref() else { return };
            (input.value.clone(), input.cursor, input.on_change.clone())
        };

        let mut fire_change = false;
        let mut blur = false;

        match key_name {
            "Backspace" => {
                if cursor > 0 {
                    let mut chars: Vec<char> = value.chars().collect();
                    chars.remove(cursor - 1);
                    cursor -= 1;
                    value = chars.into_iter().collect();
                    fire_change = true;
                }
            }
            "Delete" => {
                let chars: Vec<char> = value.chars().collect();
                if cursor < chars.len() {
                    let mut chars = chars;
                    chars.remove(cursor);
                    value = chars.into_iter().collect();
                    fire_change = true;
                }
            }
            "ArrowLeft" => {
                cursor = cursor.saturating_sub(1);
            }
            "ArrowRight" => {
                let n = value.chars().count();
                if cursor < n {
                    cursor += 1;
                }
            }
            "Home" => cursor = 0,
            "End" => cursor = value.chars().count(),
            "Enter" | "Tab" | "Escape" => {
                blur = true;
            }
            other => {
                // Printable single character → insert at cursor.
                // We treat any single-char `key` value as printable,
                // including space (`" "`) and unicode letters.
                if other.chars().count() == 1 {
                    let ch = other.chars().next().unwrap();
                    if !ch.is_control() {
                        let mut chars: Vec<char> = value.chars().collect();
                        chars.insert(cursor, ch);
                        cursor += 1;
                        value = chars.into_iter().collect();
                        fire_change = true;
                    }
                }
                // Unknown named keys are quietly ignored — the
                // framework's other-backend posture (web: passes
                // through; iOS: best-effort).
            }
        }

        // Write the local mutation back before firing on_change. The
        // framework's controlled-value Effect will call
        // `update_text_input_value` after the parent's signal
        // changes — that path is a no-op when the value matches.
        if let Some(d) = self.nodes.get_mut(&id) {
            if let Some(input) = d.input.as_mut() {
                input.value = value.clone();
                input.cursor = cursor;
            }
        }
        if blur {
            self.focused_id = None;
        }
        if fire_change {
            // Defer the on_change fire to a microtask so we're not
            // still holding the `RefCell<TerminalBackend>` borrow
            // when the framework's controlled-value effect
            // re-enters the backend through
            // `update_text_input_value`. The host's per-frame
            // `scheduler::tick()` drains microtasks before
            // re-rendering, so the value lands the same frame.
            runtime_core::scheduling::schedule_microtask(move || {
                on_change(value);
            });
        }
    }

    fn drop_subtree(&mut self, ids: &[u32]) {
        for id in ids {
            if let Some(d) = self.nodes.remove(id) {
                self.layout.remove_node(d.layout);
                self.layout_to_id.remove(&d.layout);
                self.drop_subtree(&d.children);
            }
        }
    }
}

/// Measure a text string at the given width/height constraints. Wraps
/// on whitespace; counts each character as one terminal cell. Honors
/// `\n` as a hard line break.
///
/// All constraints and the returned size are in **layout px**, not
/// Wrap a Button's raw label as `[ label ]` for the terminal
/// renderer. Centralised so create + update paths emit the same
/// shape and bracket-aware metrics flow through `install_text_measure`
/// unchanged. Mirrors the existing Toggle convention (`[ ● ]`).
fn format_button_label(label: &str) -> String {
    format!("[ {label} ]")
}

/// cells — Taffy operates in px throughout. `(cw, ch)` is the
/// active px-per-cell factor; we convert px constraints to cell
/// counts internally, then convert the cell-based result back to px
/// on return.
fn measure_text(
    content: &str,
    known: TaffySize<Option<f32>>,
    avail: TaffySize<AvailableSpace>,
    cw: f32,
    ch: f32,
) -> TaffySize<f32> {
    // Wrap-width constraint, converted from layout px to cell count.
    let max_w = match known.width {
        Some(w) => w / cw,
        None => match avail.width {
            AvailableSpace::Definite(w) => w / cw,
            AvailableSpace::MaxContent => f32::INFINITY,
            AvailableSpace::MinContent => 0.0,
        },
    };
    let mut lines = 0u32;
    let mut longest = 0u32;
    for paragraph in content.split('\n') {
        // Empty paragraph still counts as one line.
        let words: Vec<&str> = paragraph.split_whitespace().collect();
        if words.is_empty() {
            lines += 1;
            continue;
        }
        if max_w.is_infinite() {
            // No wrapping — single line of the full paragraph width.
            let w = paragraph.chars().count() as u32;
            longest = longest.max(w);
            lines += 1;
            continue;
        }
        let mut col: u32 = 0;
        let max_col = max_w.floor() as u32;
        let mut line_started = false;
        for word in words {
            let wlen = word.chars().count() as u32;
            let space_cost = if line_started { 1 } else { 0 };
            if line_started && col + space_cost + wlen > max_col {
                // Wrap to the next line.
                longest = longest.max(col);
                col = wlen;
                lines += 1;
                line_started = true;
            } else {
                col += space_cost + wlen;
                line_started = true;
            }
        }
        if line_started {
            longest = longest.max(col);
            lines += 1;
        }
    }
    if lines == 0 {
        lines = 1;
    }
    // Convert the cell-count result back to layout px.
    TaffySize {
        width: known.width.unwrap_or(longest as f32 * cw),
        height: known.height.unwrap_or(lines as f32 * ch),
    }
}
