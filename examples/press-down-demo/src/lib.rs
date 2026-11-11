//! `press-down-demo` — the two `PressActivation` modes side by side.
//!
//! Each button opens the same `Menu`. The left one is a normal click: its
//! menu opens when the pointer is released over it. The right one is
//! `PressActivation::Down`: its menu is open as soon as the pointer goes
//! down, the way a desktop menu button behaves. The counters under each
//! button show that one press is one action in either mode.

use std::rc::Rc;

use idea_ui::{
    install_idea_theme, light_theme, tone, variant, Button, Menu, MenuEntry, MenuItem, MenuLabel,
    MenuSeparator, SubMenu,
};
use runtime_core::{
    component, signal, stylesheet, ui, AlignItems, AnchorTarget, Element, FlexDirection,
    PressActivation, PressableHandle, Ref, SafeAreaSides,
};

stylesheet! {
    pub Screen<()> {
        base(_t) {
            flex_grow: 1.0,
            padding: 32,
            gap: 24,
        }
    }
}

stylesheet! {
    pub Row<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: 48,
        }
    }
}

stylesheet! {
    pub Column<()> {
        base(_t) {
            gap: 8,
            align_items: AlignItems::FlexStart,
        }
    }
}

/// One menu button and the count of times its action fired.
#[component]
fn MenuButton(
    #[prop(static)] label: String,
    #[prop(static)] activation: PressActivation,
) -> Element {
    let open = signal(false);
    let fired = signal(0u32);
    let trigger: Ref<PressableHandle> = Ref::new();
    let picked = signal(String::from("nothing"));
    let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    // A row's action: record the pick, then close the menu.
    let pick = move |name: &'static str| -> Rc<dyn Fn()> {
        Rc::new(move || {
            picked.set(name.to_string());
            open.set(false);
        })
    };
    let on_click: Rc<dyn Fn()> = Rc::new(move || {
        fired.set(fired.get() + 1);
        open.set(true);
    });

    ui! {
        view(style = Column()) {
            Button(
                label = label.clone(),
                activation = activation,
                tone = tone::Neutral,
                variant = variant::Soft,
                on_click = on_click,
                bind_to = Some(trigger),
            )
            text { move || format!("opened {} times", fired.get()) }
            text { move || format!("picked: {}", picked.get()) }
            if open.get() {
                Menu(target = Some(AnchorTarget::from(trigger)), on_dismiss = Some(close.clone())) {
                    MenuLabel(text = "Edit")
                    MenuItem(label = "Rename", on_select = pick("Rename"))
                    MenuItem(label = "Duplicate", on_select = pick("Duplicate"))
                    MenuSeparator()
                    SubMenu(label = "Move to", items = vec![
                        MenuEntry::new("Inbox", pick("Inbox")),
                        MenuEntry::new("Archive", pick("Archive")),
                    ])
                    MenuItem(label = "Delete", on_select = pick("Delete"))
                }
            }
        }
    }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    ui! {
        view(style = Screen(), safe_area = SafeAreaSides::TOP) {
            text { "Press and hold each button. The left menu opens on release; the right one opens on press-down." }
            text { "On the right one, keep holding: drag onto a row and release to pick it." }
            view(style = Row()) {
                MenuButton(label = "On release".to_string(), activation = PressActivation::Release)
                MenuButton(label = "On press-down".to_string(), activation = PressActivation::Down)
            }
        }
    }
}

/// The Android wrapper's entry (see `examples/README.md`).
pub fn scene_app() -> Element {
    app()
}

/// SDK-handler registration seam. This app registers nothing.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}
