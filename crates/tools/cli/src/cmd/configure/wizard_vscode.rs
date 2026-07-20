//! Interactive `configure vscode` wizard (dialoguer).
//!
//! A single multi-select of aspects, currently-configured ones checked. The
//! resulting selection is the desired end state: newly checked → enable,
//! unchecked-but-was-configured → remove, otherwise no change. Only actual
//! changes become requests, so the report stays clean.

use std::path::Path;

use anyhow::Result;
use configure::vscode::{self, Action, AspectRequest, ConfigureReport, ConfigureRequest};
use dialoguer::{theme::ColorfulTheme, MultiSelect};

pub fn run(dir: &Path) -> Result<ConfigureReport> {
    let state = vscode::read_state(dir)?;
    let theme = ColorfulTheme::default();
    let registry = vscode::registry();

    let labels: Vec<String> = registry
        .iter()
        .map(|a| format!("{} — {}", a.label(), a.description()))
        .collect();
    let checked: Vec<bool> = registry
        .iter()
        .map(|a| state.enabled.iter().any(|e| e == a.id()))
        .collect();
    let items: Vec<(String, bool)> =
        labels.into_iter().zip(checked.iter().copied()).collect();

    let selected = MultiSelect::with_theme(&theme)
        .with_prompt("VS Code configuration (space to toggle, enter to confirm)")
        .items_checked(&items)
        .interact()?;

    let mut aspects = Vec::new();
    for (i, aspect) in registry.iter().enumerate() {
        let want = selected.contains(&i);
        let was = checked[i];
        if want && !was {
            aspects.push(AspectRequest { id: aspect.id().to_string(), action: Action::Enable });
        } else if !want && was {
            aspects.push(AspectRequest { id: aspect.id().to_string(), action: Action::Remove });
        }
    }

    vscode::apply(dir, &ConfigureRequest { aspects })
}
