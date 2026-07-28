//! The authored app — CORE-AGNOSTIC source. Everything here is plain
//! `ui!` + `#[component]` authoring surface: snake_case primitives,
//! struct-literal components with named/defaulted props, children
//! blocks, f-string text, reactive closures, a reactive `if`, a keyed
//! `for` with row-local state, and handler-driven mutations. The crate
//! feature decides which core it lowers to (see lib.rs); nothing in
//! this file changes between the two.

use crate::prelude::*;
use runtime_macros::{component, ui};

/// One todo item. Rows are keyed by `id`.
#[derive(Clone, PartialEq)]
pub struct Todo {
    pub id: u32,
    pub label: String,
    pub done: bool,
}

/// The app's root signals, exposed so tests can drive mutations
/// directly as well as through recorded button handlers.
pub struct AppHandle {
    pub todos: Signal<Vec<Todo>>,
    pub draft: Signal<String>,
    pub next_id: Signal<u32>,
}

/// Build the app tree plus its handle. Must run with the owning
/// world/scope ambient (tests wrap this in `World::enter`).
pub fn build_app() -> (Element, AppHandle) {
    let todos = signal(vec![
        Todo { id: 1, label: "write tests".to_string(), done: false },
        Todo { id: 2, label: "ship it".to_string(), done: true },
    ]);
    let draft = signal(String::new());
    let next_id = signal(3u32);
    let root = ui! {
        TodoApp(todos = todos, draft = draft, next_id = next_id)
    };
    (root, AppHandle { todos, draft, next_id })
}

/// A titled container: reactive `title`, a static (unwrapped) flag with
/// a default, and a children block.
#[component]
fn Section(
    /// Section heading (reactive-by-default prop).
    title: String,
    /// Draws the `*` marker. Static prop with a default — call sites
    /// may omit it.
    #[prop(static, default = false)]
    highlighted: bool,
    children: Vec<Element>,
) -> Element {
    ui! {
        view {
            view {
                text { move || title.get() }
                if highlighted {
                    text { "*" }
                }
            }
            children
        }
    }
}

/// One todo row. Row-local `taps` state proves keyed reuse: it must
/// survive list edits elsewhere. `done` is derived from the list signal
/// (a kept keyed row is NOT re-rendered, so row content that can change
/// must read through a signal, not a snapshot prop).
#[component]
fn TodoRow(
    /// Row identity (static — it IS the key).
    #[prop(static)]
    id: u32,
    /// Display label (labels never change; arrives as a snapshot).
    label: String,
    /// The list signal, for derived row state.
    todos: Signal<Vec<Todo>>,
    /// Optional callbacks — bound only when present (repo rule 9.6).
    on_toggle: Option<Rc<dyn Fn()>>,
    on_remove: Option<Rc<dyn Fn()>>,
) -> Element {
    // Row-local state.
    let taps = signal(0u32);
    // Derived row state: one memo per row, cutting propagation when an
    // unrelated row toggles.
    let done = memo(move || todos.get().iter().any(|t| t.id == id && t.done));
    ui! {
        view {
            text { move || format!("{} {}", if done.get() { "[x]" } else { "[ ]" }, label.get()) }
            button(
                label = move || format!("bump{}:{}", id, taps.get()),
                on_click = move || taps.set(taps.get() + 1)
            )
            if let Some(cb) = on_toggle {
                button(label = move || format!("toggle-{}", id), on_click = move || (cb)())
            }
            if let Some(cb) = on_remove {
                button(label = move || format!("remove-{}", id), on_click = move || (cb)())
            }
        }
    }
}

/// The root component: a memo, an uncontrolled-input-free controlled
/// `text_input`, an add handler, a reactive empty-state `if`, and the
/// keyed list.
#[component]
fn TodoApp(
    todos: Signal<Vec<Todo>>,
    draft: Signal<String>,
    next_id: Signal<u32>,
) -> Element {
    let remaining = memo(move || todos.get().iter().filter(|t| !t.done).count());
    ui! {
        Section(title = "Todos", highlighted = true) {
            text { "{remaining} left" }
            text_input(
                value = draft,
                on_change = move |s: String| draft.set(s),
                placeholder = "What next?"
            )
            button(label = "add", on_click = move || {
                let name = draft.get();
                if name.is_empty() {
                    return;
                }
                let id = next_id.get();
                next_id.set(id + 1);
                let mut list = todos.get();
                list.push(Todo { id, label: name, done: false });
                todos.set(list);
                draft.set(String::new());
            })
            if todos.get().is_empty() {
                text { "nothing to do" }
            } else {
                view {
                    for todo in todos, key = todo.id {
                        TodoRow(
                            id = todo.id,
                            label = todo.label.clone(),
                            todos = todos,
                            on_toggle = Some(Rc::new({
                                let id = todo.id;
                                move || {
                                    let mut list = todos.get();
                                    if let Some(t) = list.iter_mut().find(|t| t.id == id) {
                                        t.done = !t.done;
                                    }
                                    todos.set(list);
                                }
                            }) as Rc<dyn Fn()>),
                            on_remove = Some(Rc::new({
                                let id = todo.id;
                                move || {
                                    let mut list = todos.get();
                                    list.retain(|t| t.id != id);
                                    todos.set(list);
                                }
                            }) as Rc<dyn Fn()>),
                        )
                    }
                }
            }
            Section(title = "About") {
                text { "static footer" }
            }
        }
    }
}
