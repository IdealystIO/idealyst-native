//! ASCII / terminal backend for `framework_core::Backend`.
//!
//! Renders the framework's primitive tree into a character grid. The
//! companion `host-terminal` crate paints the grid to stdout (ANSI
//! escapes for color) and forwards mouse / keyboard events back into
//! the backend's hit-tester so `Pressable` / `Button` `on_click`
//! callbacks fire.
//!
//! Layout is delegated to `native-layout` (Taffy), same as iOS /
//! Android / macOS — flex containers, gap, padding, width/height,
//! `position: absolute` all work. The unit on this backend is
//! **terminal cell** (1 col x 1 row), not pixel — author stylesheets
//! that say `width: 40` get 40 columns wide.

mod handles;
mod node;
mod render;

pub use node::{NodeKind, TermNode};
pub use render::{Cell, Grid};

/// Outcome of dispatching a mouse click through
/// [`TerminalBackend::dispatch_click`]. Lets the host know whether
/// the click was consumed (so the global handler doesn't double-fire).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClickOutcome {
    /// Click landed on a clickable node and the handler fired.
    HandlerFired,
    /// Click landed on a TextInput and focus was set.
    FocusedInput,
    /// Click landed somewhere with no handler / no input. Focus
    /// (if any) was cleared.
    Unhandled,
}

use std::collections::HashMap;
use std::rc::Rc;

use framework_core::accessibility::AccessibilityProps;
use framework_core::animation::AnimProp;
use framework_core::color::{parse_or, Rgba};
use framework_core::primitives::activity_indicator::ActivityIndicatorSize;
use framework_core::{Action, Backend, Color as FwColor, ColorScheme, Platform, StyleRules};
use native_layout::{AvailableSpace, LayoutNode, LayoutTree, Size as TaffySize};

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
}

impl Default for TerminalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalBackend {
    pub fn new() -> Self {
        Self {
            layout: LayoutTree::new(),
            nodes: HashMap::new(),
            next_id: 1,
            layout_to_id: HashMap::new(),
            viewport: (80, 24),
        }
    }

