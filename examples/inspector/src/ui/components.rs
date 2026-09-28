//! Components: the rendered component tree on the left, the selected
//! instance's props, methods and root element on the right.

use std::collections::HashSet;
use std::rc::Rc;

use idea_ui::{
    tone, typography_kind, variant, Adornment, Badge, Button, Checkbox, ControlSize, Field,
    FieldSize, Icon, Typography,
};
use runtime_core::{component, derived, effect, memo, pressable, signal, ui, Element, IntoElement, Signal, TrackSize};
use serde_json::{json, Value};

use super::styles::{
    grid_row, indent, Caption, ChevronBox, Column, ComponentName, ErrorText, KvBox, Mono,
    MonoMuted, OkText, Padded, Pane, PaneFooter, PaneHeader, RowBetween, RowStart, ScrollFill,
    SectionTitle, SplitRow, TableBox, TreeLabel, TreeList, TreeRowBox, TreeRowBoxSelected,
};
use crate::bridge::client::Focus;
use crate::bridge::model::{
    component_path, find_element, tree_counts, visible_rows, ElementDetail, Method, Prop, RowKey,
    RowKind, Snapshot, TreeOptions, TreeRow,
};

/// Props table tracks: name · type · value · mode.
static PROP_TRACKS: [TrackSize; 4] = [
    TrackSize::Px(140.0),
    TrackSize::Px(210.0),
    TrackSize::Fr(1.0),
    TrackSize::Px(92.0),
];

#[component]
pub fn ComponentsScreen(snapshot: Signal<Snapshot>) -> Element {
    let selected: Signal<Option<u64>> = signal(None);
    let collapsed: Signal<HashSet<RowKey>> = signal(HashSet::new());
    let show_elements = signal(false);
    let filter = signal(String::new());

    // The client fetches the selected instance's detail every refresh.
    effect(move || crate::set_focus(Focus { component: selected.get(), signal: None }));

    let rows = memo(move || {
        let snap = snapshot.get();
        let collapsed = collapsed.get();
        let filter = filter.get();
        visible_rows(&snap.tree, &TreeOptions { show_elements: show_elements.get(), collapsed: &collapsed, filter: &filter })
    });
    let counts = memo(move || {
        let (components, elements) = tree_counts(&snapshot.get().tree);
        format!("{components} components · {elements} elements")
    });
    let empty = memo(move || rows.get().is_empty());

    let on_show = Rc::new(move |v: bool| show_elements.set(v)) as Rc<dyn Fn(bool)>;
    let on_filter = Rc::new(move |v: String| filter.set(v)) as Rc<dyn Fn(String)>;

    ui! {
        view(style = SplitRow()) {
            view(style = Pane()) {
                view(style = PaneHeader()) {
                    view(style = RowBetween()) {
                        Typography(content = "Components".to_string(), kind = typography_kind::H3)
                        Checkbox(
                            label = Some("Show elements".to_string()),
                            value = show_elements,
                            on_change = on_show,
                            size = ControlSize::Sm,
                        )
                    }
                    Field(
                        value = filter,
                        on_change = on_filter,
                        placeholder = Some("Filter by name or test_id".to_string()),
                        size = FieldSize::Sm,
                        leading = Adornment::Icon(icons_lucide::SEARCH),
                    )
                }
                scroll_view(style = ScrollFill()) {
                    view(style = TreeList()) {
                        if empty {
                            text(style = Caption()) { "No components mounted yet." }
                        }
                        for row in rows, key = format!("{row:?}") {
                            TreeLine(row = row, selected = selected, collapsed = collapsed)
                        }
                    }
                }
                view(style = PaneFooter()) {
                    text(style = Caption()) { "{counts}" }
                }
            }
            ComponentDetail(snapshot = snapshot, selected = selected)
        }
    }
}

