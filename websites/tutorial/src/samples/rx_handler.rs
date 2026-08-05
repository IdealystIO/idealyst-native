use idea_ui::{dark_theme, set_idea_theme};
use runtime_core::{signal, ui, Element, Signal};

fn build() -> Element {
    // Creation happens at build time, while the world is entered.
    let count: Signal<i32> = signal(0);

    ui! {
        view() {
            // Handlers run OUTSIDE the world. They may read and write any
            // handle captured at build time — that is the everyday
            // surface — but they may not create signals, effects or memos,
            // and they may not reach for the ambient world.
            button(label = "Add".to_string(), on_click = move || count.update(|n| n + 1))
            button(label = "Dark".to_string(), on_click = move || set_idea_theme(dark_theme()))
        }
    }
}
