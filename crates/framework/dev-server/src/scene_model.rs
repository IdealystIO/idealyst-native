//! Live scene state mirror used to serialize the *current* UI tree
//! for catch-up replay, instead of resending the full append-only
//! command log.
//!
//! The original protocol shipped every wire command the framework
//! ever emitted to each freshly connecting client. That works for an
//! initial mount but it also makes the client re-experience every
//! transient state: open detail, close detail, type-and-delete in
//! a text field, scroll, etc. — all replayed in sequence on every
//! reconnect, producing visible flicker and unbounded memory growth.
//!
//! The scene model maintains the *current* state instead. Every
//! `Backend` method that mutates the scene also mutates the model.
//! On client connect, the recorder serializes the model into a
//! topologically-ordered list of wire `Command`s that, when replayed
//! from scratch, reproduce exactly what's on screen right now —
//! nothing more.
//!
//! Live updates after connect continue to flow through the recorder's
//! append-only log (`commands_since(cursor)`) so already-connected
//! clients receive incremental mutations as they happen.

use std::collections::HashMap;

use wire::{Command, HandlerId, NodeId, ScopeId, StyleId, WireScreenOptions, WireStyleRules};

/// All scene state needed to regenerate the wire command stream for
/// a fresh client. Updated alongside every recorder method that emits
/// a command — the model and the live log stay in lockstep, but the
/// model is the source of truth for catch-up.
#[derive(Default)]
pub struct SceneModel {
    /// Per-node create command, with current property values baked
    /// in. `update_*` methods mutate the relevant field in place so
    /// the snapshot always reflects the latest value (no need to
    /// emit `UpdateText`/`UpdateButtonLabel`/etc. during catch-up).
    pub node_create: HashMap<NodeId, Command>,
    /// Parent → ordered children. Maintained by Insert / InsertMany /
    /// ClearChildren. Used to (a) emit Insert commands during
    /// snapshot in declaration order, (b) walk reachable subtrees
    /// when pruning orphans.
    pub children: HashMap<NodeId, Vec<NodeId>>,
    /// Child → parent reverse lookup. Used during ClearChildren to
    /// drop the parent association, and when computing reachability.
    pub parent_of: HashMap<NodeId, NodeId>,
    /// Registered style rules by id. Populated by
    /// `register_stylesheet`; cleared by `unregister_stylesheet`.
    pub styles: HashMap<StyleId, WireStyleRules>,
    /// Per-node style application command. Stores the most recent
    /// `ApplyStyle` or `ApplyStyledStates`; cleared by
    /// `on_node_unstyled`.
    pub node_style: HashMap<NodeId, Command>,
    /// Nodes that have `AttachStates` wired up.
    pub node_attach_states: std::collections::HashSet<NodeId>,
    /// Per-node `SetDisabled` value (only stored when explicitly set).
    pub node_disabled: HashMap<NodeId, bool>,
    /// Per-node `ApplyPresence` command.
    pub node_presence: HashMap<NodeId, Command>,
    /// Per-icon-node stroke value from `UpdateIconStroke`. Snapshot
    /// emits the latest value so an icon that's been animated mid-
    /// session shows its current draw progress on reconnect.
    pub node_icon_stroke: HashMap<NodeId, f32>,
    /// Per-icon-node animation command. Snapshot emits this so
    /// running animations resume on reconnect. (Naive — we don't
    /// adjust `from` based on elapsed time. Acceptable for dev.)
    pub node_icon_anim: HashMap<NodeId, Command>,
    /// Per-overlay-node backdrop style application.
    pub overlay_backdrop_style: HashMap<NodeId, Command>,
    /// Per-navigator: ordered list of mounted screens. `stack[0]` is
    /// the initial route (emitted as `NavigatorAttachInitial`);
    /// `stack[1..]` are emitted as `NavigatorPush` in order. Mutated
    /// by attach-initial / push / pop / replace / reset.
    pub navigators: HashMap<NodeId, Vec<ScreenEntry>>,
    /// Per-navigator style-slot applications. Header / title / button
    /// / drawer-sidebar / etc. — stored as commands and replayed.
    pub nav_style_slots: HashMap<NodeId, NavStyleSlots>,
    /// Per-drawer-navigator sidebar attachment.
    pub drawer_sidebars: HashMap<NodeId, NodeId>,
    /// Root node from `Backend::finish(root)`. Anchors reachability
    /// — anything not in this subtree (and not a navigator screen)
    /// gets pruned during snapshot.
    pub root: Option<NodeId>,
    /// Latest color scheme. Live clients also see explicit
    /// `set_color_scheme` events, but a freshly connecting one needs
    /// to know the current value up front.
    pub color_scheme: Option<wire::WireColorScheme>,
}