/// One tree row: an expand chevron (its own hit target) and the label,
/// which selects the component (an element row selects its owner).
#[component]
fn TreeLine(
    #[prop(static)] row: TreeRow,
    selected: Signal<Option<u64>>,
    collapsed: Signal<HashSet<RowKey>>,
) -> Element {
    let owner = row.owner;
    let is_component = matches!(row.kind, RowKind::Component { .. });
    let key = row.key;
    let boxed = TreeRowBox().selected(derived(move || {
        if is_component && selected.get() == owner { TreeRowBoxSelected::On } else { TreeRowBoxSelected::Off }
    }));
    let chevron: Element = if row.has_children {
        let glyph = if row.expanded { icons_lucide::CHEVRON_DOWN } else { icons_lucide::CHEVRON_RIGHT };
        let icon = ui! { view(style = ChevronBox()) { Icon(data = glyph, size = 14.0) } };
        pressable(vec![icon], move || {
            collapsed.update(|set| {
                let mut next = set.clone();
                if !next.insert(key) {
                    next.remove(&key);
                }
                next
            })
        })
        .into_element()
    } else {
        ui! { view(style = ChevronBox()) {} }
    };
    let (glyph, name) = match &row.kind {
        RowKind::Component { name, .. } => (icons_lucide::DIAMOND, name.clone()),
        RowKind::Element { kind, .. } => (icons_lucide::SQUARE, kind.clone()),
    };
    let meta = row.meta.clone();
    let name_text: Element = if is_component {
        ui! { text(style = ComponentName()) { name } }
    } else {
        ui! { text(style = MonoMuted()) { name } }
    };
    let icon_tone: Option<idea_ui::ToneRef> = if is_component { Some(tone::Primary.into()) } else { None };
    let label = ui! {
        view(style = TreeLabel()) {
            Icon(data = glyph, size = 12.0, tone = icon_tone)
            name_text
            text(style = MonoMuted()) { meta }
        }
    };
    let select = pressable(vec![label], move || selected.set(owner)).into_element();
    ui! {
        view(style = boxed) {
            view(style = indent(row.depth)) {}
            chevron
            select
        }
    }
}

// =============================================================================
// Detail pane
// =============================================================================

#[component]
fn ComponentDetail(snapshot: Signal<Snapshot>, selected: Signal<Option<u64>>) -> Element {
    let detail = memo(move || {
        let snap = snapshot.get();
        snap.component.filter(|c| Some(c.instance_id) == selected.get())
    });
    let has_detail = memo(move || detail.get().is_some());
    let waiting = memo(move || selected.get().is_some() && detail.get().is_none());
    let nothing = memo(move || selected.get().is_none());

    ui! {
        view(style = super::styles::Main()) {
            if nothing {
                view(style = Padded()) {
                    text(style = Caption()) { "Select a component on the left to see its props, methods and root element." }
                }
            }
            if waiting {
                view(style = Padded()) {
                    text(style = Caption()) { "Loading…" }
                }
            }
            if has_detail {
                DetailBody(snapshot = snapshot, selected = selected)
            }
        }
    }
}

