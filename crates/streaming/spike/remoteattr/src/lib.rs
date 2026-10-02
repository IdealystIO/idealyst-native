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

/// A remote component that panics on purpose: its button's handler, and
/// its effect when the app's `trigger` reaches 13. For the containment
/// tests — a panic in a bundle must not take the app down.
#[component(remote)]
pub fn Fragile(trigger: ReadSignal<i64>) -> Element {
    runtime_world::effect(move || {
        if trigger.get() == 13 {
            panic!("unlucky trigger");
        }
    });
    ui! {
        view() {
            text { "fragile sees {trigger}" }
            button(label = "boom", on_click = move || panic!("boom pressed"))
        }
    }
}

/// A remote component calling native code: the camera SDK's `#[host_fn]`s.
/// `battery_level` is sync (answered inline), `take_photo` async (the app
/// runs the real future; `spawn_then` applies the result) — written exactly
/// as native code calls them.
#[component(remote)]
pub fn Snapshot() -> Element {
    let battery = signal(format!("{:.2}", spike_camera::battery_level()));
    let last = signal("none".to_string());
    let shoot = move || {
        runtime_vocabulary::scoped_spawn::spawn_then(
            spike_camera::take_photo(spike_camera::PhotoOptions { camera: "back".into() }),
            move |photo| {
                last.set(match photo {
                    Ok(p) => format!("#{} {}x{}", p.sequence, p.width, p.height),
                    Err(e) => format!("{e:?}"),
                })
            },
        )
    };
    ui! {
        view() {
            text { "battery {battery}" }
            text { "photo {last}" }
            button(label = "shoot", on_click = shoot)
        }
    }
}

/// A remote component using a `ref`: its button focuses its text input and
/// types into it — through the app's real handle.
#[component(remote)]
pub fn Focuser() -> Element {
    let input: runtime_core::Ref<runtime_core::TextInputHandle> = runtime_core::Ref::new();
    ui! {
        view() {
            text_input(value = "", on_change = |_| {}, bind = input)
            button(label = "focus", on_click = move || {
                if let Some(h) = input.get() {
                    h.focus();
                    h.insert_text("from the bundle");
                }
            })
        }
    }
}

/// A route both builds share: the app's navigator owns it, the remote
/// component navigates to it.
pub const DETAIL: runtime_shared::primitives::navigator::Route<ItemId> =
    runtime_shared::primitives::navigator::Route::new("detail", "/items/:id");

#[derive(Clone, PartialEq, Debug)]
pub struct ItemId(pub u32);

impl runtime_shared::primitives::navigator::RouteParams for ItemId {
    fn to_path(&self, pattern: &str) -> String {
        pattern.replace(":id", &self.0.to_string())
    }
    fn from_segments(segs: &std::collections::HashMap<String, String>) -> Option<Self> {
        segs.get("id").and_then(|s| s.parse().ok()).map(ItemId)
    }
}

/// An APP component taking the navigator ref — what a remote screen hands
/// its `nav` to (a header's back button). Gets the app's own `Ref` back.
#[component]
pub fn BackButton(nav: runtime_core::Ref<runtime_vocabulary::prims::NavHandle>) -> Element {
    ui! {
        button(label = "app back", on_click = move || {
            if let Some(h) = nav.get() {
                h.pop();
            }
        })
    }
}

/// A remote SCREEN: it gets the app navigator's handle as a prop (as app
/// screens do) and pushes, pops and links — typed routes, written exactly
/// as natively.
#[component(remote)]
pub fn Navigating(nav: runtime_core::Ref<runtime_vocabulary::prims::NavHandle>) -> Element {
    let open_nine = runtime_vocabulary::builders::link()
        .route(&DETAIL, ItemId(9))
        .child(runtime_vocabulary::builders::text().content("open 9"))
        .build();
    ui! {
        view() {
            text { "remote home" }
            button(label = "push 7", on_click = move || {
                if let Some(h) = nav.get() {
                    h.push(&DETAIL, ItemId(7));
                }
            })
            button(label = "pop", on_click = move || {
                if let Some(h) = nav.get() {
                    h.pop();
                }
            })
            open_nine
            BackButton(nav = nav)
        }
    }
}
