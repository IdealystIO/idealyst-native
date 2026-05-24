//! Web backend accessibility translation — `AccessibilityProps` →
//! ARIA / role / aria-live attributes on the DOM.
//!
//! Strategy: a single [`apply`] function takes a DOM `Element` and the
//! resolved `AccessibilityProps` and writes (or clears) each attribute.
//! All `create_*` paths call it after constructing the node; the
//! dynamic [`update_accessibility`](runtime_core::Backend::update_accessibility)
//! path reuses it identically. Removing-on-`None` is intentional —
//! `update_accessibility` must be able to clear an attribute that was
//! previously set, otherwise reactive a11y prop changes leak stale
//! values on the DOM.
//!
//! Native AX trees (the browser's accessibility walker) read ARIA
//! attributes directly off the DOM, so the web backend doesn't need
//! a parallel semantics tree — `dump_accessibility_tree` stays `None`.

use runtime_core::accessibility::{
    AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
use wasm_bindgen::JsCast;
use web_sys::{Element, Node};

/// Apply / refresh every ARIA attribute on `node` from `props`.
///
/// `inferred_role` is the primitive's default role (see
/// [`runtime_core::accessibility::default_role`]). If
/// `props.role.is_none()` and `inferred_role.is_some()`, the inferred
/// role is written; if both are `None`, the `role` attribute is left
/// absent. Author-supplied `props.role` always wins.
///
/// Idempotent: every attribute is either written or explicitly
/// removed. Calling twice with the same `props` produces the same DOM
/// state; calling with different `props` always converges.
pub(crate) fn apply(node: &Node, props: &AccessibilityProps, inferred_role: Option<Role>) {
    let Some(elem) = node.dyn_ref::<Element>() else {
        // Text nodes etc. — no attributes, nothing to do.
        return;
    };

    // Role: explicit override wins; otherwise fall back to the
    // primitive's default. Skip when both are None (don't emit
    // `role=""`).
    let resolved_role = props.role.or(inferred_role);
    set_or_remove(elem, "role", resolved_role.map(role_to_aria));

    // Label / hint / id.
    set_or_remove(elem, "aria-label", props.label.as_deref());
    // `aria-describedby` would require an external description node
    // we'd have to manage. For the simpler case of a free-form hint
    // string, `title` is the standard fallback that screen readers
    // also announce (and tooltip-on-hover for sighted users is a free
    // benefit). When we grow proper described-by support — pointing
    // at another node's id — this becomes a slot for that node ref.
    set_or_remove(elem, "title", props.hint.as_deref());
    set_or_remove(elem, "id", props.identifier.as_deref());

    // `aria-hidden` / per-trait state flags.
    set_or_remove(elem, "aria-hidden", if props.hidden { Some("true") } else { None });
    apply_traits(elem, props.traits);

    // Live region: written as `aria-live="polite"|"assertive"` on the
    // element itself. For elements that need their own live-region
    // wrapper (e.g. a Status banner where the parent View should be
    // the live region), authors set `live_region` on the right node.
    set_or_remove(
        elem,
        "aria-live",
        props.live_region.map(|p| live_region_attr(p)),
    );
}

fn role_to_aria(role: Role) -> &'static str {
    // ARIA 1.2 role strings. See WAI-ARIA. Each variant here matches
    // the table in `docs/accessibility-design.md` §1.
    match role {
        // Structural
        Role::Button => "button",
        Role::Link => "link",
        Role::Image => "img",
        Role::Text => "", // ARIA has no first-class "text" role; leave empty so set_or_remove drops it
        Role::Header => "heading",
        Role::List => "list",
        Role::ListItem => "listitem",
        Role::Group => "group",
        Role::Separator => "separator",
        // Input
        Role::TextField => "textbox",
        Role::TextArea => "textbox",
        Role::Switch => "switch",
        Role::Slider => "slider",
        Role::Checkbox => "checkbox",
        Role::RadioButton => "radio",
        Role::RadioGroup => "radiogroup",
        Role::ComboBox => "combobox",
        Role::SearchField => "searchbox",
        // Disclosure / navigation
        Role::Tab => "tab",
        Role::TabList => "tablist",
        Role::TabPanel => "tabpanel",
        Role::NavigationLink => "link",
        Role::MenuItem => "menuitem",
        Role::Menu => "menu",
        Role::MenuBar => "menubar",
        Role::Toolbar => "toolbar",
        // Feedback
        Role::Alert => "alert",
        Role::Status => "status",
        Role::ProgressBar => "progressbar",
        Role::Spinner => "progressbar",
        // Container / overlay
        Role::Dialog => "dialog",
        Role::AlertDialog => "alertdialog",
        Role::Drawer => "dialog",
        Role::Popover => "dialog",
        Role::Tooltip => "tooltip",
        Role::Region => "region",
        // `Role` is `#[non_exhaustive]` — future roles default to no
        // ARIA mapping until we choose one explicitly. Returning ""
        // (which set_or_remove drops) is safer than guessing.
        _ => "",
    }
}

fn live_region_attr(p: LiveRegionPriority) -> &'static str {
    match p {
        LiveRegionPriority::Polite => "polite",
        LiveRegionPriority::Assertive => "assertive",
    }
}

