//! Patterns — sensible combinations of the overlay/menu components:
//! menus, submenus, menu-with-search, an autocomplete popover, and
//! toasts.

use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{signal, ui, IntoElement, Element, PressableHandle, Ref, ViewHandle};
use idea_ui::{
    push_toast, tone, variant, Button, Card, Field, Menu, MenuEntry, MenuItem, MenuLabel,
    MenuSeparator, Stack, StackGap, SubMenu, ToastHost, ToastPlacement, Typography,
};

use crate::shell::{self, CodePanel, ComponentPage, DemoSurface, H2, P, Prop, PropsTable, Section};

const FRUITS: &[&str] = &[
    "Apple", "Apricot", "Banana", "Blackberry", "Blueberry", "Cherry", "Mango", "Peach", "Pear",
    "Pineapple", "Raspberry", "Strawberry",
];

// =============================================================================
// Menus — basic, submenu, search
// =============================================================================

pub fn menus() -> Element {
    // --- basic menu with a submenu ---
    let open = signal!(false);
    let trigger: Ref<PressableHandle> = Ref::new();
    let open_menu: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    let dismiss: Rc<dyn Fn()> = close.clone();

    let folders = vec![
        MenuEntry::new("Inbox", close.clone()),
        MenuEntry::new("Archive", close.clone()),
        MenuEntry::new("Spam", close.clone()),
    ];

    // --- search menu ---
    let sopen = signal!(false);
    let strigger: Ref<PressableHandle> = Ref::new();
    let query = signal!(String::new());
    let on_query: Rc<dyn Fn(String)> = Rc::new(move |s| query.set(s));
    let open_search: Rc<dyn Fn()> = Rc::new(move || sopen.set(true));
    let close_search: Rc<dyn Fn()> = Rc::new(move || sopen.set(false));

    shell::layout(ui! {
        ComponentPage(
            title = "Menu".to_string(),
            lead = "An anchored command surface composed from MenuItem / MenuLabel / \
                MenuSeparator / SubMenu. The host owns an open-state signal and gates \
                mounting — the same shape as Popover.".to_string(),
        ) {
            H2(content = "Menu + submenu".to_string())
            DemoSurface {
                Button(
                    label = "Actions".to_string(),
                    on_click = open_menu,
                    tone = tone::Neutral,
                    variant = variant::Soft,
                    bind_to = Some(trigger),
                )
                if open.get() {
                    Menu(target = Some(AnchorTarget::from(trigger)), on_dismiss = Some(dismiss.clone())) {
                        MenuLabel(text = "Edit")
                        MenuItem(label = "Rename", on_select = close.clone())
                        MenuItem(label = "Duplicate", on_select = close.clone())
                        MenuSeparator()
                        SubMenu(label = "Move to…", items = folders.clone())
                        MenuItem(label = "Delete", on_select = close.clone())
                    }
                }
            }

            H2(content = "Menu + search".to_string())
            P(content = "Drop a Field at the top of the menu and filter the items \
                reactively — the rows rebuild as you type.".to_string())
            DemoSurface {
                Button(
                    label = "Pick a fruit".to_string(),
                    on_click = open_search,
                    tone = tone::Primary,
                    variant = variant::Soft,
                    bind_to = Some(strigger),
                )
                if sopen.get() {
                    Menu(target = Some(AnchorTarget::from(strigger)), on_dismiss = Some(close_search.clone())) {
                        Field(value = query, on_change = on_query.clone(), placeholder = Some("Filter…".to_string()))
                        search_results(query, sopen)
                    }
                }
            }

            Section(title = "Why SubMenu takes data, not children".to_string()) {
                P(content = "A SubMenu flyout mounts conditionally, so its contents are \
                    passed as reconstructable `MenuEntry` data — the `when`-gated builder \
                    rebuilds it on each open. Top-level Menu contents are composed children \
                    because the Menu mounts them once.".to_string())
            }

            Section(title = "Menu — props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "target",     ty: "Option<AnchorTarget>", desc: "Element to anchor against — AnchorTarget::from(some_ref). Required." },
                    Prop { name: "on_dismiss", ty: "Option<Rc<dyn Fn()>>", desc: "Fires on click-outside / Escape; flip your open-state signal here." },
                    Prop { name: "side",       ty: "ElementSide",          desc: "Which side of the anchor the menu opens toward." },
                    Prop { name: "align",      ty: "ElementAlign",         desc: "Alignment along the anchor edge." },
                    Prop { name: "offset",     ty: "f32",                  desc: "Gap in px between the anchor and the menu." },
                    Prop { name: "children",   ty: "Vec<Element>",         desc: "Menu rows — MenuItem / MenuLabel / MenuSeparator / SubMenu." },
                ])
            }

            Section(title = "MenuItem — props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",     ty: "Reactive<String>", desc: "Row label. Static or live." },
                    Prop { name: "on_select", ty: "Rc<dyn Fn()>",     desc: "Fires when the row is chosen. Typically also closes the menu." },
                    Prop { name: "leading",   ty: "Option<Element>",  desc: "Optional leading element (icon, avatar)." },
                    Prop { name: "trailing",  ty: "Option<Element>",  desc: "Optional trailing element (shortcut hint, badge), pushed right." },
                    Prop { name: "active",    ty: "bool",             desc: "Renders the row in its highlighted/active state." },
                ])
            }

            Section(title = "MenuLabel & MenuSeparator — props".to_string()) {
                P(content = "`MenuLabel` is a non-interactive section heading; \
                    `MenuSeparator` is a hairline divider between groups (no props).".to_string())
                PropsTable(rows = vec![
                    Prop { name: "text", ty: "Reactive<String>", desc: "MenuLabel: section heading text." },
                ])
            }

            Section(title = "SubMenu — props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label", ty: "Reactive<String>",  desc: "Trigger row label." },
                    Prop { name: "items", ty: "Vec<MenuEntry>",    desc: "Flyout contents as reconstructable data. MenuEntry::new(label, on_select). Selecting an entry runs its on_select and closes the flyout." },
                    Prop { name: "side",  ty: "ElementSide",       desc: "Which side the flyout opens toward. Default: End (right in LTR)." },
                ])
                CodePanel(src = r##"let open = signal!(false);
let trigger: Ref<PressableHandle> = Ref::new();
let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

ui! {
    Button(label = "Actions".into(), on_click = move || open.set(true), bind_to = Some(trigger))
    if open.get() {
        Menu(target = Some(AnchorTarget::from(trigger)), on_dismiss = Some(close.clone())) {
            MenuLabel(text = "Edit")
            MenuItem(label = "Rename", on_select = close.clone())
            MenuSeparator()
            SubMenu(label = "Move to…", items = vec![
                MenuEntry::new("Inbox", close.clone()),
                MenuEntry::new("Archive", close.clone()),
            ])
        }
    }
}"##.to_string())
            }
        }
    })
}

