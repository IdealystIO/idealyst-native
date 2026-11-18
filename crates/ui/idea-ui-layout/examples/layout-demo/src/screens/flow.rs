//! A `Flow`: one step at a time, its actions pinned at the foot.

use std::rc::Rc;

use idea_ui::Field;
use idea_ui_layout::{Action, ActionBar, FieldGrid, FieldGridRow, Flow, FlowStep, PageHeader};
use runtime_core::{component, rx, signal, ui, Element};

use crate::{Demo, Shell};

const LAST_STEP: usize = 2;

#[component]
pub(crate) fn FlowScreen(#[prop(static)] shell: Shell) -> Element {
    let step = signal(0_usize);
    let name = signal("North Ramp".to_string());
    let client = signal(String::new());
    let notes = signal(String::new());
    let next: Rc<dyn Fn()> = Rc::new(move || step.set((step.get() + 1).min(LAST_STEP)));
    let back: Rc<dyn Fn()> = Rc::new(move || step.set(step.get().saturating_sub(1)));
    let jump: Rc<dyn Fn(usize)> = Rc::new(move |i| step.set(i));
    let leave: Rc<dyn Fn()> = Rc::new(move || shell.screen.set(Demo::Content));
    // A done step says what it holds, under its row in the list.
    let steps = move || {
        let at = step.get();
        vec![
            FlowStep::new("Basics").subtitle("What the project is called").status((at > 0).then(|| name.get())),
            FlowStep::new("Shifts").subtitle("When the crews work").status((at > 1).then(|| "Day, Night".to_string())),
            FlowStep::new("Crew").subtitle("Who is on it"),
        ]
    };
    ui! {
        Flow(
            header = ui! {
                PageHeader(
                    title = "New project".to_string(),
                    leading = Some(shell.leading()), status = move || shell.style_picker(),
                    actions = vec![Action::new("Cancel", leave.clone())],
                )
            },
            steps = rx!(steps()),
            current = rx!(step.get()),
            on_step = Some(jump.clone()),
            body = ui! {
                FieldGrid(columns = 2_usize) {
                    Field(label = Some("Name".to_string()), value = name)
                    Field(label = Some("Client".to_string()), value = client)
                    FieldGridRow { Field(label = Some("Notes".to_string()), value = notes) }
                }
            },
            actions = ui! {
                ActionBar(
                    primary = Some(Action::new("Next", next.clone())),
                    secondary = vec![Action::new("Back", back.clone())],
                    note = Some("Each step saves as you go.".to_string()),
                    pinned = true,
                )
            },
        )
    }
}
