//! Overview — landing page. What idea-ui is, the core design ideas,
//! and where to go next.

use runtime_core::{ui, Element};
use idea_ui::{Stack, StackGap};

use crate::shell::{self, Callout, ComponentPage, H2, P};

pub fn page() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "idea-ui".to_string(),
            lead = "A cross-platform component library built on the idealyst framework. \
                These docs are themselves written with idea-ui — every control panel, \
                every nav link, every overlay. The library documents itself.".to_string(),
        ) {
            H2(content = "The shape of the library".to_string())
            P(content = "idea-ui ships about 20 components organized around four \
                modifier axes: tone (semantic palette), variant (visual treatment), size, and \
                shape. A component picks the axes it needs and ignores the rest — Typography \
                uses none of them, Button uses all four, Stack adds its own layout-axis enums.".to_string())

            H2(content = "Theme is a trait".to_string())
            P(content = "There is no concrete Theme struct. Stylesheets read from a trait the \
                app implements on its own type (or uses the built-in light/dark structs). \
                Switching themes calls one function; every component re-resolves through CSS \
                custom-property tokens with no rebuild.".to_string())

            H2(content = "Modifiers are open traits".to_string())
            P(content = "Tone, Variant, ButtonSize and Shape are traits, not closed enums. \
                Built-ins (Primary, Filled, Md, …) are zero-sized marker structs. Adding a \
                custom `Hype` tone is a single trait impl — every component that consumes a \
                tone (Button, Badge, Tag, Alert) immediately works with it.".to_string())

            H2(content = "Live, interactive demos".to_string())
            P(content = "Each component page has a live preview alongside a control panel \
                built from idea-ui itself. Where the type system can reflect on a Props \
                struct, the panel is generated automatically via the `DocControls` derive.".to_string())

            Callout(label = "Where to start".to_string()) {
                Stack(gap = StackGap::Xs) {
                    P(content = "New here? Read Getting Started → Installation, then jump into the \
                        component pages from the sidebar.".to_string())
                    P(content = "Building your own component or theme? Skip ahead to the Extending \
                        section.".to_string())
                }
            }
        }
    })
}
