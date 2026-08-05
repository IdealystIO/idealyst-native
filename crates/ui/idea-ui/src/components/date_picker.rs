//! `DatePicker` / `DateTimePicker` / `DateRangePicker` — popup date
//! selection on a `Select`-shaped trigger.
//!
//! ```ignore
//! let due: Signal<Option<CivilDate>> = signal(None);
//! ui! {
//!     DatePicker(
//!         value = due,
//!         on_change = move |d: Option<CivilDate>| due.set(d),
//!         min = CivilDate::today(),
//!     )
//! }
//! ```
//!
//! All three share the same anatomy: a trigger (formatted value or
//! placeholder + calendar glyph, styled by the `SelectTrigger` sheet so
//! pickers sit flush beside `Select`s and `Field`s) opening an anchored
//! panel (`SelectMenu` sheet) with a borderless [`Calendar`] /
//! [`RangeCalendar`] inside. Dismissal mirrors `Select`: a fullscreen
//! transparent catcher closes on any outside press (including a
//! trigger re-press), Escape closes via the anchored overlay's
//! `on_dismiss`.
//!
//! - `DatePicker` commits and closes on day pick; `clearable` adds a
//!   footer action that commits `None`.
//! - `DateTimePicker` keeps the panel open for its time row (a
//!   [`TimeInput`] footer); each pick/edit re-commits the combined
//!   [`CivilDateTime`] live.
//! - `DateRangePicker` closes when the second endpoint completes the
//!   pair.
//!
//! `on_change` receives an `Option` — `None` only ever means an
//! explicit clear, so hosts can bind it straight to their signal.

use std::rc::Rc;

use runtime_core::primitives::overlay::{overlay, BackdropMode};
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide, ViewportPlacement};
use runtime_core::stylesheet;
use runtime_core::{
    component, effect, icon, pressable, signal, text, ui, view, when, Color, Element, FillRule,
    IconData, IdealystSchema, IntoElement, Length, Position, PressableHandle, Reactive, Ref,
    Signal, StyleApplication, StyleSheet, Tokenized, VariantEnum,
};

use crate::components::calendar::{Calendar, RangeCalendar};
use crate::components::select::SelectSize;
use crate::components::time_input::TimeInput;
use crate::date::{
    format_date, CivilDate, CivilDateTime, CivilTime, DateLabels, Weekday,
};
use crate::stylesheets::{SelectMenu, SelectOption as SelectOptionStyle, SelectTrigger};

/// Trailing trigger glyph — a calendar. Inline `IconData` like the
/// `Select` chevron: no icon-pack dependency for built-in affordances.
pub(crate) const CALENDAR_GLYPH: IconData = IconData {
    view_box: (24, 24),
    paths: &[
        "M8 2v4",
        "M16 2v4",
        "M3 10h18",
        "M5 4h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z",
    ],
    fill_rule: FillRule::NonZero,
    filled: false,
};

// Fullscreen transparent dismiss backdrop for the open panel — same
// rationale + `stylesheet!` (constructs on OPEN, so link-time CSS) as
// `select.rs`'s `SelectBackdropSheet`; private there, so restated.
stylesheet! {
    DatePopupBackdropSheet<()> {
        base(_t) {
            position: Position::Absolute,
            top: Length::Px(0.0),
            left: Length::Px(0.0),
            right: Length::Px(0.0),
            bottom: Length::Px(0.0),
            background: Color("transparent".into()),
        }
    }
}

fn popup_backdrop_sheet() -> Rc<StyleSheet> {
    DatePopupBackdropSheet::sheet()
}

/// The trigger: current value (or placeholder) + calendar glyph,
/// `SelectTrigger`-styled with its `open` highlight.
pub(crate) fn picker_trigger(
    label_source: impl Fn() -> String + 'static,
    open: Signal<bool>,
    size: Reactive<SelectSize>,
    trigger_ref: Ref<PressableHandle>,
) -> Element {
    let label_child = text(label_source).into_element();
    let glyph = icon(CALENDAR_GLYPH)
        .size(16.0)
        .color(|| Tokenized::token("color-text-muted", Color("#6b7280".into())).resolve())
        .into_element();

    let trigger_style = move || {
        StyleApplication::new(SelectTrigger::sheet())
            .with("size", size.get().as_variant_str().to_string())
            .with("open", if open.get() { "on" } else { "off" }.to_string())
    };
    // Open on press; the catcher owns closing (an open picker's next
    // click lands on the fullscreen backdrop, wherever it is).
    pressable(vec![label_child, glyph], move || open.set(true))
        .with_style(trigger_style)
        .bind(trigger_ref)
        .into_element()
}

