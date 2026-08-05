//! Mount smoke tests for the date component family: every component
//! mounts through the real `realize` path against `host-mock` without
//! panicking, and tears down cleanly.
//!
//! This is the evidence bar the repo sets for new components (see
//! `collapsible_direct_mount.rs`): a component body runs OUTSIDE any
//! effect here, so scope/cleanup misuse aborts; the day grid, the
//! zoomed month/year grids, the `switch` rebuild seam, and the
//! external-write follow effects all execute for real. The popup
//! *panels* (which construct on open) are covered at the style layer by
//! the premint tests in `date_picker.rs` — a mock host has no real
//! anchored-overlay geometry to open against.

use std::rc::Rc;

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::components::calendar::{Calendar, RangeCalendar};
use idea_ui::components::date_input::{DateInput, DateTimeInput};
use idea_ui::components::date_picker::{DatePicker, DateRangePicker, DateTimePicker};
use idea_ui::components::time_input::TimeInput;
use idea_ui::date::{CivilDate, CivilDateTime, CivilTime};
use runtime_core::{signal, ui, Element, Signal};

fn mount(build: impl FnOnce() -> Element) {
    let harness = host_mock::Harness::new();
    let tree = harness.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let realized = harness.mount(tree);
    harness.flush();
    drop(realized);
}

#[test]
fn calendar_mounts_with_and_without_selection() {
    mount(|| {
        let picked: Signal<Option<CivilDate>> = signal(None);
        ui! {
            Calendar(
                value = picked,
                on_change = Rc::new(move |d: CivilDate| picked.set(Some(d)))
                    as Rc<dyn Fn(CivilDate)>,
            )
        }
    });
    mount(|| {
        let picked: Signal<Option<CivilDate>> = signal(CivilDate::new(2026, 8, 3));
        ui! {
            Calendar(
                value = picked,
                on_change = Rc::new(move |d: CivilDate| picked.set(Some(d)))
                    as Rc<dyn Fn(CivilDate)>,
                min = CivilDate::new(2026, 1, 1),
                max = CivilDate::new(2026, 12, 31),
                framed = false,
            )
        }
    });
}

#[test]
fn calendar_follows_an_external_value_write_to_its_month() {
    let harness = host_mock::Harness::new();
    let (tree, picked) = harness.world.enter(|| {
        install_idea_theme(light_theme());
        let picked: Signal<Option<CivilDate>> = signal(CivilDate::new(2026, 8, 3));
        let tree = ui! {
            Calendar(
                value = picked,
                on_change = Rc::new(move |d: CivilDate| picked.set(Some(d)))
                    as Rc<dyn Fn(CivilDate)>,
            )
        };
        (tree, picked)
    });
    let realized = harness.mount(tree);
    harness.flush();
    // A typed-commit-style external write months away: the follow effect
    // must snap the visible month and the `switch` must rebuild the grid
    // without panicking.
    harness.world.enter(|| picked.set(CivilDate::new(2027, 2, 14)));
    harness.flush();
    drop(realized);
}

#[test]
fn range_calendar_mounts_with_a_committed_range() {
    mount(|| {
        let range: Signal<Option<(CivilDate, CivilDate)>> = signal(Some((
            CivilDate::new(2026, 8, 3).unwrap(),
            CivilDate::new(2026, 8, 14).unwrap(),
        )));
        ui! {
            RangeCalendar(
                value = range,
                on_change = Rc::new(move |a: CivilDate, b: CivilDate| range.set(Some((a, b))))
                    as Rc<dyn Fn(CivilDate, CivilDate)>,
            )
        }
    });
}

#[test]
fn time_input_mounts() {
    mount(|| {
        let t: Signal<Option<CivilTime>> = signal(CivilTime::new(9, 30, 0));
        ui! {
            TimeInput(
                value = t,
                on_change = Rc::new(move |v: Option<CivilTime>| t.set(v))
                    as Rc<dyn Fn(Option<CivilTime>)>,
                label = "Starts at".to_string(),
            )
        }
    });
}

#[test]
fn pickers_mount_closed() {
    mount(|| {
        let d: Signal<Option<CivilDate>> = signal(None);
        ui! {
            DatePicker(
                value = d,
                on_change = Rc::new(move |v: Option<CivilDate>| d.set(v))
                    as Rc<dyn Fn(Option<CivilDate>)>,
                clearable = true,
            )
        }
    });
    mount(|| {
        let dt: Signal<Option<CivilDateTime>> = signal(Some(CivilDateTime::new(
            CivilDate::new(2026, 8, 3).unwrap(),
            CivilTime::new(14, 0, 0).unwrap(),
        )));
        ui! {
            DateTimePicker(
                value = dt,
                on_change = Rc::new(move |v: Option<CivilDateTime>| dt.set(v))
                    as Rc<dyn Fn(Option<CivilDateTime>)>,
            )
        }
    });
    mount(|| {
        let r: Signal<Option<(CivilDate, CivilDate)>> = signal(None);
        ui! {
            DateRangePicker(
                value = r,
                on_change = Rc::new(move |v: Option<(CivilDate, CivilDate)>| r.set(v))
                    as Rc<dyn Fn(Option<(CivilDate, CivilDate)>)>,
            )
        }
    });
}

#[test]
fn typed_date_inputs_mount() {
    mount(|| {
        let d: Signal<Option<CivilDate>> = signal(CivilDate::new(2026, 8, 3));
        ui! {
            DateInput(
                value = d,
                on_change = Rc::new(move |v: Option<CivilDate>| d.set(v))
                    as Rc<dyn Fn(Option<CivilDate>)>,
                label = "Date of birth".to_string(),
                format = "D/M/YYYY".to_string(),
            )
        }
    });
    mount(|| {
        let dt: Signal<Option<CivilDateTime>> = signal(None);
        ui! {
            DateTimeInput(
                value = dt,
                on_change = Rc::new(move |v: Option<CivilDateTime>| dt.set(v))
                    as Rc<dyn Fn(Option<CivilDateTime>)>,
                label = "Deadline".to_string(),
            )
        }
    });
    // `picker = false` takes the bare-Field early return — a different
    // structural branch (no wrapper view, no popup `when`).
    mount(|| {
        let d: Signal<Option<CivilDate>> = signal(None);
        ui! {
            DateInput(
                value = d,
                on_change = Rc::new(move |v: Option<CivilDate>| d.set(v))
                    as Rc<dyn Fn(Option<CivilDate>)>,
                picker = false,
            )
        }
    });
}