/// One entry in a navigator's mounted-screen stack. The first entry
/// is the initial route's screen; subsequent entries are pushes.
#[derive(Clone)]
pub struct ScreenEntry {
    pub screen: NodeId,
    pub scope: ScopeId,
    pub options: WireScreenOptions,
    pub url: String,
}

/// Captures the seven style-slot Apply* commands a navigator can
/// receive. Each slot stores at most one command (last-write-wins).
#[derive(Default)]
pub struct NavStyleSlots {
    pub header: Option<Command>,
    pub title: Option<Command>,
    pub button: Option<Command>,
    pub drawer_sidebar: Option<Command>,
    pub drawer_scrim: Option<Command>,
    pub tab_bar: Option<Command>,
    pub tab_icon: Option<Command>,
    pub tab_label: Option<Command>,
}

impl SceneModel {
    pub fn new() -> Self {
        Self::default()
    }

    // -----------------------------------------------------------------
    // Node create
    // -----------------------------------------------------------------

    pub fn insert_create(&mut self, id: NodeId, cmd: Command) {
        self.node_create.insert(id, cmd);
    }

    // -----------------------------------------------------------------
    // Tree mutation
    // -----------------------------------------------------------------

    pub fn insert_child(&mut self, parent: NodeId, child: NodeId) {
        // Drop child's previous parent association if any. The
        // framework re-parents in some cases (e.g. anchored overlay
        // moves), and we mirror that.
        if let Some(prev_parent) = self.parent_of.remove(&child) {
            if let Some(siblings) = self.children.get_mut(&prev_parent) {
                siblings.retain(|c| *c != child);
            }
        }
        self.children.entry(parent).or_default().push(child);
        self.parent_of.insert(child, parent);
    }

    pub fn insert_children(&mut self, parent: NodeId, children: &[NodeId]) {
        for c in children {
            self.insert_child(parent, *c);
        }
    }

    pub fn clear_children(&mut self, node: NodeId) {
        if let Some(prev) = self.children.remove(&node) {
            for c in prev {
                self.parent_of.remove(&c);
            }
        }
    }

    // -----------------------------------------------------------------
    // Per-node mutating updates (mutate the stored Create command)
    // -----------------------------------------------------------------

    /// Apply a `UpdateText` to the stored `CreateText` so the snapshot
    /// reflects the current content. Silently no-ops if the node
    /// isn't a text node (shouldn't happen in practice — the framework
    /// only emits UpdateText against text nodes — but keep the model
    /// robust).
    pub fn update_text(&mut self, node: NodeId, content: &str) {
        if let Some(Command::CreateText { content: c, .. }) = self.node_create.get_mut(&node) {
            *c = content.to_string();
        }
    }

    pub fn update_button_label(&mut self, node: NodeId, label: &str) {
        if let Some(Command::CreateButton { label: l, .. }) = self.node_create.get_mut(&node) {
            *l = label.to_string();
        }
    }

    pub fn update_image_src(&mut self, node: NodeId, src: &str) {
        if let Some(Command::CreateImage { src: s, .. }) = self.node_create.get_mut(&node) {
            *s = src.to_string();
        }
    }

    pub fn update_icon_color(&mut self, node: NodeId, color: wire::WireColor) {
        if let Some(Command::CreateIcon { color: c, .. }) = self.node_create.get_mut(&node) {
            *c = Some(color);
        }
    }

    pub fn update_text_input_value(&mut self, node: NodeId, value: &str) {
        if let Some(Command::CreateTextInput { initial_value: v, .. }) =
            self.node_create.get_mut(&node)
        {
            *v = value.to_string();
        }
    }

    pub fn update_toggle_value(&mut self, node: NodeId, value: bool) {
        if let Some(Command::CreateToggle { initial_value: v, .. }) =
            self.node_create.get_mut(&node)
        {
            *v = value;
        }
    }

