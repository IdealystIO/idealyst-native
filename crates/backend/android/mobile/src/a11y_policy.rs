//! What `AccessibilityProps` write to an `android.view.View`.
//!
//! [`A11yWrites::of`] resolves the props to the value each View setter
//! receives; `imp::a11y` performs the writes. Every value is written on an
//! update, so a cleared prop clears the view. A view being created needs no
//! write that leaves it as a fresh view already is
//! ([`A11yWrites::is_fresh_view_state`]): most views carry no accessibility
//! props at all, and the seven setters, each a JNI call, were most of the
//! cost of creating a label.
//!
//! Un-gated so the tests run on the host.

use runtime_shared::accessibility::{AccessibilityProps, AccessibilityTraits, CheckedState, LiveRegionPriority, Role};

/// `View.IMPORTANT_FOR_ACCESSIBILITY_AUTO` — let the platform decide.
/// We use this as the "no override" baseline so the platform's
/// heuristic for unlabelled containers (drop them from the tree)
/// continues to work.
pub(crate) const IMPORTANT_AUTO: i32 = 0;
/// `View.IMPORTANT_FOR_ACCESSIBILITY_YES` — force TalkBack focus
/// regardless of subtree content. We flip to this when the author
/// supplied a `label` or when the resolved role implies a focusable
/// element (Button, Link, Slider, …).
pub(crate) const IMPORTANT_YES: i32 = 1;
/// `View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS` — drop this
/// element AND every descendant from the a11y tree. The
/// "hide an entire subtree" semantics matches iOS
/// `accessibilityElementsHidden` and web `aria-hidden="true"`.
pub(crate) const IMPORTANT_NO_HIDE_DESCENDANTS: i32 = 4;

/// `View.ACCESSIBILITY_LIVE_REGION_NONE` (0) / `POLITE` (1) / `ASSERTIVE` (2).
/// Documented in the public Android SDK; the integer values are
/// stable across releases.
pub(crate) const LIVE_REGION_NONE: i32 = 0;
pub(crate) const LIVE_REGION_POLITE: i32 = 1;
pub(crate) const LIVE_REGION_ASSERTIVE: i32 = 2;

/// The value each accessibility setter receives for one view.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct A11yWrites {
    /// `setContentDescription`; `None` clears it.
    pub(crate) content_description: Option<String>,
    /// `setTooltipText` (API 26+); `None` clears it.
    pub(crate) tooltip: Option<String>,
    /// `setAccessibilityPaneTitle` (API 28+), the identifier; `None`
    /// clears it.
    pub(crate) pane_title: Option<String>,
    /// `setImportantForAccessibility`.
    pub(crate) important: i32,
    /// `setAccessibilityLiveRegion`.
    pub(crate) live_region: i32,
    /// `setAccessibilityHeading` (API 28+).
    pub(crate) heading: bool,
    /// `setSelected`.
    pub(crate) selected: bool,
    /// `setEnabled(false)` when set. Never written `true`: that would
    /// re-enable a view another layer disabled.
    pub(crate) disabled: bool,
}

impl A11yWrites {
    /// Resolve `props` for a view whose primitive's default role is
    /// `inferred_role`. An explicit `props.role` wins.
    pub(crate) fn of(props: &AccessibilityProps, inferred_role: Option<Role>) -> Self {
        let role = props.role.or(inferred_role);
        // Important-for-accessibility: hidden > labelled/role-forced > auto.
        let important = if props.hidden {
            IMPORTANT_NO_HIDE_DESCENDANTS
        } else if props.label.is_some() || role.map(role_forces_focus).unwrap_or(false) {
            IMPORTANT_YES
        } else {
            IMPORTANT_AUTO
        };
        let live_region = match props.live_region {
            Some(LiveRegionPriority::Polite) => LIVE_REGION_POLITE,
            Some(LiveRegionPriority::Assertive) => LIVE_REGION_ASSERTIVE,
            // UPDATES_FREQUENTLY acts as an implicit `polite` live region
            // when no explicit `live_region` is set — matches the web
            // backend's behavior.
            None if props.traits.contains(AccessibilityTraits::UPDATES_FREQUENTLY) => {
                LIVE_REGION_POLITE
            }
            None => LIVE_REGION_NONE,
        };
        Self {
            content_description: compose_label(props),
            tooltip: props.hint.clone(),
            pane_title: props.identifier.clone(),
            important,
            live_region,
            heading: matches!(role, Some(Role::Header)),
            selected: props.traits.contains(AccessibilityTraits::SELECTED),
            disabled: props.traits.contains(AccessibilityTraits::DISABLED),
        }
    }

    /// True when every write leaves a newly constructed `View` as it is:
    /// no description, tooltip or pane title, importance and live region
    /// at their defaults, not a heading, not selected, enabled. Not every
    /// widget constructs with those (`TextView` raises its importance), so
    /// `imp::a11y::apply_new` names the views it may skip.
    pub(crate) fn is_fresh_view_state(&self) -> bool {
        *self
            == Self {
                content_description: None,
                tooltip: None,
                pane_title: None,
                important: IMPORTANT_AUTO,
                live_region: LIVE_REGION_NONE,
                heading: false,
                selected: false,
                disabled: false,
            }
    }
}

/// Build the TalkBack-visible contentDescription: author label first,
/// then a comma-separated tail of state flags that Android has no
/// first-class setter for (CHECKED, EXPANDED, COLLAPSED, REQUIRED,
/// READONLY, INVALID, MIXED, BUSY). `None` means "no override" —
/// TalkBack falls back to the View's natural announcement.
///
/// Per-flag string keys are English literals for v1. Localization
/// belongs to the framework's i18n layer (not wired through to a11y
/// yet); when it lands, this function should pull the strings from
/// the host's resource table instead.
fn compose_label(props: &AccessibilityProps) -> Option<String> {
    match (props.label.clone(), state_tail(props)) {
        (None, None) => None,
        (Some(b), None) => Some(b),
        (None, Some(t)) => Some(t),
        (Some(b), Some(t)) => Some(format!("{b}, {t}")),
    }
}

