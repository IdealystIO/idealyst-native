use runtime_core::{component, signal, ui, Element};

// A component is a function marked `#[component]`. Its name is PascalCase,
// and its parameters are its props.
#[component]
fn Greeting(name: String, #[prop(default = 1)] waves: u32) -> Element {
    ui! {
        text { "Hello, {name}! (waved {waves} times)" }
    }
}

// Each use of a component gets its own state: two Counters, two counts.
#[component]
fn Counter(label: String) -> Element {
    let count = signal(0);
    ui! {
        view {
            text { "{label}: {count}" }
            button(label = "+1", on_click = move || count.update(|n| n + 1))
        }
    }
}

pub fn app() -> Element {
    let name = signal(String::from("Ada"));

    ui! {
        view {
            // Props are passed by name. `waves` has a default, so it can be left out.
            Greeting(name = "world")
            Greeting(name = "Rustacean", waves = 3)

            // A prop can also be a signal: this greeting follows the input.
            text_input(value = name, on_change = move |next| name.set(next))
            Greeting(name = name)

            Counter(label = "Apples")
            Counter(label = "Pears")
        }
    }
}
