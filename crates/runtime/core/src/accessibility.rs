//! Accessibility (a11y) surface — cross-platform shape that every
//! backend maps to its native AX system (UIAccessibility on iOS,
//! AccessibilityNodeInfo on Android, ARIA on web, NSAccessibility on
//! macOS, a parallel semantics tree on wgpu/GPU backends, no-op on
//! Roku).
//!
//! See [`docs/accessibility-design.md`](../../../docs/accessibility-design.md)
//! for the full design rationale, per-platform mapping tables, and
//! migration plan. This module is the foundation that the rest of
//! the framework builds on:
//!
//! - [`Role`] — semantic widget role (`Button`, `Slider`, …).
//! - [`AccessibilityTraits`] — orthogonal per-element state flags
//!   (`SELECTED`, `DISABLED`, …).
//! - [`AccessibilityProps`] — the shape passed to every `Backend::create_*`.
//! - [`AccessibilityAction`] — custom rotor-menu / TalkBack-action
//!   entries.
//! - [`AccessibilityTree`] / [`AccessibilityNode`] — the parallel
//!   semantics tree GPU backends produce.
//! - [`announce_for_accessibility`] (Backend trait method) — global
//!   live-region announcement hook.

use std::rc::Rc;

// ---------------------------------------------------------------------------
// Role — the cross-platform widget role taxonomy.
// ---------------------------------------------------------------------------

/// Semantic widget role. Every primitive has a default role; author
/// code overrides via [`AccessibilityProps::role`] only when the
/// primitive's visible shape differs from its a11y intent (a
/// `Pressable` styled as a navigation link sets `role: Some(NavigationLink)`).
///
/// Mappings to platform AX systems live in
/// [`docs/accessibility-design.md`](../../../docs/accessibility-design.md).
/// `#[non_exhaustive]` so adding a variant later isn't a breaking
/// change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Role {
    // Structural
    Button,
    Link,
    Image,
    Text,
    Header,
    List,
    ListItem,
    Group,
    Separator,

    // Input
    TextField,
    TextArea,
    Switch,
    Slider,
    Checkbox,
    RadioButton,
    RadioGroup,
    ComboBox,
    SearchField,

    // Disclosure / navigation
    Tab,
    TabList,
    TabPanel,
    NavigationLink,
    MenuItem,
    Menu,
    MenuBar,
    Toolbar,

    // Feedback
    Alert,
    Status,
    ProgressBar,
    Spinner,

    // Container / overlay
    Dialog,
    AlertDialog,
    Drawer,
    Popover,
    Tooltip,
    Region,
}

// ---------------------------------------------------------------------------
// AccessibilityTraits — orthogonal per-element state flags.
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    /// Per-element a11y state flags. Orthogonal to [`Role`]; compose
    /// freely (`SELECTED | DISABLED | EXPANDED`). Backends translate
    /// each flag to its platform's matching AX attribute — the
    /// observable behavior is "screen reader announces selected /
    /// disabled / expanded" on every platform.
    ///
    /// `u16` is intentional: each primitive carries an instance on
    /// every node, and 2 bytes vs. 12+ for a struct-of-bools adds up
    /// at thousands of nodes. The set is small and stable — adding a
    /// flag isn't a breaking change as long as we leave bits free.
    ///
    /// Flags excluded from this surface (folded into [`Role`] instead
    /// or rejected outright): `playsSound`, `startsMediaSession`,
    /// `causesPageTurn`. See `docs/accessibility-design.md` §2.
    #[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct AccessibilityTraits: u16 {
        /// Element is currently selected (tab, list row, segment).
        const SELECTED   = 1 << 0;
        /// Interactive element is non-interactive (greyed-out).
        const DISABLED   = 1 << 1;
        /// Disclosable element is currently expanded.
        const EXPANDED   = 1 << 2;
        /// Disclosable element is currently collapsed.
        const COLLAPSED  = 1 << 3;
        /// Checkbox / radio / toggle is checked.
        const CHECKED    = 1 << 4;
        /// Tri-state checkbox in indeterminate "some children checked".
        const MIXED      = 1 << 5;
        /// Element is performing async work; screen-reader UX hints
        /// at a loading state.
        const BUSY       = 1 << 6;
        /// Form field must be filled to submit.
        const REQUIRED   = 1 << 7;
        /// Interactive element doesn't accept user input changes
        /// (display-only).
        const READONLY   = 1 << 8;
        /// Form field's current value fails validation.
        const INVALID    = 1 << 9;
        /// Hint to platform AX that this element's value updates often
        /// enough that announcements should be coalesced
        /// (iOS `.updatesFrequently`, web `aria-live=polite`).
        const UPDATES_FREQUENTLY = 1 << 10;
    }
}