/// Tail string assembled from state flags that don't have a
/// first-class Android View setter. Kept terse — TalkBack reads the
/// whole `contentDescription`, so verbose phrasing here would clog
/// the announcement.
fn state_tail(props: &AccessibilityProps) -> Option<String> {
    let traits = props.traits;
    let mut parts: Vec<&'static str> = Vec::new();
    match props.checked_state() {
        Some(CheckedState::Mixed) => parts.push("mixed"),
        Some(CheckedState::Checked) => parts.push("checked"),
        Some(CheckedState::Unchecked) => parts.push("not checked"),
        None => {}
    }
    if traits.contains(AccessibilityTraits::EXPANDED) {
        parts.push("expanded");
    } else if traits.contains(AccessibilityTraits::COLLAPSED) {
        parts.push("collapsed");
    }
    if traits.contains(AccessibilityTraits::BUSY) {
        parts.push("busy");
    }
    if traits.contains(AccessibilityTraits::REQUIRED) {
        parts.push("required");
    }
    if traits.contains(AccessibilityTraits::READONLY) {
        parts.push("read only");
    }
    if traits.contains(AccessibilityTraits::INVALID) {
        parts.push("invalid");
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

/// Does this role imply the element should be marked
/// `importantForAccessibility = YES` even without an explicit label?
/// Matches the iOS `role_forces_accessibility_element` policy — roles
/// that should *be* the focused element get forced into the AX tree.
fn role_forces_focus(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::Link
            | Role::NavigationLink
            | Role::Image
            | Role::Header
            | Role::SearchField
            | Role::Slider
            | Role::Switch
            | Role::Checkbox
            | Role::RadioButton
            | Role::Tab
            | Role::ProgressBar
            | Role::Spinner
            | Role::MenuItem
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_android_creating_a_plain_view_or_label_writes_no_accessibility_state() {
        // The bug: every created view paid seven accessibility setter JNI
        // calls that only restated a fresh view's defaults, about a third
        // of creating a label in a mounted list.
        for role in [None, Some(Role::Text), Some(Role::Group)] {
            assert!(
                A11yWrites::of(&AccessibilityProps::default(), role).is_fresh_view_state(),
                "{role:?}"
            );
        }
    }

    #[test]
    fn any_prop_that_changes_the_view_is_written_on_create() {
        let cases = [
            AccessibilityProps { label: Some("Save".into()), ..Default::default() },
            AccessibilityProps { hint: Some("Saves".into()), ..Default::default() },
            AccessibilityProps { identifier: Some("save".into()), ..Default::default() },
            AccessibilityProps { hidden: true, ..Default::default() },
            AccessibilityProps { live_region: Some(LiveRegionPriority::Polite), ..Default::default() },
            AccessibilityProps { traits: AccessibilityTraits::SELECTED, ..Default::default() },
            AccessibilityProps { traits: AccessibilityTraits::DISABLED, ..Default::default() },
            AccessibilityProps { traits: AccessibilityTraits::BUSY, ..Default::default() },
            AccessibilityProps { role: Some(Role::Header), ..Default::default() },
        ];
        for props in cases {
            assert!(!A11yWrites::of(&props, None).is_fresh_view_state(), "{props:?}");
        }
        // A role that forces focus (a button, an image) is written too.
        assert!(!A11yWrites::of(&AccessibilityProps::default(), Some(Role::Button)).is_fresh_view_state());
        assert!(!A11yWrites::of(&AccessibilityProps::default(), Some(Role::Image)).is_fresh_view_state());
    }

    #[test]
    fn state_tail_empty_traits_is_none() {
        assert!(state_tail(&AccessibilityProps::default()).is_none());
    }

    #[test]
    fn state_tail_checked_and_expanded() {
        let traits = AccessibilityTraits::CHECKED | AccessibilityTraits::EXPANDED;
        let props = AccessibilityProps { traits, ..Default::default() };
        assert_eq!(state_tail(&props).as_deref(), Some("checked, expanded"));
    }

    #[test]
    fn state_tail_mixed_supersedes_checked() {
        let traits = AccessibilityTraits::MIXED | AccessibilityTraits::CHECKED;
        let props = AccessibilityProps { traits, ..Default::default() };
        assert_eq!(state_tail(&props).as_deref(), Some("mixed"));
    }

    #[test]
    fn state_tail_says_not_checked_for_an_authored_checkable_role() {
        let props = AccessibilityProps { role: Some(Role::Checkbox), ..Default::default() };
        assert_eq!(state_tail(&props).as_deref(), Some("not checked"));
    }

    #[test]
    fn compose_label_combines_base_and_tail() {
        let props = AccessibilityProps {
            label: Some("Submit".into()),
            traits: AccessibilityTraits::BUSY,
            ..Default::default()
        };
        assert_eq!(compose_label(&props).as_deref(), Some("Submit, busy"));
    }

    #[test]
    fn compose_label_no_base_only_tail() {
        let props = AccessibilityProps { traits: AccessibilityTraits::REQUIRED, ..Default::default() };
        assert_eq!(compose_label(&props).as_deref(), Some("required"));
    }

    #[test]
    fn compose_label_default_props_is_none() {
        assert!(compose_label(&AccessibilityProps::default()).is_none());
    }

    #[test]
    fn role_forces_focus_button() {
        assert!(role_forces_focus(Role::Button));
        assert!(!role_forces_focus(Role::Text));
        assert!(!role_forces_focus(Role::Group));
    }
}