    pub fn update_slider_value(&mut self, node: NodeId, value: f32) {
        if let Some(Command::CreateSlider { initial_value: v, .. }) =
            self.node_create.get_mut(&node)
        {
            *v = value;
        }
    }

    pub fn update_web_view_url(&mut self, node: NodeId, url: &str) {
        if let Some(Command::CreateWebView { url: u, .. }) = self.node_create.get_mut(&node) {
            *u = url.to_string();
        }
    }

    pub fn update_video_src(&mut self, node: NodeId, src: &str) {
        if let Some(Command::CreateVideo { src: s, .. }) = self.node_create.get_mut(&node) {
            *s = src.to_string();
        }
    }

    pub fn set_icon_stroke(&mut self, node: NodeId, progress: f32) {
        self.node_icon_stroke.insert(node, progress);
    }

    pub fn set_icon_anim(&mut self, node: NodeId, cmd: Command) {
        self.node_icon_anim.insert(node, cmd);
    }

    // -----------------------------------------------------------------
    // Styles
    // -----------------------------------------------------------------

    pub fn register_style(&mut self, id: StyleId, rules: WireStyleRules) {
        self.styles.insert(id, rules);
    }

    pub fn unregister_style(&mut self, id: StyleId) {
        self.styles.remove(&id);
    }

    pub fn apply_style(&mut self, node: NodeId, cmd: Command) {
        self.node_style.insert(node, cmd);
    }

    pub fn clear_node_style(&mut self, node: NodeId) {
        self.node_style.remove(&node);
    }

    pub fn attach_states(&mut self, node: NodeId) {
        self.node_attach_states.insert(node);
    }

    pub fn set_disabled(&mut self, node: NodeId, disabled: bool) {
        self.node_disabled.insert(node, disabled);
    }

    pub fn apply_presence(&mut self, node: NodeId, cmd: Command) {
        self.node_presence.insert(node, cmd);
    }

    pub fn apply_overlay_backdrop_style(&mut self, node: NodeId, cmd: Command) {
        self.overlay_backdrop_style.insert(node, cmd);
    }

    // -----------------------------------------------------------------
    // Navigator stack management
    // -----------------------------------------------------------------

    pub fn navigator_attach_initial(&mut self, nav: NodeId, entry: ScreenEntry) {
        self.navigators.entry(nav).or_default().clear();
        self.navigators.entry(nav).or_default().push(entry);
    }

    pub fn navigator_push(&mut self, nav: NodeId, entry: ScreenEntry) {
        self.navigators.entry(nav).or_default().push(entry);
    }

    pub fn navigator_pop(&mut self, nav: NodeId, count: u32) {
        if let Some(stack) = self.navigators.get_mut(&nav) {
            for _ in 0..count {
                if stack.len() <= 1 {
                    break;
                }
                stack.pop();
            }
        }
    }

    /// `Replace` pops the top and pushes the new entry.
    pub fn navigator_replace(&mut self, nav: NodeId, entry: ScreenEntry) {
        let stack = self.navigators.entry(nav).or_default();
        if !stack.is_empty() {
            stack.pop();
        }
        stack.push(entry);
    }

    /// `Reset` collapses the stack to just the new entry, which now
    /// becomes the initial (replayed as `NavigatorAttachInitial`).
    pub fn navigator_reset(&mut self, nav: NodeId, entry: ScreenEntry) {
        let stack = self.navigators.entry(nav).or_default();
        stack.clear();
        stack.push(entry);
    }

    /// Pop the screen entry for a given scope (used by
    /// `handle_screen_released` for app-initiated swipe-back).
    /// Removes the entry from whatever position it's at — for stack
    /// navigators that's always the top, but the loop handles the
    /// general case.
    pub fn navigator_release_scope(&mut self, nav: NodeId, scope: ScopeId) {
        if let Some(stack) = self.navigators.get_mut(&nav) {
            stack.retain(|e| e.scope.0 != scope.0);
        }
    }

    // -----------------------------------------------------------------
    // Navigator style slots
    // -----------------------------------------------------------------

