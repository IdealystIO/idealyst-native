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

/// Chrome options a navigator screen sets and its layout reads.
pub struct Title(pub &'static str);

/// A whole stack navigator, compiled into BOTH builds: the app mounts it
/// natively (`NativeNavApp`) and from the bundle (`RemoteNavApp`), and the
/// test compares the two. Its layout reads `StackNav` (depth, back, the
/// current screen's options) and has a back button calling its `pop` and a
/// button pushing a typed route through the navigator's handle; the home
/// screen has a route link.
pub fn nav_app_view() -> Element {
    use runtime_shared::primitives::navigator::Route;
    use runtime_vocabulary::builders::{button, link, navigator_outlet, stack_navigator, text, view};
    use runtime_vocabulary::prims::{NavHandle, Screen, StackNav};
    const HOME: Route = Route::new("home", "/");
    let handle = runtime_core::Ref::<NavHandle>::new();
    stack_navigator(&HOME)
        .layout(move || {
            let nav = runtime_world::inject::<StackNav>().expect("StackNav in the layout");
            let (depth, back, chrome, pop) = (nav.depth, nav.can_go_back, nav.screen_chrome, nav.pop.clone());
            view()
                .child(text().content(move || {
                    let title = chrome.get().options.as_ref().and_then(|o| o.downcast_ref::<Title>().map(|t| t.0)).unwrap_or("-");
                    format!("depth {} back {} title {title}", depth.get(), back.get())
                }))
                .child(button().label("back").on_press(move || pop()))
                .child(button().label("push 7").on_press(move || {
                    if let Some(h) = handle.get() {
                        h.push(&DETAIL, ItemId(7));
                    }
                }))
                .child(navigator_outlet().build())
                .build()
        })
        .screen(HOME, |()| {
            view().child(text().content("home")).child(link().route(&DETAIL, ItemId(4)).child(text().content("open 4"))).build()
        })
        .screen(DETAIL, |ItemId(id)| Screen::new(text().content(format!("item {id}")).build()).with(Title("Item")))
        .on_handle(move |h| handle.fill(h))
        .build()
}

/// [`nav_app_view`], from the bundle.
#[component(remote)]
pub fn RemoteNavApp() -> Element {
    nav_app_view()
}

/// [`nav_app_view`], compiled into the app.
#[component]
pub fn NativeNavApp() -> Element {
    nav_app_view()
}

/// A swap navigator (tabs), compiled into both builds like
/// [`nav_app_view`]: its layout's tab buttons select through `SwapNav`'s
/// `on_select` (which runs the screen's select recipe, made in the bundle).
pub fn tabs_view() -> Element {
    use runtime_shared::primitives::navigator::Route;
    use runtime_vocabulary::builders::{button, navigator_outlet, swap_navigator, text, view};
    use runtime_vocabulary::prims::SwapNav;
    const FEED: Route = Route::new("feed", "/feed");
    const PROFILE: Route = Route::new("profile", "/profile");
    swap_navigator(&FEED)
        .layout(|| {
            let nav = runtime_world::inject::<SwapNav>().expect("SwapNav in the layout");
            let (active, select, select2) = (nav.active_route, nav.on_select.clone(), nav.on_select.clone());
            view()
                .child(text().content(move || format!("tab {}", active.get())))
                .child(button().label("feed").on_press(move || select("feed")))
                .child(button().label("profile").on_press(move || select2("profile")))
                .child(navigator_outlet().build())
                .build()
        })
        .screen(FEED, |()| text().content("feed screen").build())
        .screen(PROFILE, |()| text().content("profile screen").build())
        .build()
}

#[component(remote)]
pub fn RemoteTabs() -> Element {
    tabs_view()
}

#[component]
pub fn NativeTabs() -> Element {
    tabs_view()
}
