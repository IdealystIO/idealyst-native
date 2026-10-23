use runtime_core::{component, signal, ui, Element};

// Every prop arrives as a `Reactive<T>`: a fixed value or a live one,
// depending on what the caller passed. `#[prop(static)]` opts a prop out:
// it is a plain `String`, and passing it a signal would not compile.
#[component]
fn NameTag(live: String, frozen: String, #[prop(static)] heading: String) -> Element {
    // `.get()` here reads the prop ONCE, while the component builds.
    // Nothing reads it again, so this text never changes.
    let snapshot = frozen.get();

    ui! {
        view {
            text { heading }
            // Handed straight to the text, the prop stays live: the text
            // reads it again whenever it changes.
            text { "Live: {live}" }
            text { "Snapshot: {snapshot}" }
        }
    }
}

pub fn app() -> Element {
    let name = signal(String::from("Ada"));

    ui! {
        view {
            text_input(value = name, on_change = move |next| name.set(next))

            // The same signal goes to both props. Type in the box: only
            // `live` follows it.
            NameTag(live = name, frozen = name, heading = "Signals")

            // Literals are static: built once, with no subscription at all.
            NameTag(live = "Grace", frozen = "Hopper", heading = "Literals")
        }
    }
}
