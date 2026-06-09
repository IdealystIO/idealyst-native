//! `external-export-demo` — a project whose component is exported to
//! foreign frameworks (React, Vue, vanilla, …) via `idealyst export`.
//!
//! The component is ordinary platform-agnostic framework code; tagging it
//! `#[component(external)]` is the only thing that opts it into export.
//! Running `idealyst export` discovers it, generates a wasm-backed Web
//! Component (custom element) plus `.d.ts` + React/Vue wrappers, and emits
//! them to `dist/external/`. The author writes no bridge code.

use std::rc::Rc;

use runtime_core::{component, text, ui, Element, IdealystSchema, Reactive};

/// A friendly greeter. Demonstrates the two things that have to cross the
/// JS boundary for external export to be useful:
///
/// - a **reactive prop** (`name`) — a JS write updates the rendered text
///   without a remount, and
/// - a **callback** (`on_greet`) — a component event calls back into JS.
#[derive(Default, IdealystSchema)]
pub struct GreeterProps {
    /// Who to greet. Reactive: updating it from JS re-renders the text.
    pub name: Reactive<String>,
    /// Invoked when the "Greet" button is pressed.
    pub on_greet: Option<Rc<dyn Fn()>>,
}

#[component(external)]
pub fn Greeter(props: &GreeterProps) -> Element {
    let name = props.name.clone();
    let on_greet = props.on_greet.clone();
    ui! {
        view {
            text(move || format!("Hello, {}!", name.get()))
            button(label = "Greet".to_string(), on_click = move || {
                if let Some(cb) = &on_greet {
                    cb();
                }
            })
        }
    }
}
