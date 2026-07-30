use runtime_core::{rx, signal, ui, Element, Signal};

fn counter() -> Element {
    let count: Signal<i32> = signal(0);

    // This text node IS an effect. Reading `count` inside rx! subscribes
    // it, so a write repaints this one node at the flush — no diff pass,
    // no tree walk.
    ui! {
        view() {
            text() { rx!(format!("Count: {}", count.get())) }
            button(label = "Add".to_string(), on_click = move || count.update(|n| n + 1))
        }
    }
}