// ---------------------------------------------------------------------------
// LiveRegionPriority — how a backend should announce updates.
// ---------------------------------------------------------------------------

/// Priority for live-region announcements when an [`AccessibilityProps`]
/// label changes or when [`Backend::announce_for_accessibility`] is
/// called.
///
/// `Polite` queues behind the user's current screen-reader focus;
/// `Assertive` interrupts. Maps directly to ARIA `aria-live` values
/// and to per-platform announcement notification priority keys.
///
/// [`Backend::announce_for_accessibility`]: crate::Backend::announce_for_accessibility
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LiveRegionPriority {
    /// Queue the announcement behind any in-flight screen-reader
    /// speech. Use for non-critical status updates.
    Polite,
    /// Interrupt current speech and announce immediately. Use
    /// sparingly — for genuine alerts (form-submission failures,
    /// error toasts) that the user has to hear right now.
    Assertive,
}

// ---------------------------------------------------------------------------
// AccessibilityAction — custom rotor-menu / TalkBack actions.
// ---------------------------------------------------------------------------

/// Custom AX action attached to a primitive. Surfaces as a rotor entry
/// on VoiceOver, a TalkBack action in the per-element context menu, or
/// an ARIA-keyshortcuts widget on the web. Each entry is `(name,
/// handler)` — the framework's job is to dispatch the handler when
/// the user triggers the action via assistive technology.
///
/// Common uses: row-level "Delete" / "Archive" without showing visible
/// buttons; per-card "Open in new tab" / "Copy link"; per-message
/// "Reply" / "Flag".
#[derive(Clone)]
pub struct AccessibilityAction {
    /// Localized name shown in the rotor / context menu
    /// ("Delete", "Archive", "Show details").
    pub name: String,
    /// Fires when the user triggers the action via AT. Runs on the
    /// framework's reactive thread the same way a touch handler does;
    /// can synchronously update signals.
    pub handler: Rc<dyn Fn()>,
}

impl std::fmt::Debug for AccessibilityAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessibilityAction")
            .field("name", &self.name)
            .field("handler", &"<Rc<dyn Fn()>>")
            .finish()
    }
}

// ---------------------------------------------------------------------------
// AccessibilityProps — the shape passed to every `create_*`.
// ---------------------------------------------------------------------------

/// The accessibility prop bag attached to every primitive. Constructed
/// by the framework's builder layer; backends consume it inside
/// `create_*` and `update_accessibility`.
///
/// All fields are optional / defaulted: `AccessibilityProps::default()`
/// means "infer everything from the primitive type" (Backend reads the
/// primitive's default role, derives the label from visible content,
/// etc.). Author code overrides field-by-field.
///
/// The struct is `Clone` so the wgpu backend can own its own copy of
/// the props in its parallel semantics tree. Most backends pass
/// `&AccessibilityProps` through to per-attribute setter calls without
/// cloning.
#[derive(Clone, Debug, Default)]
pub struct AccessibilityProps {
    /// Author-supplied a11y label. `None` means "derive from the
    /// primitive's natural content" (Button's visible label, Text's
    /// content, Image's `alt`). Backends implement derivation; the
    /// walker doesn't pre-fill.
    ///
    /// Setting an explicit `Some(...)` opts the element into live-region
    /// announce-on-change semantics: if a reactive update changes this
    /// field, the backend re-announces via [`LiveRegionPriority`] from
    /// `live_region`. Visible-text changes do NOT auto-announce;
    /// author has to set an explicit label to opt in.
    pub label: Option<String>,

    /// Longer description ("Double tap to open menu", "Step 3 of 5").
    /// Maps to `accessibilityHint` (iOS/macOS), `aria-describedby`
    /// (web), the tail of `contentDescription` (Android).
    pub hint: Option<String>,

