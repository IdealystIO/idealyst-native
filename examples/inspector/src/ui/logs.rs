//! Logs & perf: the captured log stream (filter by source and text) beside
//! the phase timers.

use std::collections::BTreeSet;
use std::rc::Rc;

use idea_ui::{tone, typography_kind, variant, Adornment, Button, Chip, ControlSize, Field, FieldSize, Typography};
use runtime_core::{component, memo, signal, ui, Element, Signal, TrackSize};
use serde_json::json;

use super::styles::{
    grid_row, Caption, Column, Main, Mono, MonoMuted, Padded, PaneHeader, RowBetween, RowStart,
    ScrollFill, SectionTitle, SidePane, SplitRow, TableBox,
};
use crate::bridge::model::{micros, LogRow, Perf, Snapshot};

static LOG_TRACKS: [TrackSize; 3] = [TrackSize::Px(96.0), TrackSize::Px(120.0), TrackSize::Fr(1.0)];
static PHASE_TRACKS: [TrackSize; 4] =
    [TrackSize::Fr(1.0), TrackSize::Px(56.0), TrackSize::Px(72.0), TrackSize::Px(64.0)];

/// How many (filtered) log lines render — the newest ones.
const LOG_TAIL: usize = 300;

/// `HH:MM:SS.mmm` in the local time zone.
fn clock(ts_ms: u64) -> String {
    let offset_ms = runtime_core::time::local_offset_minutes() as i64 * 60_000;
    let day_ms = (ts_ms as i64 + offset_ms).rem_euclid(86_400_000);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        day_ms / 3_600_000,
        day_ms / 60_000 % 60,
        day_ms / 1000 % 60,
        day_ms % 1000
    )
}

#[component]
pub fn LogsScreen(snapshot: Signal<Snapshot>) -> Element {
    let search = signal(String::new());
    let hidden: Signal<BTreeSet<String>> = signal(BTreeSet::new());
    let sources = memo(move || {
        snapshot.get().logs.iter().map(|l| l.source.clone()).collect::<BTreeSet<String>>().into_iter().collect::<Vec<_>>()
    });
    let rows = memo(move || {
        let needle = search.get().to_lowercase();
        let hidden = hidden.get();
        let logs = snapshot.get().logs;
        let kept: Vec<LogRow> = logs
            .into_iter()
            .filter(|l| !hidden.contains(&l.source))
            .filter(|l| needle.is_empty() || l.text.to_lowercase().contains(&needle) || l.source.to_lowercase().contains(&needle))
            .collect();
        let start = kept.len().saturating_sub(LOG_TAIL);
        kept[start..].iter().rev().cloned().collect::<Vec<_>>() // newest first
    });
    let empty = memo(move || rows.get().is_empty());
    let on_search = Rc::new(move |v: String| search.set(v)) as Rc<dyn Fn(String)>;
    let clear = Rc::new(|| crate::action("clear logs", "clear_logs", json!({}))) as Rc<dyn Fn()>;

    ui! {
        view(style = SplitRow()) {
            view(style = Main()) {
                view(style = PaneHeader()) {
                    view(style = RowBetween()) {
                        Typography(content = "Logs".to_string(), kind = typography_kind::H3)
                        Button(label = "Clear".to_string(), on_click = clear, size = idea_ui::size::Sm, tone = tone::Neutral, variant = variant::Outlined)
                    }
                    view(style = RowStart()) {
                        for source in sources, key = source.clone() {
                            SourceChip(source = source, hidden = hidden)
                        }
                        Field(
                            value = search,
                            on_change = on_search,
                            placeholder = Some("Search logs".to_string()),
                            size = FieldSize::Sm,
                            leading = Adornment::Icon(icons_lucide::SEARCH),
                            width = Some(280.0),
                        )
                    }
                }
                scroll_view(style = ScrollFill()) {
                    view(style = Padded()) {
                        if empty {
                            text(style = Caption()) { "No log lines captured." }
                        }
                        for l in rows, key = format!("{}:{}:{}", l.ts, l.source, l.text) {
                            view(style = grid_row(&LOG_TRACKS, false, false)) {
                                text(style = MonoMuted()) { clock(l.ts) }
                                text(style = MonoMuted()) { l.source.clone() }
                                text(style = Mono()) { l.text.clone() }
                            }
                        }
                    }
                }
            }
            PerfPane(snapshot = snapshot)
        }
    }
}

/// A toggle chip per log source: selected = shown.
#[component]
fn SourceChip(#[prop(static)] source: String, hidden: Signal<BTreeSet<String>>) -> Element {
    let key = source.clone();
    let shown = memo(move || !hidden.get().contains(&key));
    let key = source.clone();
    let toggle = Rc::new(move || {
        hidden.update(|set| {
            let mut next = set.clone();
            if !next.remove(&key) {
                next.insert(key.clone());
            }
            next
        })
    }) as Rc<dyn Fn()>;
    ui! {
        Chip(label = source, selected = shown, on_select = Some(toggle), tone = tone::Primary, size = ControlSize::Sm)
    }
}

#[component]
fn PerfPane(snapshot: Signal<Snapshot>) -> Element {
    let rows = memo(move || match snapshot.get().perf {
        Perf::Rows(rows) => rows,
        Perf::Unavailable(_) => Vec::new(),
    });
    let hint = memo(move || match snapshot.get().perf {
        Perf::Unavailable(h) => Some(h),
        Perf::Rows(_) => None,
    });
    let unavailable = memo(move || hint.get().is_some());
    let hint_text = memo(move || hint.get().unwrap_or_default());
    let no_rows = memo(move || hint.get().is_none() && rows.get().is_empty());
    let reset = Rc::new(|| crate::action("reset perf", "clear_perf_counters", json!({}))) as Rc<dyn Fn()>;
    ui! {
        view(style = SidePane()) {
            view(style = RowBetween()) {
                Typography(content = "Performance".to_string(), kind = typography_kind::H3)
                Button(label = "Reset".to_string(), on_click = reset, size = idea_ui::size::Sm, tone = tone::Neutral, variant = variant::Outlined)
            }
            view(style = Column()) {
                text(style = SectionTitle()) { "Phase timers · since connect" }
                if unavailable {
                    text(style = Caption()) { "{hint_text}" }
                }
                if no_rows {
                    text(style = Caption()) { "No phases recorded yet." }
                }
                view(style = TableBox()) {
                    view(style = grid_row(&PHASE_TRACKS, true, false)) {
                        text(style = Caption()) { "Phase" }
                        text(style = Caption()) { "Calls" }
                        text(style = Caption()) { "Total" }
                        text(style = Caption()) { "Max" }
                    }
                    for p in rows, key = format!("{p:?}") {
                        view(style = grid_row(&PHASE_TRACKS, false, false)) {
                            text(style = Mono()) { p.phase.clone() }
                            text(style = MonoMuted()) { p.call_count.to_string() }
                            text(style = Mono()) { micros(p.total_us) }
                            text(style = MonoMuted()) { micros(p.max_us) }
                        }
                    }
                }
                text(style = Caption()) { "Timers need the app built with debug-stats, which also slows it: compare runs, not absolute numbers." }
            }
        }
    }
}
