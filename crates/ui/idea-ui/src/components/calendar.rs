//! `Calendar` / `RangeCalendar` — inline month-grid date selection.
//!
//! ```ignore
//! let picked: Signal<Option<CivilDate>> = signal(None);
//! ui! {
//!     Calendar(
//!         value = picked,
//!         on_change = move |d: CivilDate| picked.set(Some(d)),
//!         min = CivilDate::new(2026, 1, 1),
//!     )
//! }
//! ```
//!
//! Both components render the same chrome: a header (prev / title /
//! next), a Mon–Sun column row, and a fixed 6-week day grid (stable
//! height across months; adjacent-month days render muted and are
//! pickable). Pressing the title zooms out — Days → Months → Years —
//! and picking a month/year zooms back in, so reaching a birth year
//! never takes sixty clicks.
//!
//! `RangeCalendar` selects a `(start, end)` pair: the first press
//! anchors a pending start (shown selected), the second commits the
//! pair ordered chronologically and fires `on_change`.
//!
//! Selection state lives in the host's signal (controlled, like
//! `Select`); the visible month is component-local. Reactivity: the
//! grid REBUILDS only when the visible month / zoom / min / max
//! change ([`switch`] keyed on exactly that tuple); a selection change
//! only re-resolves the affected cells' styles (each cell's style
//! closure reads the selection live).

use std::rc::Rc;

use runtime_core::{
    component, effect, pressable, signal, switch, text, ui, view, Element, FillRule, IconData,
    IdealystSchema, IntoElement, Signal, StyleApplication,
};

use idea_theme::extensible::{tone, variant};

use crate::components::icon_button::{IconButton, IconButtonSize};
use crate::date::{CivilDate, DateLabels, Weekday};
use crate::stylesheets::{
    CalendarDay, CalendarHeader, CalendarPanel, CalendarTitleButton, CalendarWeekRow,
    CalendarWeekdayCell, CalendarZoomCell,
};

/// Header nav chevrons (same inline-`IconData` shape as `Select`'s
/// trigger chevron — no icon-pack dependency for built-in affordances).
pub(crate) const CHEVRON_LEFT: IconData = IconData {
    view_box: (24, 24),
    paths: &["M15 18l-6-6 6-6"],
    fill_rule: FillRule::NonZero,
    filled: false,
};
pub(crate) const CHEVRON_RIGHT: IconData = IconData {
    view_box: (24, 24),
    paths: &["M9 18l6-6-6-6"],
    fill_rule: FillRule::NonZero,
    filled: false,
};

/// Selection role of one day cell — maps onto the `CalendarDay` sheet's
/// `sel` axis.
#[derive(Copy, Clone, PartialEq)]
pub(crate) enum DaySel {
    Off,
    /// The single selection or a range endpoint (solid fill).
    On,
    /// Interior of a committed range (soft band).
    Mid,
}

/// Zoom level of the picker body. Title press cycles Days → Months →
/// Years → Days; picking a year/month steps back toward Days.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum Zoom {
    Days,
    Months,
    Years,
}

/// First cell of the 6×7 grid showing `(year, month)` with columns
/// starting on `first_weekday` — the last `first_weekday` on or before
/// the 1st of the month. Pure; unit-tested below.
pub(crate) fn month_grid_start(year: i32, month: u8, first_weekday: Weekday) -> CivilDate {
    let first = CivilDate { year, month, day: 1 };
    let offset = i64::from(
        (i16::from(first.weekday().as_index()) - i16::from(first_weekday.as_index()))
            .rem_euclid(7),
    );
    first.add_days(-offset)
}