/// The open panel: fullscreen dismiss catcher + a `SelectMenu`-styled
/// column anchored below the trigger. `content` builds the panel body
/// fresh per open.
pub(crate) fn anchored_panel(
    anchor: AnchorTarget,
    content: Vec<Element>,
    close: Rc<dyn Fn()>,
) -> Element {
    let catcher_close = close.clone();
    let catcher = overlay(Vec::new())
        .placement(ViewportPlacement::FullScreen)
        .backdrop(BackdropMode::Dismiss)
        .backdrop_style(StyleApplication::new(popup_backdrop_sheet()))
        .on_dismiss(move || (catcher_close)())
        .into_element();

    let panel = view(content)
        .with_style(StyleApplication::new(SelectMenu::sheet()))
        .into_element();

    let anchored = runtime_core::anchored_overlay(anchor, vec![panel])
        .side(ElementSide::Below)
        .align(ElementAlign::Start)
        .offset(4.0)
        .backdrop(BackdropMode::None)
        .trap_focus(false)
        .on_dismiss(move || (close)())
        .into_element();

    view(vec![catcher, anchored]).into_element()
}

/// A quiet footer action row (`Clear`), `SelectOption`-styled so it
/// reads as part of the menu family.
fn footer_action(label: String, on_press: impl Fn() + 'static) -> Element {
    pressable(vec![text(label).into_element()], on_press)
        .with_style(StyleApplication::new(SelectOptionStyle::sheet()))
        .into_element()
}

// ---------------------------------------------------------------------------
// DatePicker
// ---------------------------------------------------------------------------

// Reactive-by-default routing as in `CalendarProps`; `display_format`
// participates in the trigger's live label closure.
#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct DatePickerProps {
    /// Controlled value. The host owns the signal and writes it from
    /// `on_change`.
    pub value: Signal<Option<CivilDate>>,
    /// Fires with `Some(day)` on pick, `None` on an explicit clear.
    pub on_change: Rc<dyn Fn(Option<CivilDate>)>,
    /// Trigger text when no value is set.
    pub placeholder: Option<String>,
    /// Token format for the trigger's value text. Default `YYYY-MM-DD`.
    pub display_format: String,
    /// Earliest pickable day (inclusive).
    pub min: Option<CivilDate>,
    /// Latest pickable day (inclusive).
    pub max: Option<CivilDate>,
    /// Per-day veto — see [`CalendarProps::is_date_disabled`](crate::components::calendar::CalendarProps::is_date_disabled).
    pub is_date_disabled: Option<Rc<dyn Fn(CivilDate) -> bool>>,
    /// First calendar column. Default Monday (ISO).
    pub first_weekday: Weekday,
    /// Month/weekday display names. Default English.
    #[prop(static)]
    pub labels: Option<Rc<DateLabels>>,
    /// Trigger height — shared scale with `Select`. Default Md.
    pub size: SelectSize,
    /// Offer a `Clear` footer action that commits `None`.
    pub clearable: bool,
}

impl Default for DatePickerProps {
    fn default() -> Self {
        Self {
            value: runtime_core::signal(None),
            on_change: Rc::new(|_| {}),
            placeholder: Reactive::Static(None),
            display_format: Reactive::Static("YYYY-MM-DD".to_string()),
            min: Reactive::Static(None),
            max: Reactive::Static(None),
            is_date_disabled: None,
            first_weekday: Reactive::Static(Weekday::Monday),
            labels: None,
            size: Reactive::Static(SelectSize::default()),
            clearable: Reactive::Static(false),
        }
    }
}