    /// Author override of the inferred role. `None` means "use the
    /// primitive's default role" — every primitive ships a default
    /// via [`default_role`]. Set this when the visible primitive's
    /// shape differs from its a11y intent (e.g. a `Pressable` styled
    /// as a navigation link sets `role: Some(NavigationLink)`).
    pub role: Option<Role>,

    /// Orthogonal state flags. Empty by default.
    pub traits: AccessibilityTraits,

    /// Hide from the accessibility tree entirely. Maps to
    /// `accessibilityElementsHidden` (iOS),
    /// `importantForAccessibility = NO_HIDE_DESCENDANTS` (Android),
    /// `aria-hidden="true"` (web), `isAccessibilityElement = false`
    /// (macOS). Use for purely-decorative content (background images,
    /// dividers).
    pub hidden: bool,

    /// Live-region priority. `None` = not a live region. Other values
    /// hint platform-AX to announce updates polite-ly or assertive-ly
    /// when this element's `label` changes.
    pub live_region: Option<LiveRegionPriority>,

    /// Custom AX actions exposed to assistive technology. Empty by
    /// default. Wired through to `accessibilityCustomActions`
    /// (iOS/macOS), Android's `addAction` slot, and web custom-widget
    /// dispatch.
    pub actions: Vec<AccessibilityAction>,

    /// Stable identifier for AX-driven external tooling (XCUITest's
    /// `accessibilityIdentifier`, web `id`, UIAutomator's resource
    /// name).
    ///
    /// **Separate from `test_id` for the Robot harness** — Robot ids
    /// are for the framework's in-process e2e harness; this
    /// `identifier` is the external AX hook visible to platform-AX
    /// tooling. The two serve different audiences and should not
    /// share a field.
    pub identifier: Option<String>,
}

impl AccessibilityProps {
    /// Returns true if every field is at its default value — backends
    /// can fast-path "no a11y overrides, infer everything" without
    /// reading individual fields.
    pub fn is_default(&self) -> bool {
        self.label.is_none()
            && self.hint.is_none()
            && self.role.is_none()
            && self.traits.is_empty()
            && !self.hidden
            && self.live_region.is_none()
            && self.actions.is_empty()
            && self.identifier.is_none()
    }
}

// ---------------------------------------------------------------------------
// AccessibilityTree — parallel semantics tree produced by GPU backends.
// ---------------------------------------------------------------------------

/// Parallel semantics tree returned by [`Backend::dump_accessibility_tree`]
/// — only GPU/canvas backends implement it. Native widget backends
/// (iOS, Android, web, macOS) return `None` because their a11y data
/// lives on the widget; the platform AX walker traverses the widget
/// tree directly.
///
/// The wgpu backend builds this tree alongside its visual scene so the
/// host shell (winit on macOS, the iOS shell crate, AT-SPI on Linux)
/// can project it into the platform AX layer once per layout commit.
///
/// Design inspired by Flutter's [`SemanticsNode`](https://api.flutter.dev/flutter/semantics/SemanticsNode-class.html)
/// (parallel semantics tree, traversal order distinct from paint
/// order). See `docs/accessibility-design.md` §5.
///
/// [`Backend::dump_accessibility_tree`]: crate::Backend::dump_accessibility_tree
#[derive(Clone, Debug)]
pub struct AccessibilityTree {
    pub root: AccessibilityNode,
}

/// One node in an [`AccessibilityTree`]. `children` are in traversal
/// order — top-to-bottom, left-to-right in LTR — distinct from paint
/// order so screen-reader navigation goes the user-meaningful way even
/// if z-ordering would suggest otherwise.
#[derive(Clone, Debug)]
pub struct AccessibilityNode {
    /// Stable id assigned by the backend. Host platform AX walker uses
    /// it as the AX element id (NSAccessibility children list, iOS
    /// `UIAccessibilityElement` array, AT-SPI object refs).
    pub id: u64,

    /// Resolved a11y props for this node — the backend has folded in
    /// any default-label derivation by the time the host reads this.
    pub props: AccessibilityProps,

    /// Default role inferred from the originating primitive. Author-
    /// supplied `props.role` has already taken precedence if set; this
    /// is the final value the host announces.
    pub role: Role,

    /// Bounds in the surface's coordinate space (device-independent
    /// pixels, origin top-left). The host converts to its platform's
    /// accessibility coordinate space.
    pub bounds: AccessibilityRect,

