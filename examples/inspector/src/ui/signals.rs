//! Signals: every watched signal, and the selected one's history — plus a
//! "Set value" field for signals registered with `watch_signal_writable`.

use std::rc::Rc;

use idea_ui::{typography_kind, Button, Field, FieldSize, Typography};
use runtime_core::{component, effect, memo, pressable, signal, ui, Element, IntoElement, Signal, TrackSize};
use serde_json::{json, Value};

use super::styles::{
    bar, grid_row, Caption, ChartBox, Column, ErrorText, Main, Mono, MonoLarge, MonoMuted, OkText,
    Padded, RowBetween, RowStart, ScrollFill, SectionTitle, SidePane, SplitRow, TableBox,
};
use crate::bridge::client::Focus;
use crate::bridge::model::{ago, SignalHistory, SignalRow, Snapshot};

/// Signals table tracks: name · value · writes · last change.
static SIGNAL_TRACKS: [TrackSize; 4] = [
    TrackSize::Px(200.0),
    TrackSize::Fr(1.0),
    TrackSize::Px(72.0),
    TrackSize::Px(110.0),
];

/// History list tracks: when · value.
static HISTORY_TRACKS: [TrackSize; 2] = [TrackSize::Px(96.0), TrackSize::Fr(1.0)];

#[component]
pub fn SignalsScreen(snapshot: Signal<Snapshot>) -> Element {
    let selected: Signal<Option<u64>> = signal(None);
    effect(move || crate::set_focus(Focus { component: None, signal: selected.get() }));

    let rows = memo(move || snapshot.get().signals);
    let count = memo(move || rows.get().len().to_string());
    let empty = memo(move || rows.get().is_empty());
    let chosen = memo(move || {
        let id = selected.get()?;
        snapshot.get().signals.into_iter().find(|s| s.id == id)
    });
    let has_chosen = memo(move || chosen.get().is_some());

    ui! {
        view(style = SplitRow()) {
            view(style = Main()) {
                scroll_view(style = ScrollFill()) {
                    view(style = Padded()) {
                        view(style = RowBetween()) {
                            view(style = RowStart()) {
                                Typography(content = "Watched signals".to_string(), kind = typography_kind::H3)
                                text(style = Caption()) { "{count}" }
                            }
                            text(style = Caption()) { "A signal shows here once the app calls robot::watch_signal." }
                        }
                        if empty {
                            text(style = Caption()) { "No watched signals. Call runtime_core::robot::watch_signal(\"name\", sig) in the app to watch one." }
                        } else {
                            view(style = TableBox()) {
                                view(style = grid_row(&SIGNAL_TRACKS, true, false)) {
                                    text(style = Caption()) { "Name" }
                                    text(style = Caption()) { "Value" }
                                    text(style = Caption()) { "Writes" }
                                    text(style = Caption()) { "Last change" }
                                }
                                for row in rows, key = format!("{row:?}") {
                                    SignalLine(row = row, selected = selected)
                                }
                            }
                        }
                    }
                }
            }
            if has_chosen {
                SignalDetail(snapshot = snapshot, chosen = chosen)
            }
        }
    }
}

#[component]
fn SignalLine(#[prop(static)] row: SignalRow, selected: Signal<Option<u64>>) -> Element {
    let id = row.id;
    let is_selected = memo(move || selected.get() == Some(id));
    let last = row.changed_ago_ms.map(ago).unwrap_or_else(|| "—".to_string());
    let cells = ui! {
        view(style = move || grid_row(&SIGNAL_TRACKS, false, is_selected.get())) {
            text(style = Mono()) { row.name.clone() }
            text(style = Mono()) { row.value_text() }
            text(style = MonoMuted()) { row.writes.to_string() }
            text(style = Caption()) { last }
        }
    };
    pressable(vec![cells], move || selected.set(Some(id))).into_element()
}

/// The numeric history as bar fractions, or `None` when any value is not
/// a number (the chart only makes sense for numbers).
fn bar_fractions(h: &SignalHistory) -> Option<Vec<f32>> {
    let values: Vec<f64> = h.history.iter().map(|p| p.value.trim().parse::<f64>().ok()).collect::<Option<_>>()?;
    let (min, max) = values.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let span = (max - min).max(f64::EPSILON);
    Some(values.iter().map(|v| if max == min { 1.0 } else { ((v - min) / span) as f32 }).collect())
}