    pub fn set_nav_header_style(&mut self, nav: NodeId, cmd: Command) {
        self.nav_style_slots.entry(nav).or_default().header = Some(cmd);
    }
    pub fn set_nav_title_style(&mut self, nav: NodeId, cmd: Command) {
        self.nav_style_slots.entry(nav).or_default().title = Some(cmd);
    }
    pub fn set_nav_button_style(&mut self, nav: NodeId, cmd: Command) {
        self.nav_style_slots.entry(nav).or_default().button = Some(cmd);
    }
    pub fn set_drawer_sidebar_style(&mut self, nav: NodeId, cmd: Command) {
        self.nav_style_slots.entry(nav).or_default().drawer_sidebar = Some(cmd);
    }
    pub fn set_drawer_scrim_style(&mut self, nav: NodeId, cmd: Command) {
        self.nav_style_slots.entry(nav).or_default().drawer_scrim = Some(cmd);
    }
    pub fn set_tab_bar_style(&mut self, nav: NodeId, cmd: Command) {
        self.nav_style_slots.entry(nav).or_default().tab_bar = Some(cmd);
    }
    pub fn set_tab_icon_style(&mut self, nav: NodeId, cmd: Command) {
        self.nav_style_slots.entry(nav).or_default().tab_icon = Some(cmd);
    }
    pub fn set_tab_label_style(&mut self, nav: NodeId, cmd: Command) {
        self.nav_style_slots.entry(nav).or_default().tab_label = Some(cmd);
    }

    pub fn set_drawer_sidebar(&mut self, nav: NodeId, sidebar: NodeId) {
        self.drawer_sidebars.insert(nav, sidebar);
    }

    // -----------------------------------------------------------------
    // Snapshot serialization
    // -----------------------------------------------------------------

    /// Build a fresh command stream that, applied to an empty client
    /// from scratch, reproduces the current scene. Order matters:
    /// styles must be registered before they're applied; nodes must
    /// exist before they're inserted; child subtrees must be created
    /// before the parent that they'll be inserted into uses them in
    /// a navigator screen mount.
    ///
    /// We emit in this order:
    ///   1. `RegisterStyle` for every live style.
    ///   2. `Create*` for every node that's reachable.
    ///   3. `Insert` for every parent→child edge in tree order.
    ///   4. Per-node style applications, state attachment, disabled
    ///      flags, presence states, icon stroke / animation, overlay
    ///      backdrop styles.
    ///   5. Per-navigator: `NavigatorAttachInitial` + `NavigatorPush`
    ///      per stack entry, then style-slot commands, then drawer
    ///      sidebar attachment.
    ///   6. `Finish { root }` if a root was set.
    pub fn snapshot_commands(&self) -> Vec<Command> {
        let reachable = self.compute_reachable();
        let mut out = Vec::new();

        // 1. Styles.
        for (id, rules) in &self.styles {
            out.push(Command::RegisterStyle {
                id: *id,
                rules: rules.clone(),
            });
        }

        // 2. Node creates — only for reachable nodes. Order within
        // reachable doesn't matter for Create itself (each is
        // self-contained), but to keep ids deterministic for debug
        // we sort.
        let mut node_ids: Vec<NodeId> = self
            .node_create
            .keys()
            .filter(|id| reachable.contains(id))
            .copied()
            .collect();
        node_ids.sort_by_key(|n| n.0);
        for id in &node_ids {
            if let Some(cmd) = self.node_create.get(id) {
                out.push(cmd.clone());
            }
        }

        // 3. Insert edges, in tree order.
        let mut parent_ids: Vec<NodeId> =
            self.children.keys().copied().filter(|p| reachable.contains(p)).collect();
        parent_ids.sort_by_key(|n| n.0);
        for parent in parent_ids {
            if let Some(kids) = self.children.get(&parent) {
                for child in kids {
                    if reachable.contains(child) {
                        out.push(Command::Insert {
                            parent,
                            child: *child,
                        });
                    }
                }
            }
        }

        // 4a. Style applications.
        for id in &node_ids {
            if let Some(cmd) = self.node_style.get(id) {
                out.push(cmd.clone());
            }
        }
        // 4b. State attachment.
        for id in &node_ids {
            if self.node_attach_states.contains(id) {
                out.push(Command::AttachStates { node: *id });
            }
        }
        // 4c. Disabled.
        for id in &node_ids {
            if let Some(&d) = self.node_disabled.get(id) {
                out.push(Command::SetDisabled { node: *id, disabled: d });
            }
        }
        // 4d. Presence.
        for id in &node_ids {
            if let Some(cmd) = self.node_presence.get(id) {
                out.push(cmd.clone());
            }
        }
        // 4e. Icon stroke + animation.
        for id in &node_ids {
            if let Some(&p) = self.node_icon_stroke.get(id) {
                out.push(Command::UpdateIconStroke { node: *id, progress: p });
            }
            if let Some(cmd) = self.node_icon_anim.get(id) {
                out.push(cmd.clone());
            }
        }
        // 4f. Overlay backdrop style.
        for id in &node_ids {
            if let Some(cmd) = self.overlay_backdrop_style.get(id) {
                out.push(cmd.clone());
            }
        }

        // 5. Navigators.
        let mut nav_ids: Vec<NodeId> = self.navigators.keys().copied().collect();
        nav_ids.sort_by_key(|n| n.0);
        for nav_id in &nav_ids {
            let stack = match self.navigators.get(nav_id) {
                Some(s) => s,
                None => continue,
            };
            for (i, entry) in stack.iter().enumerate() {
                if i == 0 {
                    out.push(Command::NavigatorAttachInitial {
                        navigator: *nav_id,
                        screen: entry.screen,
                        scope: entry.scope,
                        options: entry.options.clone(),
                    });
                } else {
                    out.push(Command::NavigatorPush {
                        navigator: *nav_id,
                        screen: entry.screen,
                        scope: entry.scope,
                        options: entry.options.clone(),
                        url: entry.url.clone(),
                        restore: true,
                    });
                }
            }
            // Drawer sidebar attachment.
            if let Some(sidebar) = self.drawer_sidebars.get(nav_id) {
                out.push(Command::DrawerAttachSidebar {
                    navigator: *nav_id,
                    sidebar: *sidebar,
                });
            }
            // Style slots.
            if let Some(slots) = self.nav_style_slots.get(nav_id) {
                for slot in [
                    &slots.header,
                    &slots.title,
                    &slots.button,
                    &slots.drawer_sidebar,
                    &slots.drawer_scrim,
                    &slots.tab_bar,
                    &slots.tab_icon,
                    &slots.tab_label,
                ] {
                    if let Some(cmd) = slot {
                        out.push(cmd.clone());
                    }
                }
            }
        }

        // 6. Finish.
        if let Some(root) = self.root {
            out.push(Command::Finish { root });
        }

        out
    }