#[component]
fn DetailBody(snapshot: Signal<Snapshot>, selected: Signal<Option<u64>>) -> Element {
    let detail = memo(move || snapshot.get().component.filter(|c| Some(c.instance_id) == selected.get()));
    let name = memo(move || detail.get().map(|d| d.name).unwrap_or_default());
    let path = memo(move || {
        let snap = snapshot.get();
        selected.get().map(|id| component_path(&snap.tree, id).join(" › ")).unwrap_or_default()
    });
    let subtitle = memo(move || {
        detail
            .get()
            .map(|d| match d.location() {
                Some(loc) => format!("instance #{} · {loc}", d.instance_id),
                None => format!("instance #{}", d.instance_id),
            })
            .unwrap_or_default()
    });
    let props = memo(move || detail.get().map(|d| d.props).unwrap_or_default());
    let no_props = memo(move || props.get().is_empty());
    let methods = memo(move || detail.get().map(|d| d.methods).unwrap_or_default());
    let has_methods = memo(move || !methods.get().is_empty());
    let instance = memo(move || detail.get().map(|d| d.instance_id).unwrap_or(0));
    let result = memo(move || {
        snapshot
            .get()
            .last_action
            .filter(|a| a.label.starts_with("invoke "))
            .map(|a| match a.result {
                Ok(()) => (true, format!("{} → ok · {} ms", a.label.trim_start_matches("invoke "), a.rtt_ms)),
                Err(e) => (false, format!("{} → {e}", a.label.trim_start_matches("invoke "))),
            })
    });
    let result_ok = memo(move || result.get().map(|(ok, _)| ok) == Some(true));
    let result_err = memo(move || result.get().map(|(ok, _)| ok) == Some(false));
    let result_text = memo(move || result.get().map(|(_, t)| t).unwrap_or_default());

    ui! {
        scroll_view(style = ScrollFill()) {
            view(style = Padded()) {
                view(style = Column()) {
                    text(style = Caption()) { "{path}" }
                    Typography(content = name, kind = typography_kind::H2)
                    text(style = MonoMuted()) { "{subtitle}" }
                }

                view(style = Column()) {
                    view(style = RowBetween()) {
                        text(style = SectionTitle()) { "Props" }
                        text(style = Caption()) { "Live props update the view in place. Static props were fixed when the component was built." }
                    }
                    if no_props {
                        text(style = Caption()) { "This component takes no props." }
                    } else {
                        view(style = TableBox()) {
                            view(style = grid_row(&PROP_TRACKS, true, false)) {
                                text(style = Caption()) { "Prop" }
                                text(style = Caption()) { "Type" }
                                text(style = Caption()) { "Value" }
                                text(style = Caption()) { "Mode" }
                            }
                            for p in props, key = format!("{p:?}") {
                                PropLine(prop = p)
                            }
                        }
                    }
                }

                if has_methods {
                    view(style = Column()) {
                        text(style = SectionTitle()) { "Methods" }
                        for m in methods, key = format!("{:?}", m) {
                            MethodLine(method = m, instance = instance)
                        }
                        if result_ok {
                            text(style = OkText()) { "{result_text}" }
                        }
                        if result_err {
                            text(style = ErrorText()) { "{result_text}" }
                        }
                    }
                }

                RootElement(snapshot = snapshot)
            }
        }
    }
}

/// `mode` → badge label + tone.
fn mode_badge(mode: &str) -> (&'static str, idea_ui::ToneRef) {
    match mode {
        "live" => ("Live", tone::Success.into()),
        "static" => ("Static", tone::Neutral.into()),
        "signal" => ("Signal", tone::Info.into()),
        "handler" => ("Handler", tone::Neutral.into()),
        "children" => ("Children", tone::Neutral.into()),
        "value" => ("Value", tone::Neutral.into()),
        _ => ("Opaque", tone::Warning.into()),
    }
}

#[component]
fn PropLine(#[prop(static)] prop: Prop) -> Element {
    let (label, badge_tone) = mode_badge(&prop.mode);
    let badge_variant: idea_ui::VariantRef =
        if prop.mode == "live" { variant::Soft.into() } else { variant::Outlined.into() };
    let value = prop.value.clone().unwrap_or_else(|| "—".to_string());
    ui! {
        view(style = grid_row(&PROP_TRACKS, false, false)) {
            text(style = Mono()) { prop.name.clone() }
            text(style = MonoMuted()) { prop.ty.clone() }
            text(style = Mono()) { value }
            view(style = RowStart()) {
                Badge(
                    label = label.to_string(),
                    tone = badge_tone,
                    variant = badge_variant,
                )
            }
        }
    }
}

/// One `#[method]`: an input per argument (each parsed as JSON, falling
/// back to a plain string) and an Invoke button.
#[component]
fn MethodLine(#[prop(static)] method: Method, instance: u64) -> Element {
    let inputs: Vec<(String, String, Signal<String>)> =
        method.args.iter().map(|a| (a.name.clone(), a.ty.clone(), signal(String::new()))).collect();
    let signature = method.signature();
    let name = method.name.clone();
    let args_for_call = inputs.clone();
    let invoke = Rc::new(move || {
        let mut args = serde_json::Map::new();
        for (arg, _, value) in &args_for_call {
            let raw = value.get();
            let parsed: Value = serde_json::from_str(raw.trim()).unwrap_or(Value::String(raw));
            args.insert(arg.clone(), parsed);
        }
        let id = instance.get();
        crate::action(
            format!("invoke {signature}"),
            "invoke_method",
            json!({ "instance_id": id, "method": name, "args": Value::Object(args) }),
        );
    }) as Rc<dyn Fn()>;
    let signature = method.signature();
    ui! {
        view(style = KvRow()) {
            text(style = Mono()) { signature }
            for (arg, ty, value) in inputs {
                Field(
                    value = value,
                    on_change = Rc::new(move |v: String| value.set(v)) as Rc<dyn Fn(String)>,
                    placeholder = Some(format!("{arg}: {ty}")),
                    size = FieldSize::Sm,
                    width = Some(160.0),
                )
            }
            Button(
                label = "Invoke".to_string(),
                on_click = invoke,
                size = idea_ui::size::Sm,
                leading_icon = Some(icons_lucide::PLAY),
            )
        }
    }
}