/// Everything the shared chrome needs, bundled once — `Calendar` and
/// `RangeCalendar` differ only in `sel_of` (how a cell classifies
/// itself) and `on_pick` (what committing a day means).
pub(crate) struct CalendarCore {
    /// `(year, month)` shown by the day grid.
    pub visible: Signal<(i32, u8)>,
    pub min: runtime_core::Reactive<Option<CivilDate>>,
    pub max: runtime_core::Reactive<Option<CivilDate>>,
    pub is_date_disabled: Option<Rc<dyn Fn(CivilDate) -> bool>>,
    pub first_weekday: Weekday,
    pub labels: Rc<DateLabels>,
    /// Reads selection signals LIVE (called from cell style closures).
    pub sel_of: Rc<dyn Fn(CivilDate) -> DaySel>,
    pub on_pick: Rc<dyn Fn(CivilDate)>,
    pub framed: bool,
}

impl CalendarCore {
    fn blocked(&self, min: Option<CivilDate>, max: Option<CivilDate>, d: CivilDate) -> bool {
        min.is_some_and(|m| d < m)
            || max.is_some_and(|m| d > m)
            || self.is_date_disabled.as_ref().is_some_and(|f| f(d))
    }
}

/// Build the calendar chrome around a [`CalendarCore`].
pub(crate) fn calendar_view(core: Rc<CalendarCore>) -> Element {
    let zoom: Signal<Zoom> = signal(Zoom::Days);
    let visible = core.visible;

    // --- header -----------------------------------------------------------
    // Prev/next step by what the body currently shows: a month of days, a
    // year of months, or a 12-year page.
    let step = |dir: i32| {
        let core = core.clone();
        move || {
            let (y, m) = visible.peek();
            match zoom.peek() {
                Zoom::Days => {
                    let d = CivilDate { year: y, month: m, day: 1 }.add_months(dir);
                    visible.set((d.year, d.month));
                }
                Zoom::Months => visible.set((y + dir, m)),
                Zoom::Years => visible.set((y + dir * 12, m)),
            }
            let _ = &core; // keep the core alive as long as the handler
        }
    };
    let on_prev: Rc<dyn Fn()> = Rc::new(step(-1));
    let on_next: Rc<dyn Fn()> = Rc::new(step(1));

    let title_labels = core.labels.clone();
    let title_source = move || {
        let (y, m) = visible.get();
        match zoom.get() {
            Zoom::Days => format!("{} {}", title_labels.month_name(m), y),
            Zoom::Months => y.to_string(),
            Zoom::Years => {
                let base = y - i32::rem_euclid(y, 12);
                format!("{base} – {}", base + 11)
            }
        }
    };
    let on_title = move || {
        zoom.set(match zoom.peek() {
            Zoom::Days => Zoom::Months,
            Zoom::Months => Zoom::Years,
            Zoom::Years => Zoom::Days,
        })
    };
    let title = pressable(vec![text(title_source).into_element()], on_title)
        .with_style(StyleApplication::new(CalendarTitleButton::sheet()))
        .into_element();

    let header_children = vec![
        ui! {
            IconButton(
                icon = Some(CHEVRON_LEFT),
                on_click = on_prev,
                tone = tone::Neutral,
                variant = variant::Ghost,
                size = IconButtonSize::Sm,
            )
        },
        title,
        ui! {
            IconButton(
                icon = Some(CHEVRON_RIGHT),
                on_click = on_next,
                tone = tone::Neutral,
                variant = variant::Ghost,
                size = IconButtonSize::Sm,
            )
        },
    ];
    let header = view(header_children)
        .with_style(StyleApplication::new(CalendarHeader::sheet()))
        .into_element();

    // --- body -------------------------------------------------------------
    // Keyed on everything the grid's STRUCTURE depends on. Selection is
    // deliberately absent: it only affects cell styles, which re-resolve
    // live without a rebuild (and without remounting pressables mid-drag).
    let body_core = core.clone();
    let body = switch(
        {
            let core = core.clone();
            move || (visible.get(), zoom.get(), core.min.get(), core.max.get())
        },
        move |(vis, zoom_now, min, max): &((i32, u8), Zoom, Option<CivilDate>, Option<CivilDate>)| {
            match zoom_now {
                Zoom::Days => day_grid(&body_core, *vis, *min, *max),
                Zoom::Months => month_grid(&body_core, *vis, zoom),
                Zoom::Years => year_grid(&body_core, *vis, zoom),
            }
        },
    );

    let framed = core.framed;
    let panel_style = move || {
        StyleApplication::new(CalendarPanel::sheet())
            .with("framed", if framed { "on" } else { "off" }.to_string())
    };
    ui! {
        view(style = panel_style) {
            header
            body
        }
    }
}

