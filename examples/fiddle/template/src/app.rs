//! The starter the server replaces on every compile. It is committed so
//! the template builds on its own (`cargo check` from this directory).

use runtime_core::{component, signal, ui, Element};

#[component]
pub fn app() -> Element {
    let count = signal(0);
    ui! {
        view {
            text { "Clicked {count} times" }
            button(label = "Click me", on_click = move || count.update(|n| n + 1))
        }
    }
}
