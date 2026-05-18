//! Live scene state mirror used to serialize the *current* UI tree
//! for catch-up replay, instead of resending the full append-only
//! command log.
//!
//! The original protocol shipped every wire command the framework
//! ever emitted to each freshly connecting client. That works for an
//! initial mount but it also makes the client re-experience every
//! transient state: open detail, close detail, type-and-delete in a
//! text field, scroll, etc. — all replayed in sequence on every
//! reconnect, producing visible flicker and unbounded memory growth.
//!
//! The scene model maintains the *current* state instead. Every
//! `Backend` method on the recorder calls [`SceneModel::apply`]
//! with the command it just emitted; the model interprets the
//! command and updates its mirror. On client connect, the recorder
//! serializes the model into a topologically-ordered list of wire
//! `Command`s that, when replayed from scratch, reproduce exactly
//! what's on screen right now — nothing more.
//!
//! Live updates after connect continue to flow through the recorder's
//! append-only log (`commands_since(cursor)`) so already-connected
//! clients receive incremental mutations as they happen.

use std::collections::{HashMap, HashSet};

use wire::{Command, NodeId, ScopeId, StyleId, WireScreenOptions, WireStyleRules};

/// All scene state needed to regenerate the wire command stream for
/// a fresh client.
#[derive(Default)]
pub struct SceneModel {
    /// Per-node create command, with current property values baked
    /// in. `Update*` commands mutate the corresponding field of the
    /// stored Create in place so the snapshot always reflects the
    /// latest value (no need to also replay UpdateText / etc.).
    node_create: HashMap<NodeId, Command>,
    /// Parent → ordered children. Maintained by Insert, InsertMany,
    /// ClearChildren. Used to (a) emit Insert commands during
    /// snapshot in declaration order and (b) walk reachable subtrees.
    children: HashMap<NodeId, Vec<NodeId>>,
    /// Child → parent reverse lookup.
    parent_of: HashMap<NodeId, NodeId>,
    /// Registered style rules by id.
    styles: HashMap<StyleId, WireStyleRules>,
    /// Per-node style application command (`ApplyStyle` or
    /// `ApplyStyledStates`). Cleared by `OnNodeUnstyled`.
    node_style: HashMap<NodeId, Command>,
    /// Nodes with `AttachStates` wired up.
    node_attach_states: HashSet<NodeId>,
    /// Per-node `SetDisabled` value.
    node_disabled: HashMap<NodeId, bool>,
    /// Per-node `ApplyPresence` command.
    node_presence: HashMap<NodeId, Command>,
    /// Per-icon-node static stroke progress.
    node_icon_stroke: HashMap<NodeId, f32>,
    /// Per-icon-node animation command. Naive snapshot replay (we
    /// don't adjust `from` based on elapsed time); acceptable for
    /// dev.
    node_icon_anim: HashMap<NodeId, Command>,
    /// Per-overlay-node backdrop style.
    overlay_backdrop_style: HashMap<NodeId, Command>,
    /// Per-navigator: ordered list of mounted screens. `stack[0]` is
    /// the initial route (emitted as `NavigatorAttachInitial`);
    /// `stack[1..]` emit as `NavigatorPush`.
    navigators: HashMap<NodeId, Vec<ScreenEntry>>,
    /// Reverse lookup: scope_id → navigator that owns it. Used by
    /// `release_scope` (app-initiated swipe-back) to find which
    /// navigator's stack to mutate.
    scope_to_navigator: HashMap<u64, NodeId>,
    /// Per-navigator style-slot applications. Header / title /
    /// button / drawer-sidebar / etc.
    nav_style_slots: HashMap<NodeId, NavStyleSlots>,
    /// Per-drawer-navigator sidebar attachment.
    drawer_sidebars: HashMap<NodeId, NodeId>,
    /// Root node from `Backend::finish(root)`. Anchors reachability
    /// — anything not in this subtree (and not a navigator screen)
    /// gets pruned during snapshot.
    root: Option<NodeId>,
}

/// One entry in a navigator's mounted-screen stack.
#[derive(Clone)]
pub struct ScreenEntry {
    pub screen: NodeId,
    pub scope: ScopeId,
    pub options: WireScreenOptions,
    pub url: String,
}

/// Captures the eight style-slot Apply* commands a navigator can
/// receive. Each slot stores at most one command (last-write-wins).
#[derive(Default)]
struct NavStyleSlots {
    header: Option<Command>,
    title: Option<Command>,
    button: Option<Command>,
    drawer_sidebar: Option<Command>,
    drawer_scrim: Option<Command>,
    tab_bar: Option<Command>,
    tab_icon: Option<Command>,
    tab_label: Option<Command>,
}

