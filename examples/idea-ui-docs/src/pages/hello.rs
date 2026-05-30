//! First component — minimum-viable idea-ui app.

use std::rc::Rc;

use runtime_core::{signal, ui, Element, Signal};
use idea_ui::{typography_kind, Button, Card, Stack, StackGap, Typography};

use crate::shell::{self, CodePanel, ComponentPage, DemoSurface, H2, P};

pub fn page() -> Element {
    shell::layout(ui! {
        ComponentPage(
            title = "First component".to_string(),
            lead = "A counter screen, end to end. The whole app fits in a single \
                function.".to_string(),
        ) {
            H2(content = "The full source".to_string())
            P(content = "Install a theme, declare a signal, render a Card containing the live \
                count and a button that bumps it. That's it — no global state, no setup \
                outside this function.".to_string())
            CodePanel(src = r##"use runtime_core::{component, signal, ui, Element};
use idea_ui::{
    install_idea_theme, light_theme, tone, variant,
    typography_kind, Button, Card, Stack, StackGap, Typography,
};
use std::rc::Rc;

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    let count = signal!(0);
    let on_inc: Rc<dyn Fn()> = Rc::new(move || count.update(|n| *n += 1));

    ui! {
        Card {
            Stack(gap = StackGap::Md) {
                Typography(
                    content = "Counter".to_string(),
                    kind = typography_kind::H2,
                )
                Typography(content = format!("Value: {}", count.get()))
                Button(
                    label = "Increment".to_string(),
                    on_click = on_inc,
                    tone = tone::Primary,
                    variant = variant::Filled,
                )
            }
        }
    }
}"##.to_string())

            H2(content = "Live".to_string())
            P(content = "Same code, mounted right here:".to_string())
            DemoSurface {
                counter_demo()
            }

            H2(content = "What's going on".to_string())
            P(content = "`signal!(0)` returns a Copy handle. Reading it inside a Typography's \
                `content` (via the format!) subscribes the surrounding scope, and the \
                Increment button writes through the same handle. The framework re-runs only \
                the text node that depends on the signal — no re-render pass, no virtual \
                DOM.".to_string())

            P(content = "`tone::Primary` and `variant::Filled` are zero-sized markers. They \
                pick which arms of the installed Button stylesheet apply — no string lookups, \
                no runtime branching.".to_string())
        }
    })
}

fn counter_demo() -> Element {
    let count: Signal<i32> = signal!(0);
    let on_inc: Rc<dyn Fn()> = Rc::new(move || count.update(|n| *n += 1));
    ui! {
        Card {
            Stack(gap = StackGap::Md) {
                Typography(
                    content = "Counter".to_string(),
                    kind = typography_kind::H2,
                )
                Typography(content = runtime_core::rx!(format!("Value: {}", count.get())))
                Button(
                    label = "Increment".to_string(),
                    on_click = on_inc,
                    tone = idea_ui::tone::Primary,
                    variant = idea_ui::variant::Filled,
                )
            }
        }
    }
}
