//! `TimeInput` — typed time-of-day entry (`HH:mm` by default) on a
//! [`Field`].
//!
//! ```ignore
//! let starts: Signal<Option<CivilTime>> = signal(None);
//! ui! {
//!     TimeInput(
//!         value = starts,
//!         on_change = move |t: Option<CivilTime>| starts.set(t),
//!         label = "Starts at",
//!         format = "h:mm A",   // 12-hour clock
//!     )
//! }
//! ```
//!
//! Typing follows the typed-field contract (see
//! [`typed_field`](super::typed_field)): a valid parse commits
//! immediately, emptying commits `None`, unparseable text shows the
//! error without disturbing the last committed value, and blur
//! canonicalizes (`9:5` → `09:05`). The token `format` drives both
//! parsing and rendering — `"HH:mm"`, `"HH:mm:ss"`, `"h:mm A"`, ….
//! The smart-typing mask ([`date_mask`](crate::date_mask)) rides the
//! same format: `730` types out as `07:30` (hour 7 can't extend, so
//! the colon inserts itself), Tab completes an ambiguous partial hour
//! in place, and on a 12-hour format a single `a`/`p` completes the
//! meridiem (`730p` → `7:30 PM`).

use std::rc::Rc;

use runtime_core::{
    component, ui, Element, FillRule, IconData, IdealystSchema, Reactive, Signal,
};

use crate::components::field::{Adornment, Field, FieldSize};
use crate::components::typed_field::{typed_field_wiring, TypedFieldSpec};
use crate::date_mask::Mask;
use crate::date::{format_time, parse_time, CivilTime};

/// Leading glyph — a clock. Inline `IconData` like the other built-in
/// affordances (no icon-pack dependency).
pub(crate) const CLOCK_GLYPH: IconData = IconData {
    view_box: (24, 24),
    paths: &["M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20z", "M12 6v6l4 2"],
    fill_rule: FillRule::NonZero,
    filled: false,
};

// Routing: `value` (Signal source) and `on_change` (handler) are
// auto-skipped; the text props route `Reactive` as in `FieldProps`.
// `format` is read ONCE at build (it shapes the parse/render closures
// and the default placeholder — a live format would need the wiring
// rebuilt; snapshot, like Select's `icon`).
#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct TimeInputProps {
    /// Controlled value. The host owns the signal and writes it from
    /// `on_change`.
    pub value: Signal<Option<CivilTime>>,
    /// Fires with `Some(time)` on each valid parse, `None` when the
    /// field is emptied.
    pub on_change: Rc<dyn Fn(Option<CivilTime>)>,
    /// Token format for parsing AND display. Default `HH:mm`.
    pub format: String,
    /// Optional field label (see [`Field`]).
    pub label: Option<String>,
    /// Helper text below the input.
    pub help: Option<String>,
    /// Host-side error text; a parse error takes precedence while
    /// present.
    pub error: Option<String>,
    /// Placeholder; defaults to the format string itself (`HH:mm`).
    pub placeholder: Option<String>,
    /// Error message shown while the text doesn't parse.
    pub invalid_message: String,
    /// Input density. Default Md.
    pub size: FieldSize,
    /// Show the leading clock glyph. Default `true`.
    pub icon: bool,
    /// Pin an exact input width in pixels (see [`Field`]).
    pub width: Option<f32>,
}

impl Default for TimeInputProps {
    fn default() -> Self {
        Self {
            value: runtime_core::signal(None),
            on_change: Rc::new(|_| {}),
            format: Reactive::Static("HH:mm".to_string()),
            label: Reactive::Static(None),
            help: Reactive::Static(None),
            error: Reactive::Static(None),
            placeholder: Reactive::Static(None),
            invalid_message: Reactive::Static("Invalid time".to_string()),
            size: Reactive::Static(FieldSize::default()),
            icon: Reactive::Static(true),
            width: Reactive::Static(None),
        }
    }
}

/// Typed time-of-day input. See the module docs.
#[component]
pub fn TimeInput(props: TimeInputProps) -> Element {
    let format = props.format.get();

    let wiring = typed_field_wiring(TypedFieldSpec {
        value: props.value,
        on_commit: props.on_change.clone(),
        parse: Rc::new({
            let f = format.clone();
            move |s: &str| parse_time(s, &f)
        }),
        render: Rc::new({
            let f = format.clone();
            move |t: CivilTime| format_time(t, &f)
        }),
        invalid_message: props.invalid_message.get(),
        host_error: props.error.clone(),
        mask: Some(Rc::new(Mask::new(&format))),
    });

    // Placeholder defaults to the format itself — `HH:mm` reads as a
    // perfectly good input hint.
    let placeholder = match props.placeholder.clone() {
        Reactive::Static(None) => Reactive::Static(Some(format)),
        other => other,
    };
    let leading =
        if props.icon.get() { Adornment::Icon(CLOCK_GLYPH) } else { Adornment::None };

    ui! {
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
            leading = leading,
            width = props.width.clone(),
        )
    }
}
