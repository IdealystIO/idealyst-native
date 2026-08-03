//! `DateInput` / `DateTimeInput` — typed date entry on a [`Field`],
//! with a calendar-button popup.
//!
//! ```ignore
//! let birthday: Signal<Option<CivilDate>> = signal(None);
//! ui! {
//!     DateInput(
//!         value = birthday,
//!         on_change = move |d: Option<CivilDate>| birthday.set(d),
//!         label = "Date of birth",
//!         format = "D/M/YYYY",
//!         max = CivilDate::today(),
//!     )
//! }
//! ```
//!
//! Typing follows the typed-field contract (see
//! [`typed_field`](super::typed_field)): valid text commits as you
//! type (leniently parsed — `2026-3-7` works), emptying commits
//! `None`, garbage shows the error without disturbing the committed
//! value, and blur canonicalizes. On top of that sits the smart-typing
//! mask ([`date_mask`](crate::date_mask)): the format's delimiters
//! insert themselves as digits arrive (`07031994` → `07/03/1994`), a
//! digit no segment can extend jumps ahead on its own (month `2` →
//! `02/`), and Tab completes an ambiguous partial segment in place
//! (month `1` + Tab → `01/`, caret on the day). Deletions and
//! mid-string edits bypass the mask, so corrections behave like a
//! plain text field. The trailing calendar button opens an anchored
//! [`Calendar`] popup as an alternative to typing; picking a day
//! commits and (for `DateInput`) closes.
//!
//! `DateTimeInput` is the same machinery over [`CivilDateTime`]: its
//! `format` includes time tokens (default `YYYY-MM-DD HH:mm`) and its
//! popup adds a [`TimeInput`] row, staying open across edits like
//! `DateTimePicker`.

use std::rc::Rc;

use runtime_core::primitives::portal::AnchorTarget;
use runtime_core::{
    component, effect, signal, ui, view, when, Element, IdealystSchema, IntoElement, Reactive,
    Ref, Signal, ViewHandle,
};

use crate::components::calendar::Calendar;
use crate::components::date_picker::{anchored_panel, CALENDAR_GLYPH};
use crate::components::field::{Adornment, Field, FieldSize};
use crate::components::time_input::TimeInput;
use crate::components::typed_field::{typed_field_wiring, TypedFieldSpec};
use crate::date_mask::Mask;
use crate::date::{
    format_date, format_datetime, parse_date, parse_datetime, CivilDate, CivilDateTime,
    CivilTime, DateLabels, Weekday,
};

// Routing as in `TimeInputProps`; `format` is snapshotted at build (it
// shapes the parse/render closures and the default placeholder).
#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct DateInputProps {
    /// Controlled value. The host owns the signal and writes it from
    /// `on_change`.
    pub value: Signal<Option<CivilDate>>,
    /// Fires with `Some(date)` on each valid commit, `None` when the
    /// field is emptied.
    pub on_change: Rc<dyn Fn(Option<CivilDate>)>,
    /// Token format for parsing AND display. Default `YYYY-MM-DD`.
    pub format: String,
    /// Optional field label (see [`Field`]).
    pub label: Option<String>,
    /// Helper text below the input.
    pub help: Option<String>,
    /// Host-side error text; a parse error takes precedence while
    /// present.
    pub error: Option<String>,
    /// Placeholder; defaults to the format string itself.
    pub placeholder: Option<String>,
    /// Error message shown while the text doesn't parse.
    pub invalid_message: String,
    /// Input density. Default Md.
    pub size: FieldSize,
    /// Show the trailing calendar button + popup. Default `true`.
    pub picker: bool,
    /// Earliest pickable popup day (inclusive). NOTE: bounds gate the
    /// POPUP only — typed text is validated for shape, not range; wire
    /// `error` for range validation feedback.
    pub min: Option<CivilDate>,
    /// Latest pickable popup day (inclusive).
    pub max: Option<CivilDate>,
    /// Per-day popup veto.
    pub is_date_disabled: Option<Rc<dyn Fn(CivilDate) -> bool>>,
    /// First popup calendar column. Default Monday (ISO).
    pub first_weekday: Weekday,
    /// Month/weekday display names. Default English.
    #[prop(static)]
    pub labels: Option<Rc<DateLabels>>,
    /// Pin an exact input width in pixels (see [`Field`]).
    pub width: Option<f32>,
}

