//! Inputs — Field, Switch, Select.

use runtime_core::{signal, ui, Element};
use idea_ui::doc_controls::DocControls;
use idea_ui::{Typography, Card, Field, Select, Stack, Switch, FieldProps, SelectOption, StackGap, SwitchProps};

use crate::shell::{demo_card, page_header};

pub fn page() -> Element {
    ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Inputs",
                "Field, Switch, and Select — all controlled by host-owned `Signal<T>` values."
            ) }

            { field_demo() }
            { switch_demo() }
            { select_demo() }
        }
    }
}

fn field_demo() -> Element {
    // Field's controlled signal lives outside the docs-state — it's
    // the host's source of truth for the input value. Captured by
    // the build closure so each rebuild reuses the same signal.
    let value = signal!("".to_string());
    let on_change: std::rc::Rc<dyn Fn(String)> = std::rc::Rc::new(move |s| value.set(s));

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
                size = size
            )
        }
    });
    let controls = FieldProps::render_controls(&state);
    demo_card(
        "Field",
        "Themed text input with label, help, and error tone. The `value` signal lives \
         outside the docs state — that's the host's source of truth.",
        preview,
        controls,
    )
}

fn switch_demo() -> Element {
    let value = signal!(false);
    let on_change: std::rc::Rc<dyn Fn(bool)> = std::rc::Rc::new(move |b| value.set(b));

    let state = SwitchProps::init_state();
    let preview = SwitchProps::reactive_preview(&state, move |props| {
        let label = props.label;
        let on_change = on_change.clone();
        ui! {
            Switch(
                label = label,
                value = value,
                on_change = on_change
            )
        }
    });
    let controls = SwitchProps::render_controls(&state);
    demo_card(
        "Switch",
        "Toggle with an optional inline label. Same controlled shape — host owns the bool.",
        preview,
        controls,
    )
}

fn select_demo() -> Element {
    // Controlled signal — host owns the chosen option's id.
    let value = signal!("pear".to_string());
    let on_change: std::rc::Rc<dyn Fn(String)> = std::rc::Rc::new(move |v| value.set(v));

    // Select's props don't auto-derive — `Vec<SelectOption>` isn't
    // a controllable field type. So we hand-build the preview +
    // an inline controls panel that just describes what to click
    // on. The Select's *current selection* is shown in the
    // sibling Body using the same `value` signal — proving the
    // controlled-binding round-trip.
    let preview = ui! {
        Select(
            value = value,
            on_change = on_change,
            options = vec![
                SelectOption::new("apple", "Apple"),
                SelectOption::new("pear", "Pear"),
                SelectOption::new("banana", "Banana"),
                SelectOption::new("cherry", "Cherry"),
            ],
            placeholder = Some("Choose a fruit".to_string())
        )
    };
    let current = runtime_core::switch(
        move || value.get(),
        |v: &String| {
            let label = format!("Current value: {}", v);
            ui! { Typography(content = label, muted = true) }
        },
    );
    let notes = ui! {
        Typography(content = "Click the trigger to open the menu, then pick an option. \
                          The chosen id is written to a `Signal<String>` the host owns; \
                          the label shown on the trigger comes from looking that id up \
                          in the options list. Click outside or press Escape to \
                          dismiss without picking.".to_string(),
             muted = true)
    };
    let controls = ui! {
        Card {
            Typography(content = "Notes".to_string(), kind = idea_ui::typography_kind::H3)
            notes
            current
        }
    };
    demo_card(
        "Select",
        "Controlled dropdown. String-keyed (like Tabs): options are `{ id, label }` pairs; \
         the bound `Signal<String>` holds the active id.",
        preview,
        controls,
    )
}
