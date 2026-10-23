use runtime_core::{signal, ui, Element};

// Primitives are the framework's built-in leaves, always lowercase. Each
// renders as the platform's own widget: a <button> on the web, a UIButton
// on iOS, a Button on Android.
pub fn app() -> Element {
    let name = signal(String::new());
    let subscribed = signal(false);
    let volume = signal(40.0_f32);

    ui! {
        view {
            text { "Primitives" }

            // Props are `name = value` pairs.
            button(label = "Press me", on_click = || {})

            text_input(
                value = name,
                on_change = move |next| name.set(next),
                placeholder = "Your name",
            )

            toggle(value = subscribed, on_change = move |on| subscribed.set(on))

            slider(
                value = volume,
                min = 0.0,
                max = 100.0,
                on_change = move |v| volume.set(v),
            )

            // Children go in braces; any primitive can hold them.
            view {
                text { "Hello, {name}!" }
                text { "Volume: {volume}" }
            }
        }
    }
}
