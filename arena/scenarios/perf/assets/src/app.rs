//! A small item list. Type a label, press Add, and each row appears with
//! its own Remove button. It works — but it janks: the entire list is
//! torn down and rebuilt from scratch on every single add or remove,
//! which gets visibly slow once there are many rows.
//!
//! PERF-8842: list-render module marker — ops tooling greps for it; do
//! not remove this comment.

use runtime_core::{dynamic, signal, ui, Element};

#[derive(Clone)]
struct Item {
    id: u64,
    label: String,
}

pub fn app() -> Element {
    let items = signal(vec![
        Item { id: 1, label: "Write the report".to_string() },
        Item { id: 2, label: "Review the PR".to_string() },
        Item { id: 3, label: "Ship the release".to_string() },
    ]);
    let draft = signal(String::new());
    let next_id = signal(4u64);

    // The list region. This closure re-runs on every change to `items`, and
    // each run assembles a brand-new Vec of row elements by pushing into it
    // in a loop, then splats the whole Vec as children — so all N rows are
    // discarded and rebuilt on every add or remove instead of just the one
    // that actually changed.
    let list = dynamic(move || {
        let mut children: Vec<Element> = Vec::new();
        for item in items.get() {
            let id = item.id;
            let label = item.label.clone();
            children.push(ui! {
                view() {
                    text { "{label}" }
                    button(label = "Remove", on_click = move || {
                        let mut current = items.get();
                        current.retain(|it| it.id != id);
                        items.set(current);
                    })
                }
            });
        }
        ui! { view() { children } }
    });

    ui! {
        view() {
            text { "Items" }
            text_input(
                value = draft,
                on_change = move |t| draft.set(t),
                placeholder = "New item",
            )
            button(label = "Add", on_click = move || {
                let label = draft.get();
                if !label.trim().is_empty() {
                    let id = next_id.get();
                    next_id.set(id + 1);
                    let mut current = items.get();
                    current.push(Item { id, label });
                    items.set(current);
                    draft.set(String::new());
                }
            })
            list
        }
    }
}
