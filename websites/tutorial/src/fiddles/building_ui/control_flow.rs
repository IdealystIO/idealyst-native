use runtime_core::{component, signal, ui, Element, Signal};

#[derive(Clone, PartialEq)]
struct Todo {
    id: u32,
    title: String,
}

// One row. Its `done` signal lives as long as the row does: rows are kept
// by key, so ticking a box survives adding or removing other todos.
#[component]
fn TodoRow(id: u32, title: String, todos: Signal<Vec<Todo>>) -> Element {
    let done = signal(false);
    ui! {
        view {
            toggle(value = done, on_change = move |on| done.set(on))
            text { "{title}" }
            if done.get() {
                text { "(done)" }
            }
            button(label = "Remove", on_click = move || {
                let id = id.get();
                todos.update(|list| list.iter().filter(|t| t.id != id).cloned().collect())
            })
        }
    }
}

pub fn app() -> Element {
    let todos = signal(vec![
        Todo { id: 1, title: "Install the CLI".into() },
        Todo { id: 2, title: "Write a component".into() },
    ]);
    let draft = signal(String::new());
    let next_id = signal(3);

    let add = move || {
        let title = draft.get();
        if title.trim().is_empty() {
            return;
        }
        let id = next_id.get();
        next_id.set(id + 1);
        todos.update(|list| {
            let mut list = list.clone();
            list.push(Todo { id, title: title.clone() });
            list
        });
        draft.set(String::new());
    };

    ui! {
        view {
            view {
                text_input(value = draft, on_change = move |t| draft.set(t), placeholder = "Something to do")
                button(label = "Add", on_click = add)
            }

            // `todos` is a signal, so this loop follows it. `key` names each
            // row, so only rows whose key appears or disappears are rebuilt.
            for todo in todos, key = todo.id {
                TodoRow(id = todo.id, title = todo.title.clone(), todos = todos)
            }

            // A `match` that reads a signal re-runs when the signal changes.
            match todos.get().len() {
                0 => { text { "Nothing to do. Nice." } }
                1 => { text { "1 thing to do." } }
                n => { text { "{n} things to do." } }
            }
        }
    }
}