impl Default for DateInputProps {
    fn default() -> Self {
        Self {
            value: runtime_core::signal(None),
            on_change: Rc::new(|_| {}),
            format: Reactive::Static("YYYY-MM-DD".to_string()),
            label: Reactive::Static(None),
            help: Reactive::Static(None),
            error: Reactive::Static(None),
            placeholder: Reactive::Static(None),
            invalid_message: Reactive::Static("Invalid date".to_string()),
            size: Reactive::Static(FieldSize::default()),
            picker: Reactive::Static(true),
            min: Reactive::Static(None),
            max: Reactive::Static(None),
            is_date_disabled: None,
            first_weekday: Reactive::Static(Weekday::Monday),
            labels: None,
            width: Reactive::Static(None),
        }
    }
}

/// Typed date input with a calendar popup. See the module docs.
#[component]
pub fn DateInput(props: DateInputProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let format = props.format.get();

    let wiring = typed_field_wiring(TypedFieldSpec {
        value,
        on_commit: on_change.clone(),
        parse: Rc::new({
            let f = format.clone();
            move |s: &str| parse_date(s, &f)
        }),
        render: Rc::new({
            let f = format.clone();
            move |d: CivilDate| format_date(d, &f)
        }),
        invalid_message: props.invalid_message.get(),
        host_error: props.error.clone(),
        mask: Some(Rc::new(Mask::new(&format))),
    });

    let placeholder = match props.placeholder.clone() {
        Reactive::Static(None) => Reactive::Static(Some(format)),
        other => other,
    };

    let open: Signal<bool> = signal(false);
    let trailing = if props.picker.get() {
        Adornment::button(CALENDAR_GLYPH, move || open.set(!open.peek()))
    } else {
        Adornment::None
    };

    let field = ui! {
        Field(
            value = wiring.text,
            on_change = wiring.on_change,
            error = wiring.error,
            on_focus_change = Some(wiring.on_focus_change),
            on_key_down = wiring.on_key_down,
            label = props.label.clone(),
            help = props.help.clone(),
            placeholder = placeholder,
            size = props.size.clone(),
            trailing = trailing,
            width = props.width.clone(),
        )
    };

    if !props.picker.get() {
        return field;
    }

    // The popup anchors to a wrapper view around the whole Field group
    // (a Field exposes no handle of its own to anchor to) — so it opens
    // below label + input + help. Bounded correctness over pixel
    // perfection; the trigger-anchored pickers exist for the tight case.
    let anchor_ref: Ref<ViewHandle> = Ref::new();

    let min = props.min.clone();
    let max = props.max.clone();
    let is_date_disabled = props.is_date_disabled.clone();
    let first_weekday = props.first_weekday.clone();
    let labels = props.labels.clone();

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
                        // The host writes `value`; the wiring's follow
                        // effect re-renders the text canonically.
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
            anchored_panel(AnchorTarget::from(anchor_ref), vec![calendar], close)
        },
        runtime_core::empty_absolute_view,
    );

    view(vec![field, popup]).bind(anchor_ref).into_element()
}

// ---------------------------------------------------------------------------
// DateTimeInput
// ---------------------------------------------------------------------------

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct DateTimeInputProps {
    /// Controlled value (date + time of day).
    pub value: Signal<Option<CivilDateTime>>,
    /// Fires with `Some(value)` on each valid commit, `None` when the
    /// field is emptied.
    pub on_change: Rc<dyn Fn(Option<CivilDateTime>)>,
    /// Token format (date + time tokens) for parsing AND display.
    /// Default `YYYY-MM-DD HH:mm`.
    pub format: String,
    /// Token format for the POPUP's time row. Default `HH:mm`.
    pub time_format: String,
    /// Time used when the popup picks a day before any time is set.
    pub default_time: CivilTime,
    /// Optional field label.
    pub label: Option<String>,
    /// Helper text below the input.
    pub help: Option<String>,
    /// Host-side error text.
    pub error: Option<String>,
    /// Placeholder; defaults to the format string itself.
    pub placeholder: Option<String>,
    /// Error message shown while the text doesn't parse.
    pub invalid_message: String,
    /// Input density. Default Md.
    pub size: FieldSize,
    /// Show the trailing calendar button + popup. Default `true`.
    pub picker: bool,
    /// Earliest pickable popup day (inclusive) — popup only, like
    /// [`DateInputProps::min`].
    pub min: Option<CivilDate>,
    /// Latest pickable popup day (inclusive).
    pub max: Option<CivilDate>,
    /// Per-day popup veto.
    pub is_date_disabled: Option<Rc<dyn Fn(CivilDate) -> bool>>,
    /// First popup calendar column. Default Monday (ISO).
    pub first_weekday: Weekday,
    /// Month/weekday display names. Default English.
    #[prop(static)]
    pub labels: Option<Rc<DateLabels>>,
    /// Pin an exact input width in pixels.
    pub width: Option<f32>,
}

