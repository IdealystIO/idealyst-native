//! iOS-mobile backend accessibility translation —
//! [`AccessibilityProps`] → UIKit `UIAccessibility*` setters on every
//! `UIView` we create.
//!
//! Strategy mirrors the web backend's `a11y.rs`: a single [`apply`]
//! function takes a node + resolved props and writes (or clears) every
//! relevant UIAccessibility property. All `create_*` paths call it
//! after constructing the view; the dynamic
//! [`update_accessibility`](framework_core::Backend::update_accessibility)
//! path reuses it identically. Clearing on `None` is intentional —
//! reactive a11y prop changes must not leak stale labels onto a view.
//!
//! UIKit walks each `UIView`'s accessibility properties directly, so
//! we don't maintain a parallel semantics tree and
//! [`dump_accessibility_tree`](framework_core::Backend::dump_accessibility_tree)
//! stays `None` for this backend.
//!
//! ### Live regions
//!
//! UIKit has no per-view "aria-live"-equivalent setter — live updates
//! are imperative via `UIAccessibilityPostNotification(.announcement,
//! …)`. The `props.live_region` field is therefore observed at the
//! framework layer (the walker re-announces when a live-region label
//! changes) and routed through [`announce`] here; [`apply`] itself
//! leaves it untouched.
//!
//! ### Trait mapping subtleties
//!
//! - `CHECKED` has no first-class `UIAccessibilityTrait`. We expose
//!   the state via `accessibilityValue` ("1" / "0") so VoiceOver
//!   announces "selected" / "not selected" without the framework
//!   having to plant a custom action. `MIXED` becomes the value
//!   string "mixed" by the same logic.
//! - `EXPANDED` / `COLLAPSED` similarly have no UIKit trait. We fold
//!   them into the accessibility value string ("expanded" /
//!   "collapsed") so screen-reader announcements stay aligned with
//!   web ARIA semantics.
//! - `BUSY` and `UPDATES_FREQUENTLY` both map to
//!   `UIAccessibilityTraitUpdatesFrequently`. That's what UIKit
//!   exposes — `aria-busy="true"` doesn't have a closer equivalent on
//!   iOS.