/// The Days body: weekday column headers + six 7-cell week rows.
fn day_grid(
    core: &Rc<CalendarCore>,
    (year, month): (i32, u8),
    min: Option<CivilDate>,
    max: Option<CivilDate>,
) -> Element {
    let today = CivilDate::today();

    let mut header_cells: Vec<Element> = Vec::with_capacity(7);
    for i in 0..7u8 {
        let wd = core.first_weekday.add(i);
        header_cells.push(
            text(core.labels.weekday_short(wd).to_string())
                .with_style(StyleApplication::new(CalendarWeekdayCell::sheet()))
                .into_element(),
        );
    }
    let mut rows: Vec<Element> = Vec::with_capacity(7);
    rows.push(
        view(header_cells)
            .with_style(StyleApplication::new(CalendarWeekRow::sheet()))
            .into_element(),
    );

    let mut d = month_grid_start(year, month, core.first_weekday);
    for _week in 0..6 {
        let mut cells: Vec<Element> = Vec::with_capacity(7);
        for _col in 0..7 {
            cells.push(day_cell(core, d, month, today, core.blocked(min, max, d)));
            d = d.add_days(1);
        }
        rows.push(
            view(cells)
                .with_style(StyleApplication::new(CalendarWeekRow::sheet()))
                .into_element(),
        );
    }
    view(rows).into_element()
}

fn day_cell(
    core: &Rc<CalendarCore>,
    d: CivilDate,
    visible_month: u8,
    today: CivilDate,
    blocked: bool,
) -> Element {
    let sel_of = core.sel_of.clone();
    let style = move || {
        StyleApplication::new(CalendarDay::sheet())
            .with(
                "sel",
                match sel_of(d) {
                    DaySel::Off => "off",
                    DaySel::On => "on",
                    DaySel::Mid => "mid",
                }
                .to_string(),
            )
            .with("today", if d == today { "on" } else { "off" }.to_string())
            .with("muted", if d.month != visible_month { "on" } else { "off" }.to_string())
            .with("blocked", if blocked { "on" } else { "off" }.to_string())
    };
    let label = text(d.day.to_string()).into_element();
    if blocked {
        // A blocked day is inert — no press handler at all (a silent no-op
        // pressable would still hit-test and confuse event routing; see
        // CLAUDE.md §9.6).
        view(vec![label]).with_style(style).into_element()
    } else {
        let on_pick = core.on_pick.clone();
        let visible = core.visible;
        pressable(vec![label], move || {
            // Picking an adjacent-month day also navigates to it.
            if d.month != visible_month {
                visible.set((d.year, d.month));
            }
            (on_pick)(d);
        })
        .with_style(style)
        .into_element()
    }
}

/// The Months body: a 4×3 grid of short month names for `year`.
fn month_grid(core: &Rc<CalendarCore>, (_year, month): (i32, u8), zoom: Signal<Zoom>) -> Element {
    let visible = core.visible;
    let mut rows: Vec<Element> = Vec::with_capacity(4);
    for row in 0..4u8 {
        let mut cells: Vec<Element> = Vec::with_capacity(3);
        for col in 0..3u8 {
            let m = row * 3 + col + 1;
            let label = core.labels.months_short[usize::from(m - 1)].clone();
            let style = move || {
                StyleApplication::new(CalendarZoomCell::sheet())
                    .with("active", if m == month { "on" } else { "off" }.to_string())
            };
            cells.push(
                pressable(vec![text(label).into_element()], move || {
                    let (y, _) = visible.peek();
                    visible.set((y, m));
                    zoom.set(Zoom::Days);
                })
                .with_style(style)
                .into_element(),
            );
        }
        rows.push(
            view(cells)
                .with_style(StyleApplication::new(CalendarWeekRow::sheet()))
                .into_element(),
        );
    }
    view(rows).into_element()
}