#[component]
fn SignalDetail(snapshot: Signal<Snapshot>, chosen: Option<SignalRow>) -> Element {
    let row = chosen.clone();
    let name = memo(move || row.get().map(|r| r.name).unwrap_or_default());
    let row = chosen.clone();
    let value = memo(move || row.get().map(|r| r.value_text()).unwrap_or_default());
    let row = chosen.clone();
    let writable = memo(move || row.get().map(|r| r.writable).unwrap_or(false));
    let row = chosen.clone();
    let id = memo(move || row.get().map(|r| r.id).unwrap_or(0));
    let row = chosen;
    let subtitle = memo(move || row.get().map(|r| format!("#{} · {} writes", r.id, r.writes)).unwrap_or_default());

    let history = memo(move || snapshot.get().signal_history.filter(|h| h.id == id.get()));
    let bars = memo(move || {
        let fractions = history.get().as_ref().and_then(bar_fractions).unwrap_or_default();
        fractions.into_iter().enumerate().collect::<Vec<(usize, f32)>>()
    });
    let has_bars = memo(move || bars.get().len() > 1);
    let recent = memo(move || {
        let mut points: Vec<(String, String)> = history
            .get()
            .map(|h| h.history.into_iter().map(|p| (p.ago_ms.map(ago).unwrap_or_else(|| "—".into()), p.value)).collect())
            .unwrap_or_default();
        points.reverse(); // newest first
        points.truncate(12);
        points
    });

    let draft = signal(String::new());
    let on_draft = move |v: String| draft.set(v);
    let on_set = move || {
        let raw = draft.get();
        let parsed: Value = serde_json::from_str(raw.trim()).unwrap_or(Value::String(raw));
        crate::action(format!("write {}", name.get()), "write_signal", json!({ "id": id.get(), "value": parsed }));
    };
    let result = memo(move || {
        snapshot.get().last_action.filter(|a| a.label.starts_with("write ")).map(|a| match a.result {
            Ok(()) => (true, format!("set · {} ms", a.rtt_ms)),
            Err(e) => (false, e),
        })
    });
    let ok = memo(move || result.get().map(|(ok, _)| ok) == Some(true));
    let err = memo(move || result.get().map(|(ok, _)| ok) == Some(false));
    let result_text = memo(move || result.get().map(|(_, t)| t).unwrap_or_default());

    ui! {
        view(style = SidePane()) {
            view(style = Column()) {
                text(style = super::styles::ComponentName()) { "{name}" }
                text(style = MonoMuted()) { "{subtitle}" }
            }
            view(style = Column()) {
                text(style = Caption()) { "Current value" }
                text(style = MonoLarge()) { "{value}" }
            }
            if has_bars {
                view(style = Column()) {
                    text(style = SectionTitle()) { "History" }
                    view(style = ChartBox()) {
                        for (_i, f) in bars, key = format!("{_i}:{f}") {
                            view(style = bar(f)) {}
                        }
                    }
                }
            }
            view(style = Column()) {
                text(style = SectionTitle()) { "Recent values" }
                view(style = TableBox()) {
                    for (when, v) in recent, key = format!("{when}={v}") {
                        view(style = grid_row(&HISTORY_TRACKS, false, false)) {
                            text(style = Caption()) { when }
                            text(style = Mono()) { v }
                        }
                    }
                }
            }
            if writable {
                view(style = Column()) {
                    text(style = SectionTitle()) { "Set value" }
                    view(style = RowStart()) {
                        Field(
                            value = draft,
                            on_change = Rc::new(on_draft) as Rc<dyn Fn(String)>,
                            placeholder = Some("JSON, e.g. 4 or \"text\"".to_string()),
                            size = FieldSize::Sm,
                            width = Some(260.0),
                        )
                        Button(label = "Set".to_string(), on_click = Rc::new(on_set) as Rc<dyn Fn()>, size = idea_ui::size::Sm)
                    }
                    if ok {
                        text(style = OkText()) { "{result_text}" }
                    }
                    if err {
                        text(style = ErrorText()) { "{result_text}" }
                    }
                }
            } else {
                text(style = Caption()) { "Read-only. Register it with robot::watch_signal_writable to set it from here." }
            }
        }
    }
}
