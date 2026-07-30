use runtime_core::{signal, ui, Element, Role, Signal};

fn save_button() -> Element {
    let saves: Signal<u32> = signal(0);

    ui! {
        button(
            label = "Save".to_string(),
            on_click = move || saves.update(|n| n + 1),
            a11y_label = "Save document",
            a11y_role = Role::Button,
        )
    }
}
