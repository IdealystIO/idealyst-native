//! A `#[component(remote)]` component, for the end-to-end tests
//! (`tests/remote_attr.rs`). Every kind of prop: a value copied at mount, a
//! signal read live, and a signal written by the bundle.

// In the app build a remote component's body is compiled out (it ships in
// the bundle), so imports only it uses read as unused there.
#![cfg_attr(not(idealyst_stream_guest), allow(unused_imports))]

use std::rc::Rc;

use runtime_core::{component, signal, ui, Element, ReadSignal, Signal};

/// An APP component (not `remote`): compiled into the app; in the bundle,
/// `Greeting` uses it as an import of the app's copy. Every kind of prop
/// crossing that way: a value, a signal the bundle created and native code
/// reads (promoted), a signal native code writes, a callback into the
/// bundle, and children the bundle built.
#[component]
pub fn Panel(
    title: String,
    count: ReadSignal<i64>,
    edit: Signal<String>,
    on_reset: Option<Rc<dyn Fn()>>,
    children: Vec<Element>,
) -> Element {
    ui! {
        view() {
            text { "panel {title}" }
            text { "panel sees {count}" }
            text { "panel edit {edit}" }
            if let Some(reset) = on_reset {
                button(label = "reset", on_click = move || reset())
            }
            button(label = "app edits", on_click = move || edit.set("from app".to_string()))
            children
        }
    }
}

/// Greets, reads the app's `count`, and writes the app's `likes`.
#[component(remote)]
pub fn Greeting(name: String, count: ReadSignal<i64>, likes: Signal<i64>) -> Element {
    let taps = signal(0i64);
    let draft = signal("draft".to_string());
    let reset: Option<Rc<dyn Fn()>> = Some(Rc::new(move || taps.set(0)));
    let title = format!("of {name}");
    ui! {
        view() {
            text { "hello {name}" }
            text { "count {count}" }
            text { "taps {taps}" }
            button(label = "tap", on_click = move || taps.update(|t| t + 1))
            button(label = "like", on_click = move || likes.update(|l| l + 1))
            Panel(title = title, count = taps.read_only(), edit = draft, on_reset = reset) {
                text { "bundle child sees {draft}" }
            }
        }
    }
}