impl Default for DateTimeInputProps {
    fn default() -> Self {
        Self {
            value: runtime_core::signal(None),
            on_change: Rc::new(|_| {}),
            format: Reactive::Static("YYYY-MM-DD HH:mm".to_string()),
            time_format: Reactive::Static("HH:mm".to_string()),
            default_time: Reactive::Static(CivilTime::MIDNIGHT),
            label: Reactive::Static(None),
            help: Reactive::Static(None),
            error: Reactive::Static(None),
            placeholder: Reactive::Static(None),
            invalid_message: Reactive::Static("Invalid date/time".to_string()),
            size: Reactive::Static(FieldSize::default()),
            picker: Reactive::Static(true),
            min: Reactive::Static(None),
            max: Reactive::Static(None),
            is_date_disabled: None,
            first_weekday: Reactive::Static(Weekday::Monday),
            labels: None,
            width: Reactive::Static(None),
        }
    }
}

/// Typed date+time input with a calendar+time popup. See the module
/// docs.
#[component]
pub fn DateTimeInput(props: DateTimeInputProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let format = props.format.get();

    let wiring = typed_field_wiring(TypedFieldSpec {
        value,
        on_commit: on_change.clone(),
        parse: Rc::new({
            let f = format.clone();
            move |s: &str| parse_datetime(s, &f)
        }),
        render: Rc::new({
            let f = format.clone();
            move |dt: CivilDateTime| format_datetime(dt, &f)
        }),
        invalid_message: props.invalid_message.get(),
        host_error: props.error.clone(),
        mask: Some(Rc::new(Mask::new(&format))),
    });

    let placeholder = match props.placeholder.clone() {
        Reactive::Static(None) => Reactive::Static(Some(format)),
        other => other,
    };

    let open: Signal<bool> = signal(false);
    let trailing = if props.picker.get() {
        Adornment::button(CALENDAR_GLYPH, move || open.set(!open.peek()))
    } else {
        Adornment::None
    };

    let field = ui! {
        Field(
            value = wiring.text,
            on_change = wiring.on_change,
            error = wiring.error,
            on_focus_change = Some(wiring.on_focus_change),
            on_key_down = wiring.on_key_down,
            label = props.label.clone(),
            help = props.help.clone(),
            placeholder = placeholder,
            size = props.size.clone(),
            trailing = trailing,
            width = props.width.clone(),
        )
    };

    if !props.picker.get() {
        return field;
    }

    let anchor_ref: Ref<ViewHandle> = Ref::new();

    // Popup-side halves — same shape as `DateTimePicker`.
    let date_part: Signal<Option<CivilDate>> = signal(value.peek().map(|dt| dt.date));
    let time_part: Signal<Option<CivilTime>> = signal(Some(
        value.peek().map(|dt| dt.time).unwrap_or(props.default_time.get()),
    ));
    effect!({
        if let Some(dt) = value.get() {
            date_part.set(Some(dt.date));
            time_part.set(Some(dt.time));
        } else {
            date_part.set(None);
        }
    });

    let min = props.min.clone();
    let max = props.max.clone();
    let is_date_disabled = props.is_date_disabled.clone();
    let first_weekday = props.first_weekday.clone();
    let labels = props.labels.clone();
    let time_format = props.time_format.clone();
    let default_time = props.default_time.clone();

    let popup = when(
        move || open.get(),
        move || {
            let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));

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

            anchored_panel(AnchorTarget::from(anchor_ref), vec![calendar, time_row], close)
        },
        runtime_core::empty_absolute_view,
    );

    view(vec![field, popup]).bind(anchor_ref).into_element()
}

runtime_core::recipe!(
    DateInput,
    /// Birthdate entry: lenient typed input (`7/3/1994` parses against
    /// `D/M/YYYY` and canonicalizes on blur) with the popup calendar capped
    /// at today. The month/year zoom in the popup header makes reaching a
    /// birth year fast.
    pub fn date_input_birthdate() -> ::runtime_core::Element {
        use crate::components::date_input::DateInput;
        use crate::date::CivilDate;
        use ::runtime_core::{signal, ui, Signal};
        use ::std::rc::Rc;

        let birthday: Signal<Option<CivilDate>> = signal(None);
        let on_change: Rc<dyn Fn(Option<CivilDate>)> = Rc::new(move |d| birthday.set(d));
        ui! {
            DateInput(
                value = birthday,
                on_change = on_change,
                label = "Date of birth".to_string(),
                format = "D/M/YYYY".to_string(),
                max = CivilDate::today(),
            )
        }
    }
);
