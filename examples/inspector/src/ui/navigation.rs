//! Navigation: every registered navigator, the selected one's back stack
//! and query state, and push / replace / reset / pop.

use std::rc::Rc;

use idea_ui::{tone, typography_kind, variant, Badge, Button, Field, FieldSize, Typography};
use runtime_core::{component, derived, memo, pressable, signal, ui, Element, IntoElement, Signal, TrackSize};
use serde_json::json;

use super::styles::{
    grid_row, Caption, Column, ComponentName, ErrorText, ListButton, ListButtonSelected, Main, Mono,
    MonoMuted, OkText, Padded, Pane, RowStart, ScrollFill, SectionTitle, SplitRow, StackCard,
    StackCardCurrent, StackCards, TableBox,
};
use crate::bridge::model::{Navigator, Snapshot};

static QUERY_TRACKS: [TrackSize; 2] = [TrackSize::Px(160.0), TrackSize::Fr(1.0)];

#[component]
pub fn NavigationScreen(snapshot: Signal<Snapshot>) -> Element {
    let picked: Signal<Option<u64>> = signal(None);
    let navs = memo(move || snapshot.get().navigators);
    let empty = memo(move || navs.get().is_empty());
    // The picked navigator, else the current one, else the first.
    let chosen = memo(move || {
        let navs = navs.get();
        picked
            .get()
            .and_then(|id| navs.iter().find(|n| n.nav_id == id).cloned())
            .or_else(|| navs.iter().find(|n| n.is_current).cloned())
            .or_else(|| navs.first().cloned())
    });
    let chosen_id = memo(move || chosen.get().map(|n| n.nav_id));
    let has_chosen = memo(move || chosen.get().is_some());

    ui! {
        view(style = SplitRow()) {
            view(style = Pane()) {
                view(style = Padded()) {
                    Typography(content = "Navigators".to_string(), kind = typography_kind::H3)
                    if empty {
                        text(style = Caption()) { "No navigator is mounted." }
                    }
                    for n in navs, key = format!("{n:?}") {
                        NavigatorItem(nav = n, picked = picked, chosen = chosen_id)
                    }
                }
            }
            view(style = Main()) {
                if has_chosen {
                    NavigatorDetail(snapshot = snapshot, nav = chosen)
                }
            }
        }
    }
}

#[component]
fn NavigatorItem(#[prop(static)] nav: Navigator, picked: Signal<Option<u64>>, chosen: Option<u64>) -> Element {
    let id = nav.nav_id;
    let row = ListButton().selected(derived(move || {
        if chosen.get() == Some(id) { ListButtonSelected::On } else { ListButtonSelected::Off }
    }));
    let focused = nav.is_current;
    let body = ui! {
        view(style = row) {
            view(style = RowStart()) {
                text(style = ComponentName()) { format!("#{id}") }
                text(style = Caption()) { nav.kind_label() }
                if focused {
                    Badge(label = "Focused".to_string(), tone = tone::Primary, variant = variant::Soft)
                }
            }
            text(style = MonoMuted()) { nav.active_path.clone() }
        }
    };
    pressable(vec![body], move || picked.set(Some(id))).into_element()
}