fn apply_traits(elem: &Element, traits: AccessibilityTraits) {
    // Each flag maps to one ARIA attribute. `set_or_remove` always
    // gets a `Some(_)` (the flag's bool serialized as "true"/"false")
    // OR we explicitly `remove_attribute` when the flag has no
    // meaningful "false" representation (e.g. `aria-busy="false"` is
    // legal but noisy; we'd rather not have the attribute at all when
    // the flag is off).
    bool_or_remove(elem, "aria-selected", traits.contains(AccessibilityTraits::SELECTED));
    bool_or_remove(elem, "aria-disabled", traits.contains(AccessibilityTraits::DISABLED));
    // aria-expanded is tri-state (true/false/undefined). We use
    // EXPANDED for "true" and COLLAPSED for "false" so authors can
    // disambiguate "is currently collapsed" from "has no expand state."
    if traits.contains(AccessibilityTraits::EXPANDED) {
        let _ = elem.set_attribute("aria-expanded", "true");
    } else if traits.contains(AccessibilityTraits::COLLAPSED) {
        let _ = elem.set_attribute("aria-expanded", "false");
    } else {
        let _ = elem.remove_attribute("aria-expanded");
    }
    // aria-checked: similar tri-state w/ "mixed" support.
    if traits.contains(AccessibilityTraits::MIXED) {
        let _ = elem.set_attribute("aria-checked", "mixed");
    } else if traits.contains(AccessibilityTraits::CHECKED) {
        let _ = elem.set_attribute("aria-checked", "true");
    } else {
        let _ = elem.remove_attribute("aria-checked");
    }
    bool_or_remove(elem, "aria-busy", traits.contains(AccessibilityTraits::BUSY));
    bool_or_remove(elem, "aria-required", traits.contains(AccessibilityTraits::REQUIRED));
    bool_or_remove(elem, "aria-readonly", traits.contains(AccessibilityTraits::READONLY));
    bool_or_remove(elem, "aria-invalid", traits.contains(AccessibilityTraits::INVALID));
    // UPDATES_FREQUENTLY → aria-live=polite (only if the author hasn't
    // already set a live_region). We don't override an explicit
    // live_region here — that's set by the caller via `props.live_region`.
    // This branch only kicks in when the trait flag is on AND no
    // explicit live_region was set. To avoid overwriting, we only
    // emit when there's no existing aria-live attribute.
    if traits.contains(AccessibilityTraits::UPDATES_FREQUENTLY)
        && !elem.has_attribute("aria-live")
    {
        let _ = elem.set_attribute("aria-live", "polite");
    }
}

fn set_or_remove(elem: &Element, attr: &str, value: Option<&str>) {
    match value {
        // `Some("")` from role mapping means "this role has no ARIA
        // equivalent" — clear the attribute.
        Some("") | None => {
            let _ = elem.remove_attribute(attr);
        }
        Some(v) => {
            let _ = elem.set_attribute(attr, v);
        }
    }
}

fn bool_or_remove(elem: &Element, attr: &str, on: bool) {
    if on {
        let _ = elem.set_attribute(attr, "true");
    } else {
        let _ = elem.remove_attribute(attr);
    }
}

// ---------------------------------------------------------------------------
// announce_for_accessibility — lazy hidden live-region pair on <body>.
// ---------------------------------------------------------------------------

/// Post a one-shot accessibility announcement. The web pattern is a
/// hidden element with `aria-live` set; assistive tech (NVDA, JAWS,
/// VoiceOver) observes mutations to it and reads the new text.
///
/// We keep one polite + one assertive live-region pair on
/// `document.body`, lazily created on first use. Each call:
///
/// 1. Writes `msg` into the matching element's `textContent`.
/// 2. Schedules a clear after a short delay so re-announcing the
///    same string is observed as a fresh mutation (screen readers
///    suppress identical consecutive text).
///
/// Idempotent across calls; safe to invoke from any
/// `Backend::announce_for_accessibility` site.
pub(crate) fn announce(msg: &str, priority: LiveRegionPriority) {
    let Some(window) = web_sys::window() else { return };
    let Some(document) = window.document() else { return };
    let Some(body) = document.body() else { return };

    let id = match priority {
        LiveRegionPriority::Polite => "__idealyst_ax_announce_polite",
        LiveRegionPriority::Assertive => "__idealyst_ax_announce_assertive",
    };
    let live_attr = live_region_attr(priority);

    // Find existing or create.
    let region = if let Some(existing) = document.get_element_by_id(id) {
        existing
    } else {
        let elem = match document.create_element("div") {
            Ok(e) => e,
            Err(_) => return,
        };
        let _ = elem.set_attribute("id", id);
        let _ = elem.set_attribute("aria-live", live_attr);
        let _ = elem.set_attribute("aria-atomic", "true");
        // Visually-hidden styles. Position + clip so the element
        // doesn't take any visible space but stays in the AX tree.
        let _ = elem.set_attribute(
            "style",
            "position:absolute;left:-10000px;top:auto;width:1px;height:1px;\
             overflow:hidden;clip:rect(1px,1px,1px,1px);white-space:nowrap;",
        );
        let _ = body.append_child(&elem);
        elem
    };

    // Clear-then-set so consecutive identical announcements still
    // produce a mutation event. Some readers (NVDA) suppress text
    // changes that don't actually change the textContent.
    region.set_text_content(Some(""));
    region.set_text_content(Some(msg));
}