    /// Children in traversal order.
    pub children: Vec<AccessibilityNode>,
}

/// Rectangle in device-independent pixels, origin at top-left. Kept
/// local to this module to avoid coupling [`AccessibilityNode`] to a
/// graphics-coordinate type — backends and host shells convert to
/// their own coordinate spaces.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct AccessibilityRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

// ---------------------------------------------------------------------------
// PrimitiveKind + default_role table.
// ---------------------------------------------------------------------------

/// The set of primitive kinds the framework ships. Used by
/// [`default_role`] to map a primitive variant to its default a11y
/// role without coupling this module to `Element` (which lives in
/// a sibling module and depends on a lot of unrelated machinery).
///
/// Kept in lockstep with `crate::element::Element`'s variants —
/// see the `.claude/audits/accessibility-default-role.md` audit (added
/// in phase 8) which scans for new `Element` variants without
/// matching entries here. Control-flow variants (`When`, `Switch`,
/// `Repeat`) are intentionally absent: they're transparent containers
/// with no a11y identity of their own; the walker reads their inner
/// primitive's kind instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PrimitiveKind {
    View,
    Text,
    Button,
    Pressable,
    Image,
    Icon,
    Link,
    TextInput,
    TextArea,
    Toggle,
    Slider,
    ScrollView,
    ActivityIndicator,
    Virtualizer,
    Graphics,
    Portal,
    Presence,
    External,
    Navigator,
    Lazy,
}

/// Default a11y role inferred from the primitive type. Author-supplied
/// [`AccessibilityProps::role`] overrides. Backends call this via
/// `props.role.unwrap_or_else(|| default_role(kind))`.
///
/// Returns `None` for non-interactive structural primitives (View,
/// Group containers, Presence wrappers, ReactiveAnchor) — the backend
/// is expected to either drop them from the AX tree entirely or treat
/// them as transparent passthrough. Author code that needs an explicit
/// role on a container sets `props.role` explicitly.
pub fn default_role(kind: PrimitiveKind) -> Option<Role> {
    match kind {
        // Interactive
        PrimitiveKind::Button => Some(Role::Button),
        PrimitiveKind::Pressable => Some(Role::Button),
        PrimitiveKind::Link => Some(Role::Link),
        PrimitiveKind::TextInput => Some(Role::TextField),
        PrimitiveKind::TextArea => Some(Role::TextArea),
        PrimitiveKind::Toggle => Some(Role::Switch),
        PrimitiveKind::Slider => Some(Role::Slider),

        // Content
        PrimitiveKind::Text => Some(Role::Text),
        PrimitiveKind::Image => Some(Role::Image),
        PrimitiveKind::Icon => Some(Role::Image),

        // Feedback
        PrimitiveKind::ActivityIndicator => Some(Role::Spinner),

        // Container / virtualized
        PrimitiveKind::Virtualizer => Some(Role::List),
        PrimitiveKind::ScrollView => None, // transparent; per-platform scroll affordance lives on the OS chrome
        PrimitiveKind::Portal => None,     // transparent; the portal-mounted content carries its own role
        PrimitiveKind::Presence => None,   // transparent; wrapped subtree carries its role

        // Structural
        PrimitiveKind::View => None,
        PrimitiveKind::Graphics => None, // GPU-rendered content lives in dump_accessibility_tree
        PrimitiveKind::External => None, // third-party content sets its own role
        PrimitiveKind::Navigator => None, // navigator container is transparent; screens carry their own role
        PrimitiveKind::Lazy => None,      // transparent; the chunk's mounted root carries its own role
    }
}