/// Reactive filtered rows for the search menu. `switch` rebuilds the
/// row set whenever `query` changes.
fn search_results(query: runtime_core::Signal<String>, open: runtime_core::Signal<bool>) -> Element {
    runtime_core::switch(
        move || query.get(),
        move |q: &String| {
            let needle = q.to_lowercase();
            let mut rows: Vec<Element> = Vec::new();
            for &f in FRUITS {
                if needle.is_empty() || f.to_lowercase().contains(&needle) {
                    let pick: Rc<dyn Fn()> = {
                        let label = f.to_string();
                        Rc::new(move || {
                            query.set(label.clone());
                            open.set(false);
                        })
                    };
                    rows.push(ui! { MenuItem(label = f, on_select = pick) });
                }
            }
            if rows.is_empty() {
                rows.push(ui! { MenuItem(label = "No matches", on_select = Rc::new(|| {}) as Rc<dyn Fn()>) });
            }
            ui! { Stack(gap = StackGap::None) { rows } }
        },
    )
}

// =============================================================================
// Combos — autocomplete popover + toasts
// =============================================================================

pub fn combos() -> Element {
    // --- autocomplete: Field with a results popover below ---
    let query = signal!(String::new());
    let on_query: Rc<dyn Fn(String)> = Rc::new(move |s| query.set(s));
    let anchor: Ref<ViewHandle> = Ref::new();

    let field = ui! {
        Field(
            label = Some("Search fruit".to_string()),
            value = query,
            on_change = on_query,
            placeholder = Some("Type to filter…".to_string()),
        )
    };
    let anchored_field = runtime_core::view(vec![field]).bind(anchor).into_element();
    let results = autocomplete_popover(query, anchor);

    shell::layout(ui! {
        ComponentPage(
            title = "Combinations".to_string(),
            lead = "Composing the pieces: an input with a live results popover, and \
                transient toasts.".to_string(),
        ) {
            // Mount the toast host once for this page.
            ToastHost(placement = ToastPlacement::Top)

            H2(content = "Input + results popover".to_string())
            P(content = "A Field anchors a popover that lists live matches; picking one \
                fills the field. The popover only mounts while the query is non-empty.".to_string())
            DemoSurface {
                anchored_field
                results
            }

            H2(content = "Toasts".to_string())
            P(content = "Push a toast from anywhere with `push_toast`; the ToastHost \
                mounted at the top of the page renders + auto-dismisses them.".to_string())
            DemoSurface {
                Stack(gap = StackGap::Sm, axis = idea_ui::StackAxis::Row) {
                    Button(label = "Success".to_string(), on_click = toast("Saved!", "success"), tone = tone::Success, variant = variant::Soft)
                    Button(label = "Warning".to_string(), on_click = toast("Check your input", "warning"), tone = tone::Warning, variant = variant::Soft)
                    Button(label = "Error".to_string(), on_click = toast("Something went wrong", "danger"), tone = tone::Danger, variant = variant::Filled)
                }
            }

            Section(title = "ToastHost — props".to_string()) {
                P(content = "Mount one `ToastHost` near the root of the surface you want \
                    toasts to appear over; it renders + auto-dismisses every toast pushed \
                    via the global queue.".to_string())
                PropsTable(rows = vec![
                    Prop { name: "placement", ty: "ToastPlacement", desc: "Where the stack anchors on the viewport: Top (default) or Bottom." },
                ])
            }

            Section(title = "Pushing toasts".to_string()) {
                P(content = "`push_toast` / `push_toast_with` are free functions — call them \
                    from anywhere (event handlers, async results). They return the toast id, \
                    which you can pass to `dismiss_toast` to close it early.".to_string())
                CodePanel(src = r##"// Default Filled variant
push_toast("Saved!", tone::Success);

// Explicit variant
push_toast_with("Heads up", tone::Warning, variant::Soft);

// Dismiss early by id
let id = push_toast("Uploading…", tone::Info);
// later:
dismiss_toast(id);"##.to_string())
            }
        }
    })
}

/// A button click handler that pushes a toast of the given tone.
fn toast(message: &'static str, tone_key: &'static str) -> Rc<dyn Fn()> {
    Rc::new(move || {
        match tone_key {
            "success" => { push_toast(message, tone::Success); }
            "warning" => { push_toast(message, tone::Warning); }
            "danger" => { push_toast(message, tone::Danger); }
            _ => { push_toast(message, tone::Info); }
        }
    })
}

/// The autocomplete results: a `when`-gated, `switch`-rebuilt list of
/// matches in a Card, anchored below the field.
fn autocomplete_popover(query: runtime_core::Signal<String>, anchor: Ref<ViewHandle>) -> Element {
    runtime_core::when(
        move || !query.get().is_empty(),
        move || {
            let rows = runtime_core::switch(
                move || query.get(),
                move |q: &String| {
                    let needle = q.to_lowercase();
                    let mut rows: Vec<Element> = Vec::new();
                    for &f in FRUITS {
                        if f.to_lowercase().contains(&needle) {
                            let pick: Rc<dyn Fn()> = {
                                let label = f.to_string();
                                Rc::new(move || query.set(label.clone()))
                            };
                            rows.push(ui! { MenuItem(label = f, on_select = pick) });
                        }
                    }
                    if rows.is_empty() {
                        rows.push(ui! { Typography(content = "No matches".to_string(), muted = true) });
                    }
                    ui! { Stack(gap = StackGap::None) { rows } }
                },
            );
            runtime_core::anchored_overlay(
                AnchorTarget::from(anchor),
                vec![ui! { Card { rows } }],
            )
            .side(ElementSide::Below)
            .align(ElementAlign::Start)
            .offset(4.0)
            .backdrop(BackdropMode::None)
            .trap_focus(false)
            .into_element()
        },
        || ui! { view {} }.into_element(),
    )
}