runtime_core::stylesheet! {
    KvRow<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: runtime_core::FlexDirection::Row,
            align_items: runtime_core::AlignItems::Center,
            gap: 12.0,
            padding_vertical: 10.0,
            padding_horizontal: 14.0,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.md(),
            background: t.color.surface(),
        }
    }
}

/// The selected component's root element: its identity and frame, and
/// what the host actually drew (`introspect_native`).
#[component]
fn RootElement(snapshot: Signal<Snapshot>) -> Element {
    let info = memo(move || {
        let snap = snapshot.get();
        let detail: Option<ElementDetail> = snap.element.clone();
        detail.map(|d| {
            let node = find_element(&snap.tree, d.element_id);
            let mut rows = vec![("Element".to_string(), format!(
                "#{} · {}",
                d.element_id,
                node.map(|n| n.kind.to_lowercase()).unwrap_or_else(|| "?".into())
            ))];
            if let Some(t) = node.and_then(|n| n.test_id.clone()) {
                rows.push(("test_id".into(), t));
            }
            if let Some(f) = d.frame {
                rows.push(("Frame".into(), format!("x {} · y {} · {} × {}", f.x.round(), f.y.round(), f.width.round(), f.height.round())));
            }
            if let Some(n) = node {
                rows.push(("Children".into(), n.children.len().to_string()));
            }
            let native: Vec<(String, String, Option<String>)> = d
                .native
                .as_ref()
                .map(|n| {
                    let mut v = vec![("class".to_string(), n.class.clone(), None)];
                    v.extend(n.props.iter().map(|(k, val)| {
                        let shown = val.display();
                        let swatch = (val.ty == "color").then(|| shown.clone());
                        (k.replace('_', " "), shown, swatch)
                    }));
                    v
                })
                .unwrap_or_default();
            (rows, native)
        })
    });
    let has = memo(move || info.get().is_some());
    let rows = memo(move || info.get().map(|(r, _)| r).unwrap_or_default());
    let native = memo(move || info.get().map(|(_, n)| n).unwrap_or_default());
    let has_native = memo(move || !native.get().is_empty());
    ui! {
        if has {
            view(style = super::styles::RowStart()) {
                view(style = super::styles::Grow()) {
                    text(style = SectionTitle()) { "Root element" }
                    view(style = KvBox()) {
                        for (k, v) in rows, key = format!("{k}={v}") {
                            text(style = Caption()) { k }
                            text(style = Mono()) { v }
                        }
                    }
                }
                if has_native {
                    view(style = super::styles::Grow()) {
                        text(style = SectionTitle()) { "As drawn" }
                        view(style = KvBox()) {
                            for (k, v, swatch) in native, key = format!("{k}={v}:{swatch:?}") {
                                NativeLine(name = k, value = v, swatch = swatch)
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One native property: its name, and its value (with a colour chip for
/// a colour).
#[component]
fn NativeLine(
    #[prop(static)] name: String,
    #[prop(static)] value: String,
    #[prop(static)] swatch: Option<String>,
) -> Element {
    let chip: Element = match swatch {
        Some(c) => ui! { view(style = super::styles::swatch(c)) {} },
        None => runtime_core::fragment(Vec::new()),
    };
    ui! {
        text(style = Caption()) { name }
        view(style = RowStart()) {
            chip
            text(style = Mono()) { value }
        }
    }
}

/// Test seam: the detail pane with a chosen selection (the screen keeps
/// its selection private).
#[cfg(test)]
#[component]
pub(super) fn TestDetail(snapshot: Signal<Snapshot>, selected: Signal<Option<u64>>) -> Element {
    ui! { DetailBody(snapshot = snapshot, selected = selected) }
}