impl SceneModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Single entry point for the recorder: interpret a freshly
    /// emitted wire `Command` and mutate the model to reflect it.
    /// This mirrors the same state the iOS / web client will reach
    /// when they apply the same command, so on snapshot we can
    /// re-derive the equivalent command stream.
    pub fn apply(&mut self, cmd: &Command) {
        match cmd {
            // -- Create commands. Stored verbatim; later Update*
            //    commands mutate the stored copy's relevant field.
            Command::CreateView { id } => {
                self.node_create.insert(*id, cmd.clone());
            }
            Command::CreateText { id, .. }
            | Command::CreateButton { id, .. }
            | Command::CreatePressable { id, .. }
            | Command::CreateReactiveAnchor { id }
            | Command::CreateImage { id, .. }
            | Command::CreateIcon { id, .. }
            | Command::CreateTextInput { id, .. }
            | Command::CreateToggle { id, .. }
            | Command::CreateSlider { id, .. }
            | Command::CreateScrollView { id, .. }
            | Command::CreateWebView { id, .. }
            | Command::CreateVideo { id, .. }
            | Command::CreateActivityIndicator { id, .. }
            | Command::CreateLink { id, .. }
            | Command::CreateOverlay { id, .. }
            | Command::CreateGraphics { id, .. }
            | Command::CreateVirtualizer { id, .. } => {
                self.node_create.insert(*id, cmd.clone());
            }
            Command::CreateNavigator { id, .. }
            | Command::CreateTabNavigator { id, .. }
            | Command::CreateDrawerNavigator { id, .. } => {
                self.node_create.insert(*id, cmd.clone());
                self.navigators.entry(*id).or_default();
            }

            // -- Tree mutation.
            Command::Insert { parent, child } => {
                self.insert_child_internal(*parent, *child);
            }
            Command::InsertMany { parent, children } => {
                for c in children {
                    self.insert_child_internal(*parent, *c);
                }
            }
            Command::ClearChildren { node } => {
                if let Some(prev) = self.children.remove(node) {
                    for c in prev {
                        self.parent_of.remove(&c);
                    }
                }
            }

            // -- Updates: mutate the stored Create command's field.
            Command::UpdateText { node, content } => {
                if let Some(Command::CreateText { content: c, .. }) =
                    self.node_create.get_mut(node)
                {
                    *c = content.clone();
                }
            }
            Command::UpdateButtonLabel { node, label } => {
                if let Some(Command::CreateButton { label: l, .. }) =
                    self.node_create.get_mut(node)
                {
                    *l = label.clone();
                }
            }
            Command::UpdateImageSrc { node, src } => {
                if let Some(Command::CreateImage { src: s, .. }) =
                    self.node_create.get_mut(node)
                {
                    *s = src.clone();
                }
            }
            Command::UpdateIconColor { node, color } => {
                if let Some(Command::CreateIcon { color: c, .. }) =
                    self.node_create.get_mut(node)
                {
                    *c = Some(color.clone());
                }
            }
            Command::UpdateIconStroke { node, progress } => {
                self.node_icon_stroke.insert(*node, *progress);
            }
            Command::AnimateIconStroke { node, .. } => {
                self.node_icon_anim.insert(*node, cmd.clone());
            }
            Command::UpdateTextInputValue { node, value } => {
                if let Some(Command::CreateTextInput { initial_value: v, .. }) =
                    self.node_create.get_mut(node)
                {
                    *v = value.clone();
                }
            }
            Command::UpdateToggleValue { node, value } => {
                if let Some(Command::CreateToggle { initial_value: v, .. }) =
                    self.node_create.get_mut(node)
                {
                    *v = *value;
                }
            }
            Command::UpdateSliderValue { node, value } => {
                if let Some(Command::CreateSlider { initial_value: v, .. }) =
                    self.node_create.get_mut(node)
                {
                    *v = *value;
                }
            }
            Command::UpdateWebViewUrl { node, url } => {
                if let Some(Command::CreateWebView { url: u, .. }) =
                    self.node_create.get_mut(node)
                {
                    *u = url.clone();
                }
            }
            Command::UpdateVideoSrc { node, src } => {
                if let Some(Command::CreateVideo { src: s, .. }) =
                    self.node_create.get_mut(node)
                {
                    *s = src.clone();
                }
            }
            Command::SetDisabled { node, disabled } => {
                self.node_disabled.insert(*node, *disabled);
            }

            // -- Styles.
            Command::RegisterStyle { id, rules } => {
                self.styles.insert(*id, rules.clone());
            }
            Command::UnregisterStyle { id } => {
                self.styles.remove(id);
            }
            Command::ApplyStyle { node, .. }
            | Command::ApplyStyledStates { node, .. } => {
                self.node_style.insert(*node, cmd.clone());
            }
            Command::AttachStates { node } => {
                self.node_attach_states.insert(*node);
            }
            Command::OnNodeUnstyled { node } => {
                self.node_style.remove(node);
            }

            // -- Presence.
            Command::ApplyPresence { node, .. } => {
                self.node_presence.insert(*node, cmd.clone());
            }

            // -- Navigator control plane.
            Command::NavigatorAttachInitial {
                navigator,
                screen,
                scope,
                options,
            } => {
                let entry = ScreenEntry {
                    screen: *screen,
                    scope: *scope,
                    options: options.clone(),
                    url: String::new(),
                };
                let stack = self.navigators.entry(*navigator).or_default();
                stack.clear();
                stack.push(entry);
                self.scope_to_navigator.insert(scope.0, *navigator);
            }
            Command::NavigatorPush {
                navigator,
                screen,
                scope,
                options,
                url,
                ..
            } => {
                let entry = ScreenEntry {
                    screen: *screen,
                    scope: *scope,
                    options: options.clone(),
                    url: url.clone(),
                };
                self.navigators.entry(*navigator).or_default().push(entry);
                self.scope_to_navigator.insert(scope.0, *navigator);
            }
            Command::NavigatorPop { navigator, count } => {
                if let Some(stack) = self.navigators.get_mut(navigator) {
                    for _ in 0..*count {
                        if stack.len() <= 1 {
                            break;
                        }
                        if let Some(popped) = stack.pop() {
                            self.scope_to_navigator.remove(&popped.scope.0);
                        }
                    }
                }
            }
            Command::NavigatorReplace {
                navigator,
                screen,
                scope,
                options,
                url,
                ..
            } => {
                let entry = ScreenEntry {
                    screen: *screen,
                    scope: *scope,
                    options: options.clone(),
                    url: url.clone(),
                };
                let stack = self.navigators.entry(*navigator).or_default();
                if let Some(prev) = stack.pop() {
                    self.scope_to_navigator.remove(&prev.scope.0);
                }
                stack.push(entry);
                self.scope_to_navigator.insert(scope.0, *navigator);
            }
            Command::NavigatorReset {
                navigator,
                screen,
                scope,
                options,
                url,
                ..
            } => {
                let entry = ScreenEntry {
                    screen: *screen,
                    scope: *scope,
                    options: options.clone(),
                    url: url.clone(),
                };
                let stack = self.navigators.entry(*navigator).or_default();
                for prev in stack.drain(..) {
                    self.scope_to_navigator.remove(&prev.scope.0);
                }
                stack.push(entry);
                self.scope_to_navigator.insert(scope.0, *navigator);
            }
            Command::NavigatorMountTab { .. } => {
                // Tab mounting is currently surfaced as a Push when
                // replayed by the AAS client; the live broadcast
                // handles the mount itself. No model state to track
                // for the demo, and tab navigators don't appear in
                // the current example. Revisit when adding tabs.
            }
            Command::DrawerAttachSidebar { navigator, sidebar } => {
                self.drawer_sidebars.insert(*navigator, *sidebar);
            }
            Command::OpenDrawer { .. }
            | Command::CloseDrawer { .. }
            | Command::ToggleDrawer { .. } => {
                // Drawer open-state is broadcast live; not part of
                // the persistent snapshot. The client's drawer
                // defaults to closed on fresh mount.
            }
            Command::ApplyNavigatorHeaderStyle { navigator, .. } => {
                self.nav_style_slots.entry(*navigator).or_default().header = Some(cmd.clone());
            }
            Command::ApplyNavigatorTitleStyle { navigator, .. } => {
                self.nav_style_slots.entry(*navigator).or_default().title = Some(cmd.clone());
            }
            Command::ApplyNavigatorButtonStyle { navigator, .. } => {
                self.nav_style_slots.entry(*navigator).or_default().button = Some(cmd.clone());
            }
            Command::ApplyDrawerSidebarStyle { navigator, .. } => {
                self.nav_style_slots.entry(*navigator).or_default().drawer_sidebar =
                    Some(cmd.clone());
            }
            Command::ApplyDrawerScrimStyle { navigator, .. } => {
                self.nav_style_slots.entry(*navigator).or_default().drawer_scrim =
                    Some(cmd.clone());
            }
            Command::ApplyTabBarStyle { navigator, .. } => {
                self.nav_style_slots.entry(*navigator).or_default().tab_bar = Some(cmd.clone());
            }
            Command::ApplyTabIconStyle { navigator, .. } => {
                self.nav_style_slots.entry(*navigator).or_default().tab_icon = Some(cmd.clone());
            }
            Command::ApplyTabLabelStyle { navigator, .. } => {
                self.nav_style_slots.entry(*navigator).or_default().tab_label = Some(cmd.clone());
            }

            Command::ApplyOverlayBackdropStyle { node, .. } => {
                self.overlay_backdrop_style.insert(*node, cmd.clone());
            }

            Command::VirtualizerDataChanged { .. }
            | Command::VirtualizerAttachItem { .. } => {
                // Virtualizer items are managed live; for a fresh
                // snapshot the client will request items as scroll
                // produces visible indices.
            }

            Command::Finish { root } => {
                self.root = Some(*root);
            }
            Command::ReleaseNode { node } => {
                self.node_create.remove(node);
                self.children.remove(node);
                self.parent_of.remove(node);
                self.node_style.remove(node);
                self.node_attach_states.remove(node);
                self.node_disabled.remove(node);
                self.node_presence.remove(node);
                self.node_icon_stroke.remove(node);
                self.node_icon_anim.remove(node);
                self.overlay_backdrop_style.remove(node);
            }
            Command::InstallThemeVariables { .. } => {
                // Theme variables are broadcast live; not modeled
                // here. Acceptable for dev — themes rarely change
                // mid-session.
            }
        }
    }

    fn insert_child_internal(&mut self, parent: NodeId, child: NodeId) {
        // Re-parent: drop the child's previous parent association so
        // it doesn't appear twice in the snapshot's Insert sequence.
        if let Some(prev_parent) = self.parent_of.remove(&child) {
            if let Some(siblings) = self.children.get_mut(&prev_parent) {
                siblings.retain(|c| *c != child);
            }
        }
        self.children.entry(parent).or_default().push(child);
        self.parent_of.insert(child, parent);
    }

    // -----------------------------------------------------------------
    // Snapshot serialization
    // -----------------------------------------------------------------

    /// Build a fresh command stream that, applied to an empty client
    /// from scratch, reproduces the current scene. Order matters:
    /// styles must be registered before they're applied; nodes must
    /// exist before they're inserted or referenced as a navigator
    /// screen.
    ///
    /// Emit order:
    ///   1. `RegisterStyle` for every live style.
    ///   2. `Create*` for every reachable node.
    ///   3. `Insert` for every parent→child edge.
    ///   4. Per-node style applications, state attach, disabled,
    ///      presence, icon stroke / animation, overlay backdrop
    ///      style.
    ///   5. Per-navigator: `NavigatorAttachInitial` + `NavigatorPush`,
    ///      then drawer sidebar attachment, then style-slot
    ///      applications.
    ///   6. `Finish { root }` if a root was set.
    pub fn snapshot_commands(&self) -> Vec<Command> {
        let reachable = self.compute_reachable();
        let mut out = Vec::new();

        // 1. Styles.
        let mut style_ids: Vec<StyleId> = self.styles.keys().copied().collect();
        style_ids.sort_by_key(|s| s.0);
        for id in style_ids {
            if let Some(rules) = self.styles.get(&id) {
                out.push(Command::RegisterStyle {
                    id,
                    rules: rules.clone(),
                });
            }
        }

        // 2. Node creates. Sort for determinism.
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

        // 3. Insert edges. Iterate parents in node-id order; emit
        //    each parent's children in original Insert order.
        for parent in &node_ids {
            if let Some(kids) = self.children.get(parent) {
                for child in kids {
                    if reachable.contains(child) {
                        out.push(Command::Insert {
                            parent: *parent,
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
        // 4b. State attach.
        for id in &node_ids {
            if self.node_attach_states.contains(id) {
                out.push(Command::AttachStates { node: *id });
            }
        }
        // 4c. Disabled.
        for id in &node_ids {
            if let Some(&d) = self.node_disabled.get(id) {
                out.push(Command::SetDisabled {
                    node: *id,
                    disabled: d,
                });
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
                out.push(Command::UpdateIconStroke {
                    node: *id,
                    progress: p,
                });
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
            let Some(stack) = self.navigators.get(nav_id) else {
                continue;
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
                        // restore=true signals to web backends not to
                        // touch `history.pushState` (the browser
                        // already has these URLs from its own
                        // history). Native backends ignore.
                        restore: true,
                    });
                }
            }
            if let Some(sidebar) = self.drawer_sidebars.get(nav_id) {
                out.push(Command::DrawerAttachSidebar {
                    navigator: *nav_id,
                    sidebar: *sidebar,
                });
            }
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
    /// stack of screens; everything not reachable is pruned from the
    /// snapshot. Orphans typically come from popped screens or
    /// dismissed overlays.
    fn compute_reachable(&self) -> HashSet<NodeId> {
        let mut reachable = HashSet::new();
        let mut stack: Vec<NodeId> = Vec::new();

        if let Some(root) = self.root {
            stack.push(root);
        }
        // Navigators themselves and their currently-mounted screens
        // are always reachable, even if they aren't connected via
        // `Insert` to the finish-root (e.g. host-attached
        // navigators on native).
        for (nav_id, screens) in &self.navigators {
            stack.push(*nav_id);
            for entry in screens {
                stack.push(entry.screen);
            }
        }
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