/// The Years body: the 12-year page containing `year`, aligned to a
/// 12-year boundary so paging is stable.
fn year_grid(core: &Rc<CalendarCore>, (year, _month): (i32, u8), zoom: Signal<Zoom>) -> Element {
    let visible = core.visible;
    let base = year - i32::rem_euclid(year, 12);
    let mut rows: Vec<Element> = Vec::with_capacity(4);
    for row in 0..4i32 {
        let mut cells: Vec<Element> = Vec::with_capacity(3);
        for col in 0..3i32 {
            let y = base + row * 3 + col;
            let style = move || {
                StyleApplication::new(CalendarZoomCell::sheet())
                    .with("active", if y == year { "on" } else { "off" }.to_string())
            };
            cells.push(
                pressable(vec![text(y.to_string()).into_element()], move || {
                    let (_, m) = visible.peek();
                    visible.set((y, m));
                    zoom.set(Zoom::Months);
                })
                .with_style(style)
                .into_element(),
            );
        }
        rows.push(
            view(cells)
                .with_style(StyleApplication::new(CalendarWeekRow::sheet()))
                .into_element(),
        );
    }
    view(rows).into_element()
}

/// The `(year, month)` a calendar should open on: the selection if any,
/// else today.
pub(crate) fn initial_visible(selected: Option<CivilDate>) -> (i32, u8) {
    let d = selected.unwrap_or_else(CivilDate::today);
    (d.year, d.month)
}

// ---------------------------------------------------------------------------
// Calendar — single date
// ---------------------------------------------------------------------------

// Reactive-by-default: `min`/`max`/`first_weekday`/`framed` are DATA
// (`Reactive<T>`); AUTO-SKIPPED: `value` (Signal source), `on_change` /
// `is_date_disabled` (handlers). `labels` is structural (name lookups
// happen at grid build) — `#[prop(static)]`.
#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct CalendarProps {
    /// Controlled selection. The host owns the signal; picking a day
    /// fires `on_change` (the host writes the signal, like `Select`).
    pub value: Signal<Option<CivilDate>>,
    /// Fires with the picked day.
    pub on_change: Rc<dyn Fn(CivilDate)>,
    /// Earliest pickable day (inclusive). Earlier days render blocked.
    pub min: Option<CivilDate>,
    /// Latest pickable day (inclusive).
    pub max: Option<CivilDate>,
    /// Per-day veto (e.g. weekends, fully-booked days). Blocked days
    /// render dimmed and take no press handler.
    pub is_date_disabled: Option<Rc<dyn Fn(CivilDate) -> bool>>,
    /// First column of the grid. Default Monday (ISO).
    pub first_weekday: Weekday,
    /// Month/weekday display names. Default English — pass your own
    /// [`DateLabels`] to localize.
    #[prop(static)]
    pub labels: Option<Rc<DateLabels>>,
    /// Draw the panel border. Turn off when embedding in a popup that
    /// already has chrome (the pickers do).
    pub framed: bool,
}

impl Default for CalendarProps {
    fn default() -> Self {
        Self {
            value: runtime_core::signal(None),
            on_change: Rc::new(|_| {}),
            min: runtime_core::Reactive::Static(None),
            max: runtime_core::Reactive::Static(None),
            is_date_disabled: None,
            first_weekday: runtime_core::Reactive::Static(Weekday::Monday),
            labels: None,
            framed: runtime_core::Reactive::Static(true),
        }
    }
}