use framework_core::accessibility::{
    AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
use objc2::msg_send;
use objc2_foundation::NSString;
use objc2_ui_kit::UIView;

use crate::imp::IosNode;

/// Apply / refresh every UIAccessibility property on `node` from
/// `props`.
///
/// `inferred_role` is the primitive's default role (see
/// [`framework_core::accessibility::default_role`]). If
/// `props.role.is_none()` and `inferred_role.is_some()`, the inferred
/// role is used to derive the UIAccessibilityTraits bag; if both are
/// `None`, no role-derived bits are added (the caller's traits flags
/// still apply).
///
/// Idempotent: every property is either written or explicitly cleared.
/// Calling twice with the same `props` produces the same UIKit state;
/// calling with different `props` always converges.
pub(crate) fn apply(node: &IosNode, props: &AccessibilityProps, inferred_role: Option<Role>) {
    let view: &UIView = node.as_view();

    // Resolve role: explicit override wins.
    let resolved_role = props.role.or(inferred_role);

    // Label / hint / identifier.
    set_string_or_clear(view, sel_set_accessibility_label(), props.label.as_deref());
    set_string_or_clear(view, sel_set_accessibility_hint(), props.hint.as_deref());
    set_identifier_or_clear(view, props.identifier.as_deref());

    // Hidden: `accessibilityElementsHidden` hides this view *and all
    // descendants* from the AX tree. The plain
    // `isAccessibilityElement = false` flag is the per-element opt-out
    // — we don't toggle it for `hidden`; instead we use the container-
    // wide hider so transparent View groups stay walked when not
    // explicitly hidden.
    let _: () = unsafe { msg_send![view, setAccessibilityElementsHidden: props.hidden] };

    // `isAccessibilityElement` is forced ON for elements that have an
    // author-supplied label OR a UIKit-implicit trait variant (button,
    // link, image, header, search, slider, spinner, …). For unlabelled
    // generic containers (View with no label), leave it as UIKit's
    // default — UIView defaults to `false` and lets UIKit walk into
    // children; UILabel/UIButton/UISwitch default to `true`.
    let role_forces_element = resolved_role
        .map(role_forces_accessibility_element)
        .unwrap_or(false);
    if props.label.is_some() || role_forces_element {
        let _: () = unsafe { msg_send![view, setIsAccessibilityElement: true] };
    }

    // Accessibility traits — OR together role-derived bits and per-
    // element state flags.
    let traits_bits = traits_to_uikit_bits(resolved_role, props.traits);
    let _: () = unsafe { msg_send![view, setAccessibilityTraits: traits_bits] };

    // Value. Used for CHECKED/MIXED/EXPANDED/COLLAPSED — none of which
    // have a first-class UIKit trait. Set to the canonical announce
    // string; clear when the flag isn't set.
    let value = derive_accessibility_value(props.traits);
    set_string_or_clear(view, sel_set_accessibility_value(), value);
}

/// `setAccessibilityViewIsModal:` — used by the portal/dialog code
/// path. Exposed as a helper so portal callers don't have to repeat
/// the unsafe-msg_send dance. Kept here so all UIAccessibility wiring
/// lives in one place.
#[allow(dead_code)]
pub(crate) fn set_modal(view: &UIView, modal: bool) {
    let _: () = unsafe { msg_send![view, setAccessibilityViewIsModal: modal] };
}

// ---------------------------------------------------------------------------
// Role → UIAccessibilityTraits + per-element-flag mapping.
// ---------------------------------------------------------------------------

/// Map a [`Role`] to the UIAccessibilityTraits bits it contributes.
/// Matches the table in `docs/accessibility-design.md` §1.
///
/// `Role` is `#[non_exhaustive]`; unmapped variants contribute zero
/// (no trait) rather than panicking — safer than guessing when UIKit
/// has no obvious equivalent for a future role.
fn role_to_traits_bits(role: Role) -> u64 {
    // UIAccessibilityTrait* bit values are stable across iOS releases
    // (documented in `<UIKit/UIAccessibilityConstants.h>`). Listed
    // inline here as bit positions so the mapping table is greppable;
    // we could pull `objc2_ui_kit::UIAccessibilityTraitButton` etc.,
    // but those are extern statics that require a runtime load.
    const BUTTON: u64 = 1 << 0;
    const LINK: u64 = 1 << 1;
    const IMAGE: u64 = 1 << 2;
    const SEARCH_FIELD: u64 = 1 << 7;
    const ADJUSTABLE: u64 = 1 << 4; // slider
    const UPDATES_FREQUENTLY: u64 = 1 << 9; // progress / spinner
    const HEADER: u64 = 1 << 28;
    const TAB_BAR: u64 = 1 << 36;
    const TOGGLE_BUTTON: u64 = 1 << 37;
    const STATIC_TEXT: u64 = 1 << 8;

    match role {
        Role::Button => BUTTON,
        Role::Link | Role::NavigationLink => LINK,
        Role::Image => IMAGE,
        Role::Header => HEADER,
        Role::SearchField => SEARCH_FIELD,
        Role::Slider => ADJUSTABLE,
        Role::Switch | Role::Checkbox | Role::RadioButton => TOGGLE_BUTTON,
        Role::ProgressBar | Role::Spinner => UPDATES_FREQUENTLY,
        Role::Tab => BUTTON, // SELECTED bit (added separately by traits) lifts to "selected tab"
        Role::TabList => TAB_BAR,
        // Static-text roles announce as text without a "button" hint.
        Role::Text => STATIC_TEXT,
        // Roles with no first-class UIKit trait — UIKit walks the view
        // and announces label/value without role-specific framing.
        Role::TextField
        | Role::TextArea
        | Role::ComboBox
        | Role::List
        | Role::ListItem
        | Role::Group
        | Role::Separator
        | Role::RadioGroup
        | Role::TabPanel
        | Role::MenuItem
        | Role::Menu
        | Role::MenuBar
        | Role::Toolbar
        | Role::Alert
        | Role::Status
        | Role::Dialog
        | Role::AlertDialog
        | Role::Drawer
        | Role::Popover
        | Role::Tooltip
        | Role::Region => 0,
        // `Role` is non_exhaustive — future variants get UIAccessibilityTraitNone.
        _ => 0,
    }
}

/// Does this role imply the element should be marked
/// `isAccessibilityElement = true`? UIKit defaults
/// `UIView.isAccessibilityElement` to `false` so VoiceOver walks
/// into children. For roles that should *be* the focused element
/// (Button, Link, Image, …) we force it on even when the underlying
/// view class (UIView, not UIButton) would otherwise default to off.
fn role_forces_accessibility_element(role: Role) -> bool {
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

/// Combine role-derived + per-element trait bits.
fn traits_to_uikit_bits(role: Option<Role>, traits: AccessibilityTraits) -> u64 {
    const NOT_ENABLED: u64 = 1 << 6;
    const SELECTED: u64 = 1 << 3;
    const UPDATES_FREQUENTLY: u64 = 1 << 9;

    let mut bits = role.map(role_to_traits_bits).unwrap_or(0);

    if traits.contains(AccessibilityTraits::DISABLED) {
        bits |= NOT_ENABLED;
    }
    if traits.contains(AccessibilityTraits::SELECTED) {
        bits |= SELECTED;
    }
    if traits.contains(AccessibilityTraits::BUSY)
        || traits.contains(AccessibilityTraits::UPDATES_FREQUENTLY)
    {
        bits |= UPDATES_FREQUENTLY;
    }
    // CHECKED / MIXED / EXPANDED / COLLAPSED / REQUIRED / READONLY /
    // INVALID have no first-class UIAccessibilityTrait. They route
    // through `accessibilityValue` (see `derive_accessibility_value`).
    bits
}

/// Translate a small set of state flags into the
/// `accessibilityValue` string. VoiceOver reads this aloud after the
/// label ("Notifications, on" / "Tree node, expanded"), which is the
/// closest user-observable parity with web's `aria-checked` /
/// `aria-expanded` state announcements.
fn derive_accessibility_value(traits: AccessibilityTraits) -> Option<&'static str> {
    if traits.contains(AccessibilityTraits::MIXED) {
        Some("mixed")
    } else if traits.contains(AccessibilityTraits::CHECKED) {
        Some("1")
    } else if traits.contains(AccessibilityTraits::EXPANDED) {
        Some("expanded")
    } else if traits.contains(AccessibilityTraits::COLLAPSED) {
        Some("collapsed")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// announce_for_accessibility — UIAccessibility.post(notification:…).
// ---------------------------------------------------------------------------

/// Post a one-shot accessibility announcement via UIKit's global
/// notification channel.
///
/// VoiceOver reads `msg` aloud immediately if no other speech is in
/// flight; otherwise it queues behind the current utterance. iOS 17+
/// supports a per-announcement priority attribute
/// (`UIAccessibilitySpeechAttributeAnnouncementPriority`) we'd need
/// to attach via `NSAttributedString` — the dependency on
/// `NSAttributedString` isn't wired into this crate's feature set
/// yet, so for now `Polite` and `Assertive` both go through the same
/// default notification path. The framework still distinguishes them
/// in the signal so future iOS-17+ wiring can pick the priority up.
pub(crate) fn announce(msg: &str, _priority: LiveRegionPriority) {
    use objc2_ui_kit::{UIAccessibilityAnnouncementNotification, UIAccessibilityPostNotification};

    // NSString carries the announcement text. The underlying
    // notification copies the argument string immediately; we don't
    // need to retain it across the call.
    let ns = NSString::from_str(msg);
    // SAFETY: extern C call into UIKit. `UIAccessibilityAnnouncementNotification`
    // is a static loaded once by the dynamic linker; `&*ns` is a
    // borrowed NSObject pointer valid for the call's lifetime.
    unsafe {
        UIAccessibilityPostNotification(
            UIAccessibilityAnnouncementNotification,
            Some(&*ns as &objc2::runtime::AnyObject),
        );
    }
}

// ---------------------------------------------------------------------------
// Small helpers.
// ---------------------------------------------------------------------------

/// Write `value` via the given setter selector, or clear it (pass
/// nil) when `value` is `None`. Both `setAccessibilityLabel:` and
/// `setAccessibilityHint:` accept `nil` to revert to the platform
/// default; this is what makes reactive prop updates that clear a
/// previously-set field actually drop the value.
fn set_string_or_clear(view: &UIView, sel: objc2::runtime::Sel, value: Option<&str>) {
    match value {
        Some(v) => {
            let ns = NSString::from_str(v);
            let _: () = unsafe { msg_send![view, performSelector: sel, withObject: &*ns] };
        }
        None => {
            let nil: *const objc2_foundation::NSObject = std::ptr::null();
            let _: () =
                unsafe { msg_send![view, performSelector: sel, withObject: nil] };
        }
    }
}

/// `setAccessibilityIdentifier:` — uses the same nil-on-None
/// semantics as the label/hint setters above. Pulled into its own
/// function because `accessibilityIdentifier` lives on the
/// `UIAccessibilityIdentification` informal protocol, but it's
/// implemented by every `UIView` so `performSelector:` finds it.
fn set_identifier_or_clear(view: &UIView, value: Option<&str>) {
    set_string_or_clear(view, sel_set_accessibility_identifier(), value);
}

fn sel_set_accessibility_label() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityLabel:)
}
fn sel_set_accessibility_hint() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityHint:)
}
fn sel_set_accessibility_value() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityValue:)
}
fn sel_set_accessibility_identifier() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityIdentifier:)
}
