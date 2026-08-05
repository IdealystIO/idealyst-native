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
//! commits and (for `DateInput`) closes. `clearable` adds an ✕ button
//! left of the calendar button that empties the field and commits
//! `None` — the input-side sibling of the pickers' `Clear` footer
//! action.
//!
//! `DateTimeInput` is the same machinery over [`CivilDateTime`]: its
//! `format` includes time tokens (default `YYYY-MM-DD HH:mm`) and its
//! popup adds a [`TimeInput`] row, staying open across edits like
//! `DateTimePicker`.

use std::rc::Rc;

use runtime_core::primitives::portal::AnchorTarget;
use runtime_core::{
    component, effect, signal, ui, view, when, Element, FillRule, IconData, IdealystSchema,
    IntoElement, Reactive, Ref, Signal, ViewHandle,
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

/// Trailing clear glyph — an ✕. Inline `IconData` like
/// [`CALENDAR_GLYPH`]: no icon-pack dependency for built-in affordances.
const CLEAR_GLYPH: IconData = IconData {
    view_box: (24, 24),
    paths: &["M18 6 6 18", "m6 6 12 12"],
    fill_rule: FillRule::NonZero,
    filled: false,
};

/// The trailing adornment shared by [`DateInput`] / [`DateTimeInput`]:
/// an optional clear ✕ (which empties the field and commits `None`)
/// left of the optional calendar button.
fn trailing_adornments(
    clearable: bool,
    clear: Rc<dyn Fn()>,
    picker: bool,
    open: Signal<bool>,
) -> Adornment {
    let mut items: Vec<Adornment> = Vec::new();
    if clearable {
        items.push(Adornment::button(CLEAR_GLYPH, move || (clear)()));
    }
    if picker {
        items.push(Adornment::button(CALENDAR_GLYPH, move || open.set(!open.peek())));
    }
    // An empty group renders nothing, so the no-adornment Field path
    // (bare input, no shell) is preserved when both are off.
    Adornment::Group(items)
}

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
    /// Offer a trailing ✕ button (left of the calendar button) that
    /// empties the field and commits `None`. Default `false`.
    pub clearable: bool,
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
            clearable: Reactive::Static(false),
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
    let trailing = trailing_adornments(
        props.clearable.get(),
        wiring.clear.clone(),
        props.picker.get(),
        open,
    );

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
    /// Offer a trailing ✕ button (left of the calendar button) that
    /// empties the field and commits `None`. Default `false`.
    pub clearable: bool,
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
            clearable: Reactive::Static(false),
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
    let trailing = trailing_adornments(
        props.clearable.get(),
        wiring.clear.clone(),
        props.picker.get(),
        open,
    );

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::{commit, with_test_world};
    use idea_theme::theme::{install_idea_theme, light_theme};

    /// Drill to a built `DateInput`'s adorned-shell children:
    /// root view [field, popup] → Field group view [shell] → shell row.
    fn shell_children(root: Element) -> Vec<Element> {
        let mut outer = classify(root).children();
        let field_group = classify(outer.remove(0)).children();
        let mut shell = None;
        for c in field_group {
            if let P::View { children, .. } = classify(c) {
                shell = Some(children);
            }
        }
        shell.expect("an adorned DateInput renders a shell row")
    }

    /// The ✕ renders LEFT of the calendar button, and pressing it
    /// commits `None` — the whole point of `clearable`.
    #[test]
    fn clearable_renders_x_before_calendar_and_commits_none() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let value: Signal<Option<CivilDate>> = signal(CivilDate::new(2026, 8, 5));
            let el = DateInput(DateInputProps {
                value,
                on_change: Rc::new(move |d| value.set(d)),
                clearable: Reactive::Static(true),
                ..Default::default()
            });

            let shell = shell_children(el);
            // Row order: input, ✕, calendar.
            let mut it = shell.into_iter();
            assert!(matches!(classify(it.next().unwrap()), P::TextInput { .. }));
            let clear = match classify(it.next().unwrap()) {
                P::Pressable { children, on_click, .. } => {
                    let glyph = classify(children.into_iter().next().unwrap());
                    match glyph {
                        P::Icon { data, .. } => assert_eq!(
                            data.paths,
                            CLEAR_GLYPH.paths,
                            "the ✕ sits left of the calendar button"
                        ),
                        _ => panic!("clear button wraps an icon"),
                    }
                    on_click
                }
                _ => panic!("clearable renders a pressable ✕"),
            };
            match classify(it.next().unwrap()) {
                P::Pressable { children, .. } => match classify(children.into_iter().next().unwrap())
                {
                    P::Icon { data, .. } => assert_eq!(data.paths, CALENDAR_GLYPH.paths),
                    _ => panic!("calendar button wraps an icon"),
                },
                _ => panic!("picker renders the calendar button"),
            }

            (clear)();
            commit();
            assert_eq!(value.peek(), None, "pressing ✕ commits None");
        });
    }

    /// Default stays as before: no ✕, just the calendar button.
    #[test]
    fn non_clearable_renders_calendar_button_only() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let shell = shell_children(DateInput(DateInputProps::default()));
            let kinds: Vec<_> = shell
                .into_iter()
                .map(|c| match classify(c) {
                    P::TextInput { .. } => "input",
                    P::Pressable { .. } => "button",
                    _ => "other",
                })
                .collect();
            assert_eq!(kinds, vec!["input", "button"], "one trailing button: the calendar");
        });
    }
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