/// Inline single-date month calendar. See the module docs.
#[component]
pub fn Calendar(props: CalendarProps) -> Element {
    let value = props.value;
    let visible = signal(initial_visible(value.peek()));

    // Follow EXTERNAL selection writes (a typed commit in `DateInput`,
    // a programmatic set) to their month. `peek` on `visible` keeps the
    // effect off the nav signal — stepping months must not snap back.
    effect!({
        if let Some(d) = value.get() {
            if visible.peek() != (d.year, d.month) {
                visible.set((d.year, d.month));
            }
        }
    });

    let core = CalendarCore {
        visible,
        min: props.min.clone(),
        max: props.max.clone(),
        is_date_disabled: props.is_date_disabled.clone(),
        first_weekday: props.first_weekday.get(),
        labels: props.labels.clone().unwrap_or_else(DateLabels::english),
        sel_of: Rc::new(move |d| {
            if value.get() == Some(d) {
                DaySel::On
            } else {
                DaySel::Off
            }
        }),
        on_pick: props.on_change.clone(),
        framed: props.framed.get(),
    };
    calendar_view(Rc::new(core))
}

// ---------------------------------------------------------------------------
// RangeCalendar — (start, end) pair
// ---------------------------------------------------------------------------

// Same routing rationale as `CalendarProps`.
#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct RangeCalendarProps {
    /// Controlled committed range (chronologically ordered). The host
    /// owns the signal; a completed pick fires `on_change`.
    pub value: Signal<Option<(CivilDate, CivilDate)>>,
    /// Fires with `(start, end)`, ordered, when the second endpoint is
    /// picked.
    pub on_change: Rc<dyn Fn(CivilDate, CivilDate)>,
    /// Earliest pickable day (inclusive).
    pub min: Option<CivilDate>,
    /// Latest pickable day (inclusive).
    pub max: Option<CivilDate>,
    /// Per-day veto — see [`CalendarProps::is_date_disabled`].
    pub is_date_disabled: Option<Rc<dyn Fn(CivilDate) -> bool>>,
    /// First column of the grid. Default Monday (ISO).
    pub first_weekday: Weekday,
    /// Month/weekday display names. Default English.
    #[prop(static)]
    pub labels: Option<Rc<DateLabels>>,
    /// Panel border — see [`CalendarProps::framed`].
    pub framed: bool,
}

impl Default for RangeCalendarProps {
    fn default() -> Self {
        Self {
            value: runtime_core::signal(None),
            on_change: Rc::new(|_, _| {}),
            min: runtime_core::Reactive::Static(None),
            max: runtime_core::Reactive::Static(None),
            is_date_disabled: None,
            first_weekday: runtime_core::Reactive::Static(Weekday::Monday),
            labels: None,
            framed: runtime_core::Reactive::Static(true),
        }
    }
}