#[component]
fn NavigatorDetail(snapshot: Signal<Snapshot>, nav: Option<Navigator>) -> Element {
    let n = nav.clone();
    let title = memo(move || n.get().map(|n| format!("{} navigator #{}", n.kind_label(), n.nav_id)).unwrap_or_default());
    let n = nav.clone();
    let path = memo(move || n.get().map(|n| n.active_path).unwrap_or_default());
    let n = nav.clone();
    let stack = memo(move || {
        let n = n.get();
        let len = n.as_ref().map(|n| n.stack.len()).unwrap_or(0);
        n.map(|n| {
            n.stack.into_iter().enumerate().map(|(i, e)| (i, e.route, e.path, i + 1 == len)).collect::<Vec<_>>()
        })
        .unwrap_or_default()
    });
    let n = nav.clone();
    let query = memo(move || n.get().map(|n| n.query()).unwrap_or_default());
    let no_query = memo(move || query.get().is_empty());
    let n = nav.clone();
    let controllable = memo(move || n.get().map(|n| n.controllable).unwrap_or(false));
    let n = nav;
    let nav_id = memo(move || n.get().map(|n| n.nav_id).unwrap_or(0));

    let draft = signal(String::new());
    let on_draft = move |v: String| draft.set(v);
    let send = move |action: &'static str| {
        Rc::new(move || {
            let mut args = json!({ "nav_id": nav_id.get(), "action": action });
            if action != "pop" {
                args["path"] = json!(draft.get());
            }
            crate::action(format!("navigate {action}"), "navigate", args);
        }) as Rc<dyn Fn()>
    };
    let result = memo(move || {
        snapshot.get().last_action.filter(|a| a.label.starts_with("navigate ")).map(|a| match a.result {
            Ok(()) => (true, format!("{} · {} ms", a.label.trim_start_matches("navigate "), a.rtt_ms)),
            Err(e) => (false, e),
        })
    });
    let ok = memo(move || result.get().map(|(ok, _)| ok) == Some(true));
    let err = memo(move || result.get().map(|(ok, _)| ok) == Some(false));
    let result_text = memo(move || result.get().map(|(_, t)| t).unwrap_or_default());

    ui! {
        scroll_view(style = ScrollFill()) {
            view(style = Padded()) {
                view(style = Column()) {
                    Typography(content = title, kind = typography_kind::H2)
                    text(style = Mono()) { "{path}" }
                }
                view(style = Column()) {
                    text(style = SectionTitle()) { "Back stack · oldest first" }
                    view(style = StackCards()) {
                        for (i, route, path, current) in stack, key = format!("{i}:{route}:{path}:{current}") {
                            view(style = StackCard().current(if current { StackCardCurrent::On } else { StackCardCurrent::Off })) {
                                view(style = RowStart()) {
                                    text(style = Caption()) { i.to_string() }
                                    if current {
                                        Badge(label = "Current".to_string(), tone = tone::Primary, variant = variant::Soft)
                                    }
                                }
                                text(style = ComponentName()) { route.clone() }
                                text(style = MonoMuted()) { path.clone() }
                            }
                        }
                    }
                }
                view(style = Column()) {
                    text(style = SectionTitle()) { "Screen state · query params" }
                    if no_query {
                        text(style = Caption()) { "The current path has no query parameters." }
                    } else {
                        view(style = TableBox()) {
                            for (k, v) in query, key = format!("{k}={v}") {
                                view(style = grid_row(&QUERY_TRACKS, false, false)) {
                                    text(style = Mono()) { k }
                                    text(style = Mono()) { v }
                                }
                            }
                        }
                    }
                }
                if controllable {
                    view(style = Column()) {
                        text(style = SectionTitle()) { "Drive this navigator" }
                        view(style = RowStart()) {
                            Field(
                                value = draft,
                                on_change = Rc::new(on_draft) as Rc<dyn Fn(String)>,
                                placeholder = Some("/settings/profile".to_string()),
                                size = FieldSize::Sm,
                                width = Some(300.0),
                            )
                            Button(label = "Push".to_string(), on_click = send("push"), size = idea_ui::size::Sm)
                            Button(label = "Replace".to_string(), on_click = send("replace"), size = idea_ui::size::Sm, tone = tone::Neutral, variant = variant::Outlined)
                            Button(label = "Reset".to_string(), on_click = send("reset"), size = idea_ui::size::Sm, tone = tone::Neutral, variant = variant::Outlined)
                            Button(label = "Pop".to_string(), on_click = send("pop"), size = idea_ui::size::Sm, tone = tone::Neutral, variant = variant::Outlined)
                        }
                        text(style = Caption()) { "Paths resolve like a deep link: the full path, including the navigator's base." }
                        if ok {
                            text(style = OkText()) { "{result_text}" }
                        }
                        if err {
                            text(style = ErrorText()) { "{result_text}" }
                        }
                    }
                }
            }
        }
    }
}