/// Popup single-date picker. See the module docs.
#[component]
pub fn DatePicker(props: DatePickerProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();

    let open: Signal<bool> = signal(false);
    let trigger_ref: Ref<PressableHandle> = Ref::new();

    let fmt = props.display_format.clone();
    let placeholder = props.placeholder.clone();
    let label_source = move || {
        value
            .get()
            .map(|d| format_date(d, &fmt.get()))
            .or_else(|| placeholder.get())
            .unwrap_or_else(|| "Select date".to_string())
    };
    let trigger = picker_trigger(label_source, open, props.size.clone(), trigger_ref);

    let min = props.min.clone();
    let max = props.max.clone();
    let is_date_disabled = props.is_date_disabled.clone();
    let first_weekday = props.first_weekday.clone();
    let labels = props.labels.clone();
    let clearable = props.clearable.clone();

    let popup = when(
        move || open.get(),
        move || {
            let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

            let pick_close = close.clone();
            let pick_commit = on_change.clone();
            let calendar = ui! {
                Calendar(
                    value = value,
                    on_change = Rc::new(move |d: CivilDate| {
                        (pick_commit)(Some(d));
                        (pick_close)();
                    }) as Rc<dyn Fn(CivilDate)>,
                    min = min.clone(),
                    max = max.clone(),
                    is_date_disabled = is_date_disabled.clone(),
                    first_weekday = first_weekday.clone(),
                    labels = labels.clone(),
                    framed = false,
                )
            };

            let mut content = vec![calendar];
            if clearable.get() {
                let clear_close = close.clone();
                let clear_commit = on_change.clone();
                content.push(footer_action("Clear".to_string(), move || {
                    (clear_commit)(None);
                    (clear_close)();
                }));
            }
            anchored_panel(AnchorTarget::from(trigger_ref), content, close)
        },
        runtime_core::empty_absolute_view,
    );

    ui! {
        view {
            trigger
            popup
        }
    }
}

// ---------------------------------------------------------------------------
// DateTimePicker
// ---------------------------------------------------------------------------

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct DateTimePickerProps {
    /// Controlled value (date + time of day).
    pub value: Signal<Option<CivilDateTime>>,
    /// Fires with the combined value on every pick/edit; `None` on
    /// clear.
    pub on_change: Rc<dyn Fn(Option<CivilDateTime>)>,
    /// Trigger text when no value is set.
    pub placeholder: Option<String>,
    /// Token format for the trigger. Default `YYYY-MM-DD HH:mm`.
    pub display_format: String,
    /// Token format for the panel's time row. Default `HH:mm` —
    /// pass `"h:mm A"` for a 12-hour clock.
    pub time_format: String,
    /// Time used when a day is picked before any time is set.
    pub default_time: CivilTime,
    /// Earliest pickable day (inclusive).
    pub min: Option<CivilDate>,
    /// Latest pickable day (inclusive).
    pub max: Option<CivilDate>,
    /// Per-day veto.
    pub is_date_disabled: Option<Rc<dyn Fn(CivilDate) -> bool>>,
    /// First calendar column. Default Monday (ISO).
    pub first_weekday: Weekday,
    /// Month/weekday display names. Default English.
    #[prop(static)]
    pub labels: Option<Rc<DateLabels>>,
    /// Trigger height. Default Md.
    pub size: SelectSize,
    /// Offer a `Clear` footer action.
    pub clearable: bool,
}

impl Default for DateTimePickerProps {
    fn default() -> Self {
        Self {
            value: runtime_core::signal(None),
            on_change: Rc::new(|_| {}),
            placeholder: Reactive::Static(None),
            display_format: Reactive::Static("YYYY-MM-DD HH:mm".to_string()),
            time_format: Reactive::Static("HH:mm".to_string()),
            default_time: Reactive::Static(CivilTime::MIDNIGHT),
            min: Reactive::Static(None),
            max: Reactive::Static(None),
            is_date_disabled: None,
            first_weekday: Reactive::Static(Weekday::Monday),
            labels: None,
            size: Reactive::Static(SelectSize::default()),
            clearable: Reactive::Static(false),
        }
    }
}