/// Map a `Element` reference to its [`PrimitiveKind`]. Used by the
/// walker's a11y plumbing to look up the default role for the
/// primitive's variant without exposing every primitive's internal
/// structure to the accessibility module.
///
/// Returns `None` for control-flow primitives (`When`, `Switch`,
/// `Repeat`) which are transparent containers with no a11y identity
/// of their own — the walker reads the actually-mounted subtree's
/// kind instead.
pub fn primitive_kind(p: &crate::Element) -> Option<PrimitiveKind> {
    use crate::Element;
    match p {
        Element::View { .. } => Some(PrimitiveKind::View),
        Element::Text { .. } => Some(PrimitiveKind::Text),
        Element::Button { .. } => Some(PrimitiveKind::Button),
        Element::Pressable { .. } => Some(PrimitiveKind::Pressable),
        Element::Image { .. } => Some(PrimitiveKind::Image),
        Element::Icon { .. } => Some(PrimitiveKind::Icon),
        Element::Link { .. } => Some(PrimitiveKind::Link),
        Element::TextInput { .. } => Some(PrimitiveKind::TextInput),
        Element::TextArea { .. } => Some(PrimitiveKind::TextArea),
        Element::Toggle { .. } => Some(PrimitiveKind::Toggle),
        Element::Slider { .. } => Some(PrimitiveKind::Slider),
        Element::ScrollView { .. } => Some(PrimitiveKind::ScrollView),
        Element::ActivityIndicator { .. } => Some(PrimitiveKind::ActivityIndicator),
        Element::Virtualizer { .. } => Some(PrimitiveKind::Virtualizer),
        Element::Graphics { .. } => Some(PrimitiveKind::Graphics),
        Element::Portal { .. } => Some(PrimitiveKind::Portal),
        Element::Presence { .. } => Some(PrimitiveKind::Presence),
        Element::External { .. } => Some(PrimitiveKind::External),
        Element::Navigator { .. } => Some(PrimitiveKind::Navigator),
        Element::Lazy { .. } => Some(PrimitiveKind::Lazy),
        // Control flow — transparent.
        Element::When { .. }
        | Element::Switch { .. }
        | Element::Each { .. }
        | Element::Repeat { .. } => None,
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_props_is_default() {
        let p = AccessibilityProps::default();
        assert!(p.is_default());
    }

    #[test]
    fn props_with_label_is_not_default() {
        let p = AccessibilityProps {
            label: Some("Submit".into()),
            ..Default::default()
        };
        assert!(!p.is_default());
    }

    #[test]
    fn traits_compose() {
        let t = AccessibilityTraits::SELECTED | AccessibilityTraits::DISABLED;
        assert!(t.contains(AccessibilityTraits::SELECTED));
        assert!(t.contains(AccessibilityTraits::DISABLED));
        assert!(!t.contains(AccessibilityTraits::CHECKED));
    }

    #[test]
    fn traits_default_is_empty() {
        let t = AccessibilityTraits::default();
        assert!(t.is_empty());
    }

    #[test]
    fn default_role_inferences_match_design_doc() {
        // Spot-check the load-bearing entries from `docs/accessibility-design.md` §3.
        assert_eq!(default_role(PrimitiveKind::Button), Some(Role::Button));
        assert_eq!(default_role(PrimitiveKind::Pressable), Some(Role::Button));
        assert_eq!(default_role(PrimitiveKind::Toggle), Some(Role::Switch));
        assert_eq!(default_role(PrimitiveKind::Slider), Some(Role::Slider));
        assert_eq!(default_role(PrimitiveKind::TextInput), Some(Role::TextField));
        assert_eq!(default_role(PrimitiveKind::TextArea), Some(Role::TextArea));
        assert_eq!(default_role(PrimitiveKind::Link), Some(Role::Link));
        assert_eq!(default_role(PrimitiveKind::Virtualizer), Some(Role::List));

        // Containers / transparents.
        assert_eq!(default_role(PrimitiveKind::View), None);
        assert_eq!(default_role(PrimitiveKind::ScrollView), None);
        assert_eq!(default_role(PrimitiveKind::Portal), None);
        assert_eq!(default_role(PrimitiveKind::Presence), None);
    }

    #[test]
    fn live_region_priority_distinct() {
        // Sanity — the two priorities are distinguishable. Locked in
        // because backends pattern-match on these and a future
        // refactor that merges them would silently downgrade Assertive
        // to Polite on every backend.
        assert_ne!(LiveRegionPriority::Polite, LiveRegionPriority::Assertive);
    }

    #[test]
    fn role_is_copy_and_hashable() {
        // Required by the framework's resolution caches that key on
        // Role. If Role ever grows a non-Copy field, the caches need
        // updating — this test catches it at compile time.
        fn assert_copy<T: Copy>() {}
        fn assert_hash<T: std::hash::Hash + Eq>() {}
        assert_copy::<Role>();
        assert_hash::<Role>();
    }
}