    /// Update the viewport size in cells. Next `render_to_grid` call
    /// uses the new size for the layout pass.
    pub fn set_viewport(&mut self, cols: u16, rows: u16) {
        self.viewport = (cols, rows);
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
                opacity: 1.0,
                translate_x: 0.0,
                translate_y: 0.0,
                animated_bg: None,
                animated_fg: None,
                toggle_value: false,
                anim_phase: 0.0,
            },
        );
        TermNode { id }
    }

    /// Walk the backend's nodes and find the root view (no parent
    /// edge in the layout tree). Returns the first such node — there
    /// is typically only one (the `mount` entry root).
    pub(crate) fn find_root(&self) -> Option<u32> {
        for (id, data) in &self.nodes {
            if self.layout.is_root(data.layout) {
                return Some(*id);
            }
        }
        None
    }

    /// Hit-test the rendered tree at terminal-cell coordinates
    /// `(col, row)`. Returns the deepest `on_click` handler whose
    /// frame contains the point. Called by the host on mouse-down /
    /// click events.
    ///
    /// Must be called after a `render_to_grid` (or other compute)
    /// call so frames are populated.
    pub fn hit_test(&self, col: u16, row: u16) -> Option<Rc<dyn Fn()>> {
        let root = self.find_root()?;
        let mut found: Option<Rc<dyn Fn()>> = None;
        self.hit_test_walk(root, 0.0, 0.0, col as f32, row as f32, &mut found);
        found
    }

    fn hit_test_walk(
        &self,
        id: u32,
        parent_x: f32,
        parent_y: f32,
        col: f32,
        row: f32,
        out: &mut Option<Rc<dyn Fn()>>,
    ) {
        let Some(data) = self.nodes.get(&id) else { return };
        let frame = self.layout.frame_of(data.layout);
        let x = parent_x + frame.x;
        let y = parent_y + frame.y;
        let inside =
            col >= x && col < x + frame.width && row >= y && row < y + frame.height;
        if !inside {
            return;
        }
        // Visit children first so the deepest hit wins (children
        // paint on top).
        for &child in &data.children {
            self.hit_test_walk(child, x, y, col, row, out);
        }
        if out.is_none() {
            if let Some(handler) = &data.on_click {
                *out = Some(handler.clone());
            }
        }
    }
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
        _leading_icon: Option<&framework_core::primitives::icon::IconData>,
        _trailing_icon: Option<&framework_core::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let node = self.alloc_node(NodeKind::Button, label.to_string());
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
        self.update_text(node, label);
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
        self.layout.set_style(layout_node, style);

        // Cache the resolved fg/bg so the renderer doesn't have to
        // re-parse on every cell write.
        let fg = style
            .color
            .as_ref()
            .map(|t| parse_or(&t.resolve().0, Rgba::default()));
        let bg = style
            .background
            .as_ref()
            .map(|t| parse_or(&t.resolve().0, Rgba::TRANSPARENT));

        if let Some(d) = self.nodes.get_mut(&node.id) {
            d.style = Some(style.clone());
            d.fg = fg;
            d.bg = bg;
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
            self.layout.set_intrinsic_size(d.layout, 5.0, 1.0);
        }
        node
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        if let Some(d) = self.nodes.get_mut(&node.id) {
            d.toggle_value = value;
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
            let w = match size {
                ActivityIndicatorSize::Small => 3.0,
                ActivityIndicatorSize::Large => 5.0,
            };
            self.layout.set_intrinsic_size(d.layout, w, 1.0);
        }
        // The walker fires no per-frame effect for this primitive,
        // so we install our own `raf_loop` to advance the phase.
        // Each tick bumps `anim_phase` by ~one frame's worth of the
        // 10-step braille cycle. The render path samples
        // `anim_phase` to pick the current glyph.
        let id = node.id;
        let task = framework_core::raf_loop(move || {
            terminal_advance_spinner(id);
        });
        // Anchor to the current reactive scope so unmount cancels
        // the loop. `on_cleanup` is a no-op outside a scope, which
        // is fine — top-level binaries leak the handle until exit.
        framework_core::on_cleanup(move || drop(task));
        node
    }

    fn make_view_handle(&self, node: &Self::Node) -> framework_core::ViewHandle {
        handles::make_view_handle(node)
    }

    fn make_text_handle(&self, node: &Self::Node) -> framework_core::TextHandle {
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
            AnimProp::Opacity => d.opacity = value.clamp(0.0, 1.0),
            AnimProp::TranslateX => d.translate_x = value,
            AnimProp::TranslateY => d.translate_y = value,
            // Scale/Rotate/ZIndex don't map cleanly to a cell grid —
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
            // No gradients in ASCII — see [[project_aas_graphics_unsupported]]
            // for the equivalent posture on the AAS backend.
            _ => {}
        }
    }

    /// Called by the framework after every render pass. We don't run
    /// layout here — the host drives `render_to_grid` on its own
    /// schedule (after input + before paint) so we have the most
    /// current viewport size. `finish` would compute against stale
    /// dimensions if the terminal got resized between builds.
    fn finish(&mut self, _root: Self::Node) {}
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
        // We re-fetch content + style from the nodes map on every
        // measure call so text edits don't require re-installing the
        // measure_fn. Read through a `Weak`-shaped snapshot — we
        // can't borrow `&mut self` inside the closure, so we
        // capture the id and read from a thread-local… actually,
        // simpler: capture the current content by reading the field
        // each time via a small accessor on a clone of the data.
        //
        // Trick: we capture the content string by cloning it now,
        // but `update_text` swaps it AND calls `mark_dirty` which
        // forces a fresh measure that re-reads via the closure's
        // captured snapshot — broken.
        //
        // Instead, give the closure a clone of the nodes' Rc-shared
        // content cell. Cheapest path: store content in an
        // Rc<RefCell<String>>. But that ripples through NodeData.
        //
        // Pragmatic alternative: re-install the measure_fn on every
        // update_text. Cheap (one Rc clone) and avoids restructuring.
        let content = self
            .nodes
            .get(&id)
            .map(|d| d.content.clone())
            .unwrap_or_default();
        let f: native_layout::MeasureFn = Rc::new(move |known, avail| {
            measure_text(&content, known, avail)
        });
        self.layout.set_measure_fn(layout, f);
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
fn measure_text(
    content: &str,
    known: TaffySize<Option<f32>>,
    avail: TaffySize<AvailableSpace>,
) -> TaffySize<f32> {
    // Resolve the width constraint we'll wrap against.
    let max_w = match known.width {
        Some(w) => w,
        None => match avail.width {
            AvailableSpace::Definite(w) => w,
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
    TaffySize {
        width: known.width.unwrap_or(longest as f32),
        height: known.height.unwrap_or(lines as f32),
    }
}