/// Popup date+time picker: a calendar with a time row. Stays open
/// across picks (unlike [`DatePicker`]) so the user can set both parts;
/// outside click / Escape closes. See the module docs.
#[component]
pub fn DateTimePicker(props: DateTimePickerProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();

    let open: Signal<bool> = signal(false);
    let trigger_ref: Ref<PressableHandle> = Ref::new();

    // The two halves the panel edits. Component-local; hosts only see
    // combined commits. Initialized from the value, kept in sync with
    // external writes below.
    let date_part: Signal<Option<CivilDate>> = signal(value.peek().map(|dt| dt.date));
    let time_part: Signal<Option<CivilTime>> = signal(Some(
        value.peek().map(|dt| dt.time).unwrap_or(props.default_time.get()),
    ));
    let default_time = props.default_time.clone();
    effect!({
        if let Some(dt) = value.get() {
            date_part.set(Some(dt.date));
            time_part.set(Some(dt.time));
        } else {
            date_part.set(None);
        }
    });

    let fmt = props.display_format.clone();
    let placeholder = props.placeholder.clone();
    let label_source = move || {
        value
            .get()
            .map(|dt| crate::date::format_datetime(dt, &fmt.get()))
            .or_else(|| placeholder.get())
            .unwrap_or_else(|| "Select date & time".to_string())
    };
    let trigger = picker_trigger(label_source, open, props.size.clone(), trigger_ref);

    let min = props.min.clone();
    let max = props.max.clone();
    let is_date_disabled = props.is_date_disabled.clone();
    let first_weekday = props.first_weekday.clone();
    let labels = props.labels.clone();
    let clearable = props.clearable.clone();
    let time_format = props.time_format.clone();

    let popup = when(
        move || open.get(),
        move || {
            let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

            // Combined commit — from either half.
            let commit = {
                let on_change = on_change.clone();
                let default_time = default_time.clone();
                Rc::new(move || {
                    if let Some(d) = date_part.peek() {
                        let t = time_part.peek().unwrap_or(default_time.get());
                        (on_change)(Some(CivilDateTime::new(d, t)));
                    }
                })
            };

            let pick_commit = commit.clone();
            let calendar = ui! {
                Calendar(
                    value = date_part,
                    on_change = Rc::new(move |d: CivilDate| {
                        date_part.set(Some(d));
                        (pick_commit)();
                    }) as Rc<dyn Fn(CivilDate)>,
                    min = min.clone(),
                    max = max.clone(),
                    is_date_disabled = is_date_disabled.clone(),
                    first_weekday = first_weekday.clone(),
                    labels = labels.clone(),
                    framed = false,
                )
            };

            let time_commit = commit.clone();
            let time_row = ui! {
                TimeInput(
                    value = time_part,
                    on_change = Rc::new(move |t: Option<CivilTime>| {
                        if let Some(t) = t {
                            time_part.set(Some(t));
                            (time_commit)();
                        }
                    }) as Rc<dyn Fn(Option<CivilTime>)>,
                    format = time_format.clone(),
                )
            };

            let mut content = vec![calendar, time_row];
            if clearable.get() {
                let clear_close = close.clone();
                let clear_commit = on_change.clone();
                content.push(footer_action("Clear".to_string(), move || {
                    (clear_commit)(None);
                    (clear_close)();
                }));
            }
            anchored_panel(AnchorTarget::from(trigger_ref), content, close)
        },
        runtime_core::empty_absolute_view,
    );

    ui! {
        view {
            trigger
            popup
        }
    }
}

// ---------------------------------------------------------------------------
// DateRangePicker
// ---------------------------------------------------------------------------

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct DateRangePickerProps {
    /// Controlled committed range, chronologically ordered.
    pub value: Signal<Option<(CivilDate, CivilDate)>>,
    /// Fires with `Some((start, end))` when the pair completes; `None`
    /// on clear.
    pub on_change: Rc<dyn Fn(Option<(CivilDate, CivilDate)>)>,
    /// Trigger text when no range is set.
    pub placeholder: Option<String>,
    /// Token format for EACH endpoint in the trigger text. Default
    /// `YYYY-MM-DD` (rendered `start – end`).
    pub display_format: String,
    /// Earliest pickable day (inclusive).
    pub min: Option<CivilDate>,
    /// Latest pickable day (inclusive).
    pub max: Option<CivilDate>,
    /// Per-day veto.
    pub is_date_disabled: Option<Rc<dyn Fn(CivilDate) -> bool>>,
    /// First calendar column. Default Monday (ISO).
    pub first_weekday: Weekday,
    /// Month/weekday display names. Default English.
    #[prop(static)]
    pub labels: Option<Rc<DateLabels>>,
    /// Trigger height. Default Md.
    pub size: SelectSize,
    /// Offer a `Clear` footer action.
    pub clearable: bool,
}

impl Default for DateRangePickerProps {
    fn default() -> Self {
        Self {
            value: runtime_core::signal(None),
            on_change: Rc::new(|_| {}),
            placeholder: Reactive::Static(None),
            display_format: Reactive::Static("YYYY-MM-DD".to_string()),
            min: Reactive::Static(None),
            max: Reactive::Static(None),
            is_date_disabled: None,
            first_weekday: Reactive::Static(Weekday::Monday),
            labels: None,
            size: Reactive::Static(SelectSize::default()),
            clearable: Reactive::Static(false),
        }
    }
}

