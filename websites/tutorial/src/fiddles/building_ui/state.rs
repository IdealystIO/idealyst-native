use runtime_core::{signal, ui, Element};

pub fn app() -> Element {
    // A signal holds a value that can change. Reading it in the UI
    // subscribes that spot to it; writing it updates every reader.
    let count = signal(0);
    let step = signal(1);

    ui! {
        view {
            // An f-string slot naming a signal updates on its own.
            text { "Count: {count}  (step {step})" }

            // A closure works for anything an f-string can't express.
            text { move || if count.get() % 2 == 0 { "even" } else { "odd" } }

            view {
                button(label = "Step 1", on_click = move || step.set(1))
                button(label = "Step 5", on_click = move || step.set(5))
            }

            // `update` takes the current value and returns the next one.
            button(label = "Add step", on_click = move || count.update(|n| n + step.get()))
            button(label = "Reset", on_click = move || count.set(0))

            // Inside a handler, `count.get()` keeps returning the value from
            // before the handler ran: writes take effect once it finishes.
            // `update` is the exception — its closure is handed the latest
            // pending value — so these two updates add the step twice.
            button(
                label = "Add step twice",
                on_click = move || {
                    count.update(|n| n + step.get());
                    count.update(|n| n + step.get());
                },
            )
        }
    }
}