    /// Walk the live tree from the finish-root plus every navigator's
    /// stack of screens; everything not reachable is orphan and
    /// pruned from the snapshot. Orphans typically come from
    /// `NavigatorPop` (screen subtree no longer attached anywhere)
    /// or from screens that got created but never mounted (overlays
    /// that were dismissed, etc.).
    fn compute_reachable(&self) -> std::collections::HashSet<NodeId> {
        let mut reachable = std::collections::HashSet::new();
        let mut stack: Vec<NodeId> = Vec::new();

        if let Some(root) = self.root {
            stack.push(root);
        }
        for screens in self.navigators.values() {
            for entry in screens {
                stack.push(entry.screen);
            }
        }
        // The navigator nodes themselves: they live in node_create
        // and may or may not be reachable from the finish-root,
        // depending on whether they were inserted into a regular view
        // hierarchy or used as the host's direct mount. Be inclusive.
        for nav_id in self.navigators.keys() {
            stack.push(*nav_id);
        }
        // Drawer sidebars too — they're attached to the drawer
        // navigator but not via Insert.
        for sidebar in self.drawer_sidebars.values() {
            stack.push(*sidebar);
        }

        while let Some(id) = stack.pop() {
            if !reachable.insert(id) {
                continue;
            }
            if let Some(kids) = self.children.get(&id) {
                for c in kids {
                    if !reachable.contains(c) {
                        stack.push(*c);
                    }
                }
            }
        }
        reachable
    }
}

/// Helper for the recorder: assemble a `ScreenEntry` from the fields
/// already on hand at push/replace/reset/attach-initial sites.
pub fn screen_entry(
    screen: NodeId,
    scope_raw: u64,
    options: WireScreenOptions,
    url: String,
) -> ScreenEntry {
    ScreenEntry {
        screen,
        scope: ScopeId(scope_raw),
        options,
        url,
    }
}

// Silence unused-import warnings for things we expose but the
// recorder accesses via `crate::scene_model::SceneModel` paths.
#[allow(dead_code)]
fn _suppress_unused(_h: HandlerId) {}