/// Popup date-range picker: first press anchors the start, the second
/// commits `(start, end)` and closes. See the module docs.
#[component]
pub fn DateRangePicker(props: DateRangePickerProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();

    let open: Signal<bool> = signal(false);
    let trigger_ref: Ref<PressableHandle> = Ref::new();

    let fmt = props.display_format.clone();
    let placeholder = props.placeholder.clone();
    let label_source = move || {
        value
            .get()
            .map(|(a, b)| {
                let f = fmt.get();
                format!("{} – {}", format_date(a, &f), format_date(b, &f))
            })
            .or_else(|| placeholder.get())
            .unwrap_or_else(|| "Select range".to_string())
    };
    let trigger = picker_trigger(label_source, open, props.size.clone(), trigger_ref);

    let min = props.min.clone();
    let max = props.max.clone();
    let is_date_disabled = props.is_date_disabled.clone();
    let first_weekday = props.first_weekday.clone();
    let labels = props.labels.clone();
    let clearable = props.clearable.clone();

    let popup = when(
        move || open.get(),
        move || {
            let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

            let pick_close = close.clone();
            let pick_commit = on_change.clone();
            let calendar = ui! {
                RangeCalendar(
                    value = value,
                    on_change = Rc::new(move |a: CivilDate, b: CivilDate| {
                        (pick_commit)(Some((a, b)));
                        (pick_close)();
                    }) as Rc<dyn Fn(CivilDate, CivilDate)>,
                    min = min.clone(),
                    max = max.clone(),
                    is_date_disabled = is_date_disabled.clone(),
                    first_weekday = first_weekday.clone(),
                    labels = labels.clone(),
                    framed = false,
                )
            };

            let mut content = vec![calendar];
            if clearable.get() {
                let clear_close = close.clone();
                let clear_commit = on_change.clone();
                content.push(footer_action("Clear".to_string(), move || {
                    (clear_commit)(None);
                    (clear_close)();
                }));
            }
            anchored_panel(AnchorTarget::from(trigger_ref), content, close)
        },
        runtime_core::empty_absolute_view,
    );

    ui! {
        view {
            trigger
            popup
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};

    // The popup panel + backdrop construct on OPEN — the premint crawl
    // never gets there. The panel reuses SelectMenu (premint pinned in
    // select.rs); the backdrop sheet is ours, so pin it here.
    #[test]
    fn popup_backdrop_sheet_premints() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            assert!(
                StyleApplication::new(popup_backdrop_sheet()).preminted_class_list().is_some(),
                "the popup backdrop must premint (it constructs on open)"
            );
        });
    }
}

runtime_core::recipe!(
    DatePicker,
    /// Deadline picker bounded to today-or-later, clearable. The host owns
    /// the signal; `on_change` receives `None` only from the Clear action,
    /// so binding it straight to the signal is the whole wiring.
    pub fn date_picker_deadline() -> ::runtime_core::Element {
        use crate::components::date_picker::DatePicker;
        use crate::date::CivilDate;
        use ::runtime_core::{signal, ui, Signal};
        use ::std::rc::Rc;

        let due: Signal<Option<CivilDate>> = signal(None);
        let on_change: Rc<dyn Fn(Option<CivilDate>)> = Rc::new(move |d| due.set(d));
        ui! {
            DatePicker(
                value = due,
                on_change = on_change,
                min = CivilDate::today(),
                placeholder = "Pick a deadline".to_string(),
                clearable = true,
            )
        }
    }
);

runtime_core::recipe!(
    DateRangePicker,
    /// Report-period range: first press anchors the start, second commits
    /// the ordered pair and closes.
    pub fn date_range_picker_period() -> ::runtime_core::Element {
        use crate::components::date_picker::DateRangePicker;
        use crate::date::CivilDate;
        use ::runtime_core::{signal, ui, Signal};
        use ::std::rc::Rc;

        let period: Signal<Option<(CivilDate, CivilDate)>> = signal(None);
        let on_change: Rc<dyn Fn(Option<(CivilDate, CivilDate)>)> =
            Rc::new(move |r| period.set(r));
        ui! {
            DateRangePicker(
                value = period,
                on_change = on_change,
                placeholder = "Report period".to_string(),
            )
        }
    }
);
