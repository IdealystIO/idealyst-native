//! Inputs — Field, Switch, Select (one page each).

use std::rc::Rc;

use runtime_core::{signal, ui, Element};
use idea_ui::doc_controls::DocControls;
use idea_ui::{
    typography_kind, Field, FieldProps, Select, SelectOption, Stack, StackGap, Switch, SwitchProps,
    Typography,
};

use crate::shell::{
    self, Callout, CodePanel, ComponentPage, Demo, H2, P, Prop, PropsTable, Section,
};

// =============================================================================
// Field
// =============================================================================

pub fn field() -> Element {
    let value = signal!("".to_string());
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |s| value.set(s));

    let state = FieldProps::init_state();
    let preview = FieldProps::reactive_preview(&state, move |props| {
        let label = props.label;
        let placeholder = props.placeholder;
        let help = props.help;
        let error = props.error;
        let size = props.size;
        let on_change = on_change.clone();
        ui! {
            Field(
                label = label,
                value = value,
                on_change = on_change,
                placeholder = placeholder,
                help = help,
                error = error,
                size = size,
            )
        }
    });
    let controls = FieldProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Field".to_string(),
            lead = "Themed text input with label, helper text, and error tone. The \
                `value` signal lives outside the component — the host owns the input \
                state.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Controlled inputs".to_string()) {
                P(content = "Field is controlled — `value: Signal<String>` is read every render, \
                    `on_change: Rc<dyn Fn(String)>` is invoked when the user types. The host's \
                    signal is the source of truth; the input mirrors it.".to_string())
                CodePanel(src = r##"let email = signal!("".to_string());
let on_email: Rc<dyn Fn(String)> = Rc::new(move |s| email.set(s));

ui! {
    Field(
        label = Some("Email".into()),
        value = email,
        on_change = on_email,
        placeholder = Some("you@example.com".into()),
        help = Some("We'll never share your email.".into()),
    )
}"##.to_string())
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",       ty: "Reactive<Option<String>>", desc: "Optional label above the input." },
                    Prop { name: "value",       ty: "Signal<String>",            desc: "Controlled input value." },
                    Prop { name: "on_change",   ty: "Rc<dyn Fn(String)>",        desc: "Fires on every keystroke." },
                    Prop { name: "placeholder", ty: "Option<String>",            desc: "Hint text shown when value is empty." },
                    Prop { name: "help",        ty: "Reactive<Option<String>>",  desc: "Helper text below the input." },
                    Prop { name: "error",       ty: "Reactive<Option<String>>",  desc: "Error message below the input. Takes precedence over `help` when Some." },
                    Prop { name: "tone",        ty: "Option<ToneRef>",           desc: "Optional border + help-text color overlay. Auto-applied as Danger when `error` is Some." },
                    Prop { name: "size",        ty: "FieldSize",                 desc: "Sm / Md / Lg." },
                ])
            }

            Section(title = "Validation".to_string()) {
                P(content = "Wire an error message reactively from your validator. The Field \
                    flips to the Danger tone automatically when `error` is Some.".to_string())
                CodePanel(src = r##"let email = signal!("".to_string());
let error = rx!(if email.get().contains('@') { None } else { Some("Invalid email".into()) });

Field(
    label = Some("Email".into()),
    value = email,
    on_change = on_email,
    error = error,
)"##.to_string())
            }
        }
    })
}

// =============================================================================
// Switch
// =============================================================================

pub fn switch() -> Element {
    let value = signal!(false);
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |b| value.set(b));

    let state = SwitchProps::init_state();
    let preview = SwitchProps::reactive_preview(&state, move |props| {
        let label = props.label;
        let on_change = on_change.clone();
        ui! {
            Switch(
                label = label,
                value = value,
                on_change = on_change,
            )
        }
    });
    let controls = SwitchProps::render_controls(&state);

    shell::layout(ui! {
        ComponentPage(
            title = "Switch".to_string(),
            lead = "Toggle with an optional inline label. The host owns the bool.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "label",     ty: "Reactive<Option<String>>", desc: "Optional inline label rendered to the left of the toggle." },
                    Prop { name: "value",     ty: "Signal<bool>",             desc: "Controlled bool state." },
                    Prop { name: "on_change", ty: "Rc<dyn Fn(bool)>",         desc: "Fires when the user flips the toggle." },
                ])
            }

            Section(title = "How it composes".to_string()) {
                P(content = "Switch wraps the framework's native `toggle` primitive — the visual \
                    is platform-native (UISwitch on iOS, Switch on Android, CSS toggle on web). \
                    No tone/variant axes today; when the framework primitive grows a tint hook, \
                    a Tone axis would land here.".to_string())
            }
        }
    })
}

// =============================================================================
// Select
// =============================================================================

pub fn select() -> Element {
    let value = signal!("pear".to_string());
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| value.set(v));

    let preview = ui! {
        Select(
            value = value,
            on_change = on_change,
            options = vec![
                SelectOption::new("apple",  "Apple"),
                SelectOption::new("pear",   "Pear"),
                SelectOption::new("banana", "Banana"),
                SelectOption::new("cherry", "Cherry"),
            ],
            placeholder = Some("Choose a fruit".to_string()),
        )
    };

    let current = runtime_core::switch(
        move || value.get(),
        |v: &String| {
            let label = format!("Current value: {}", v);
            ui! { Typography(content = label, muted = true) }
        },
    );

    let controls = ui! {
        Stack(gap = StackGap::Sm) {
            Typography(content = "Notes".to_string(), kind = typography_kind::H3)
            Typography(
                content = "Click the trigger to open the menu, pick an option, then click \
                    outside or press Escape to dismiss.".to_string(),
                muted = true,
            )
            current
        }
    };

    shell::layout(ui! {
        ComponentPage(
            title = "Select".to_string(),
            lead = "Controlled dropdown. Options are { id, label } pairs; the bound \
                Signal<String> holds the active id.".to_string(),
        ) {
            H2(content = "Live demo".to_string())
            Demo(preview = preview, controls = controls)

            Section(title = "Why string-keyed".to_string()) {
                P(content = "Select's value type is `Signal<String>` so it round-trips through \
                    URL params, persisted state, and analytics events without a generic. The \
                    label-to-id mapping lives in the options vec.".to_string())
            }

            Section(title = "Props".to_string()) {
                PropsTable(rows = vec![
                    Prop { name: "value",       ty: "Signal<String>",        desc: "The active option's id." },
                    Prop { name: "on_change",   ty: "Rc<dyn Fn(String)>",    desc: "Fires when the user picks an option; receives the new id." },
                    Prop { name: "options",     ty: "Vec<SelectOption>",     desc: "Options to show. SelectOption::new(id, label)." },
                    Prop { name: "size",        ty: "SelectSize",            desc: "Sm / Md / Lg — trigger height." },
                    Prop { name: "placeholder", ty: "Option<String>",        desc: "Text shown on the trigger when no option is selected." },
                ])
            }

            Section(title = "Reactive option labels".to_string()) {
                P(content = "SelectOption::new takes `Into<Reactive<String>>` for the label, so \
                    a translated dropdown can pass a Signal<String> per option and have rows \
                    re-render on language change without rebuilding the menu.".to_string())
            }

            Callout(label = "For multi-select".to_string()) {
                P(content = "Select is single-select. Multi-select is on the roadmap as a \
                    separate component; for now, compose Tag(on_remove = ...) chips alongside \
                    a Field for fast adds.".to_string())
            }
        }
    })
}