/// Inline range calendar: first press anchors the start, second press
/// commits the ordered pair. See the module docs.
#[component]
pub fn RangeCalendar(props: RangeCalendarProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    // The in-progress first endpoint. Component-local: a half-picked
    // range is UI state, not a committed value.
    let pending: Signal<Option<CivilDate>> = signal(None);

    let visible = signal(initial_visible(value.peek().map(|(s, _)| s)));
    effect!({
        if let Some((s, _)) = value.get() {
            if visible.peek() != (s.year, s.month) {
                visible.set((s.year, s.month));
            }
        }
    });

    let core = CalendarCore {
        visible,
        min: props.min.clone(),
        max: props.max.clone(),
        is_date_disabled: props.is_date_disabled.clone(),
        first_weekday: props.first_weekday.get(),
        labels: props.labels.clone().unwrap_or_else(DateLabels::english),
        sel_of: Rc::new(move |d| {
            // A pending anchor takes over the display — the old committed
            // range would otherwise show two competing selections.
            if let Some(s) = pending.get() {
                return if d == s { DaySel::On } else { DaySel::Off };
            }
            match value.get() {
                Some((a, b)) if d == a || d == b => DaySel::On,
                Some((a, b)) if a < d && d < b => DaySel::Mid,
                _ => DaySel::Off,
            }
        }),
        on_pick: Rc::new(move |d| match pending.peek() {
            None => pending.set(Some(d)),
            Some(s) => {
                pending.set(None);
                let (lo, hi) = if d < s { (d, s) } else { (s, d) };
                (on_change)(lo, hi);
            }
        }),
        framed: props.framed.get(),
    };
    calendar_view(Rc::new(core))
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::resolve_style;

    #[test]
    fn grid_start_lands_on_the_first_weekday_on_or_before_the_1st() {
        // Aug 2026: the 1st is a Saturday → Monday-first grid starts Jul 27.
        assert_eq!(
            month_grid_start(2026, 8, Weekday::Monday),
            CivilDate::new(2026, 7, 27).unwrap()
        );
        // Sunday-first grid starts Jul 26.
        assert_eq!(
            month_grid_start(2026, 8, Weekday::Sunday),
            CivilDate::new(2026, 7, 26).unwrap()
        );
        // Jun 2026: the 1st IS a Monday → the grid starts on it exactly.
        assert_eq!(
            month_grid_start(2026, 6, Weekday::Monday),
            CivilDate::new(2026, 6, 1).unwrap()
        );
        // Grid always spans 42 days; day 42 must be in the next month.
        let start = month_grid_start(2026, 8, Weekday::Monday);
        assert_eq!(start.add_days(41).month, 9);
    }

    #[test]
    fn day_sheet_selection_arms_resolve_distinctly() {
        with_test_world(|| {
            install_idea_theme(light_theme());

            let on = resolve_style(
                &StyleApplication::new(CalendarDay::sheet()).with("sel", "on".to_string()),
            );
            let mid = resolve_style(
                &StyleApplication::new(CalendarDay::sheet()).with("sel", "mid".to_string()),
            );
            let off = resolve_style(&StyleApplication::new(CalendarDay::sheet()));

            // Endpoint = solid fill; interior = a different (soft) wash;
            // resting cells are transparent. If these collapse the range
            // band becomes unreadable.
            assert_ne!(on.background, mid.background);
            assert_ne!(on.background, off.background);
            // The interior band squares its corners so consecutive cells
            // read as one strip.
            assert_eq!(
                mid.border_top_left_radius.clone().map(|r| r.resolve()),
                Some(runtime_core::Length::Px(0.0))
            );
        });
    }

    #[test]
    fn day_sheet_today_ring_survives_selection() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            // `today on` + `sel on` must keep the selected fill (the today
            // marker is a ring, not a competing background).
            let both = resolve_style(
                &StyleApplication::new(CalendarDay::sheet())
                    .with("sel", "on".to_string())
                    .with("today", "on".to_string()),
            );
            let sel_only = resolve_style(
                &StyleApplication::new(CalendarDay::sheet()).with("sel", "on".to_string()),
            );
            assert_eq!(both.background, sel_only.background);
            assert!(both.border_top_color.is_some(), "today draws its ring");
        });
    }

    #[test]
    fn calendar_sheets_premint() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            // The calendar first constructs when a picker OPENS — the
            // premint dump's crawl never gets there, so only build-time
            // CSS can style it under `--premint-only` (same constraint as
            // SelectMenu; see select.rs's premint test).
            for (name, sheet) in [
                ("CalendarPanel", CalendarPanel::sheet()),
                ("CalendarDay", CalendarDay::sheet()),
                ("CalendarZoomCell", CalendarZoomCell::sheet()),
                ("CalendarWeekdayCell", CalendarWeekdayCell::sheet()),
                ("CalendarWeekRow", CalendarWeekRow::sheet()),
                ("CalendarHeader", CalendarHeader::sheet()),
                ("CalendarTitleButton", CalendarTitleButton::sheet()),
            ] {
                assert!(
                    StyleApplication::new(sheet).preminted_class_list().is_some(),
                    "{name} must premint"
                );
            }
        });
    }
}
