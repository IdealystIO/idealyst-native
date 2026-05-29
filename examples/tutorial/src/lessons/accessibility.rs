//! Accessibility track. The data model and backend wiring are shipped;
//! the author-facing setter surface is still landing, and these lessons
//! say so plainly rather than teaching an API that isn't there yet.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Typography};

use crate::common::{Callout, CodePanel, DocsLink, LessonPage};
use crate::routes::{A11Y_DEFAULTS_ROUTE, A11Y_MODEL_ROUTE};
use crate::shell;

pub fn defaults() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = A11Y_DEFAULTS_ROUTE.name(),
            title = "Accessible by default".to_string(),
            lead = "Default roles and platform label-derivation cover the common case.".to_string(),
        ) {
            Typography(
                content = "The framework carries one accessibility model and each backend maps \
                    it to its platform's native system: UIAccessibility on iOS, NSAccessibility \
                    on macOS, AccessibilityNodeInfo on Android, ARIA on web. You write the \
                    model once; the backend translates it.".to_string()
            )

            Typography(content = "What you get for free".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Every primitive ships a default semantic role: Button becomes a \
                    button, Text a text node, Image an image, Slider a slider. For standard \
                    controls the platform derives the spoken label from the visible content \
                    \u{2014} a button announces its title, a text node announces its string. So \
                    a labeled button is already announced correctly on VoiceOver and TalkBack \
                    with no extra code.".to_string()
            )
            Typography(
                content = "You only override the defaults in three cases: the visible shape and \
                    the a11y intent diverge (a Pressable that's really a navigation link), the \
                    element carries state a screen reader should announce (selected, disabled, \
                    expanded), or the content is decorative and should be hidden from the \
                    tree.".to_string()
            )

            DocsLink(
                summary = "The full per-platform mapping and the model reference.".to_string(),
                link_label = "Accessibility guide".to_string(),
                doc_file = "accessibility.md".to_string(),
            )
        }
    })
}

pub fn model() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = A11Y_MODEL_ROUTE.name(),
            title = "The accessibility model".to_string(),
            lead = "AccessibilityProps: roles, traits, live regions, and actions.".to_string(),
        ) {
            Typography(
                content = "When you do need to override, the per-element data is \
                    AccessibilityProps. Every field is optional; the default means \"infer \
                    everything from the primitive.\" The fields are: label (spoken name), hint \
                    (longer description), role (override the inferred role), traits (state \
                    flags), hidden (drop from the tree), live_region (announce updates), \
                    actions (custom assistive-tech actions), and identifier (a stable id for \
                    external AX tooling).".to_string()
            )
            CodePanel(src = r##"use runtime_core::accessibility::{AccessibilityProps, Role, AccessibilityTraits};

let props = AccessibilityProps {
    label: Some("Close dialog".to_string()),
    role: Some(Role::Button),
    traits: AccessibilityTraits::DISABLED,
    ..Default::default()
};"##.to_string())

            Typography(content = "Roles and traits".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Role names a widget's semantics independent of how it looks \u{2014} \
                    Button, Link, Slider, Tab, Dialog, and more. AccessibilityTraits is a \
                    bitflag set of orthogonal states you compose with the | operator: \
                    SELECTED, DISABLED, EXPANDED, CHECKED, BUSY, REQUIRED, INVALID, and others. \
                    Each maps to the platform's matching AX attribute.".to_string()
            )
            CodePanel(src = r##"let traits = AccessibilityTraits::SELECTED | AccessibilityTraits::EXPANDED;"##.to_string())

            Typography(
                content = "Live regions and actions".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Setting live_region (Polite or Assertive) together with an explicit \
                    label makes the backend re-announce when a reactive update changes the \
                    label \u{2014} Polite queues behind current speech, Assertive interrupts. \
                    An AccessibilityAction { name, handler } exposes an action to assistive \
                    tech with no visible control (a rotor entry on VoiceOver, a TalkBack menu \
                    action); the handler runs on the reactive thread and can update \
                    signals.".to_string()
            )

            Callout(label = "Attaching props is still being built out".to_string()) {
                Typography(
                    content = "AccessibilityProps reaches every backend, but the only shipped \
                        author setter is LazyBuilder::with_accessibility(props). There is not \
                        yet a .accessibility(props) method on the common builders or an \
                        accessibility = ... prop in ui!, and announce_for_accessibility is a \
                        Backend method with no author-facing free function. Until that lands, \
                        the defaults from the previous step carry the common case.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "The model reference and the in-progress author surface.".to_string(),
                link_label = "Accessibility guide".to_string(),
                doc_file = "accessibility.md".to_string(),
            )
        }
    })
}
