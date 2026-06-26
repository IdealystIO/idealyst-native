// Fixture: a *plausible* idealyst todo app, used only as a static-search
// target for the arena's static tier integration test. It is intentionally
// NOT a buildable crate (no Cargo.toml) — the static verifier reads source,
// it does not compile. It hits every decision-item pattern in the todo-app
// rubric exactly once so a passing run is unambiguous.

use idealyst::prelude::*;

#[component]
fn TodoApp() -> Element {
    let items = signal!(Vec::<String>::new());
    let draft = signal!(String::new());

    // Hydrate persisted items on mount.
    let saved = storage::get("todo.items").unwrap_or_default();
    items.set(saved);

    ui! {
        view() {
            text_input(value = draft, placeholder = "Add a todo")
            flat_list(data = items) { |item|
                view() {
                    text() { item }
                }
            }
        }
    }
}

fn main() {
    idealyst::run(TodoApp);
}
