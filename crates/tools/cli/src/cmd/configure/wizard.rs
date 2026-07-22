//! Interactive `configure devcontainer` wizard (dialoguer).
//!
//! Builds a [`ConfigureRequest`] from prompts and applies it. The wizard emits
//! only the *changes* the user makes — services left "as is" produce no
//! request — so the resulting report is clean (no spurious already-configured
//! warnings). On a re-run, the currently-configured set is what the per-service
//! action prompt operates on; new services are offered via a multi-select.

use std::path::Path;

use anyhow::Result;
use configure::devcontainer::{
    self, Action, ConfigureReport, ConfigureRequest, ServiceRequest,
};
use dialoguer::{theme::ColorfulTheme, MultiSelect, Select};

pub fn run(dir: &Path, config: Option<&str>) -> Result<ConfigureReport> {
    let state = devcontainer::read_state(dir, config)?;
    let theme = ColorfulTheme::default();

    if !state.exists {
        println!("No devcontainer found — a base one will be initialized in .devcontainer/.");
    }

    let registry = devcontainer::registry();
    let mut requests: Vec<ServiceRequest> = Vec::new();

    // 1. Existing services: per-service Leave / Reconfigure / Remove.
    for svc in &registry {
        let Some(current) = state.enabled.iter().find(|e| e.id == svc.id()) else {
            continue;
        };
        let variant_suffix = current
            .variant
            .as_deref()
            .map(|v| format!(" ({v})"))
            .unwrap_or_default();
        let choice = Select::with_theme(&theme)
            .with_prompt(format!("{}{} is configured", svc.label(), variant_suffix))
            .items(&["Leave as is", "Reconfigure", "Remove"])
            .default(0)
            .interact()?;
        match choice {
            0 => {}
            1 => requests.push(ServiceRequest {
                id: svc.id().to_string(),
                variant: pick_variant(&theme, &**svc, current.variant.as_deref())?,
                action: Action::Reconfigure,
            }),
            _ => requests.push(ServiceRequest {
                id: svc.id().to_string(),
                variant: None,
                action: Action::Remove,
            }),
        }
    }

    // 2. Offer to add services that aren't configured yet.
    let addable: Vec<&Box<dyn devcontainer::DevService>> = registry
        .iter()
        .filter(|s| !state.enabled.iter().any(|e| e.id == s.id()))
        .collect();
    if !addable.is_empty() {
        let labels: Vec<String> = addable
            .iter()
            .map(|s| format!("{} — {}", s.label(), s.description()))
            .collect();
        let selected = MultiSelect::with_theme(&theme)
            .with_prompt("Add services (space to toggle, enter to confirm)")
            .items(&labels)
            .interact()?;
        for idx in selected {
            let svc = addable[idx];
            let variant = pick_variant(&theme, &**svc, None)?;
            // A variant-bearing add uses Reconfigure so the chosen variant is
            // set deterministically; variantless just enables.
            let action = if svc.variants().is_empty() {
                Action::Enable
            } else {
                Action::Reconfigure
            };
            requests.push(ServiceRequest {
                id: svc.id().to_string(),
                variant,
                action,
            });
        }
    }

    devcontainer::apply(
        dir,
        &ConfigureRequest {
            services: requests,
            config: config.map(String::from),
        },
    )
}

/// Prompt for a variant when the service has them, defaulting to `current` (or
/// the service default). Returns `None` for variantless services.
fn pick_variant(
    theme: &ColorfulTheme,
    svc: &dyn devcontainer::DevService,
    current: Option<&str>,
) -> Result<Option<String>> {
    let variants = svc.variants();
    if variants.is_empty() {
        return Ok(None);
    }
    let labels: Vec<&str> = variants.iter().map(|v| v.label).collect();
    let default_idx = current
        .and_then(|c| variants.iter().position(|v| v.id == c))
        .unwrap_or(0);
    let idx = Select::with_theme(theme)
        .with_prompt(format!("{} variant", svc.label()))
        .items(&labels)
        .default(default_idx)
        .interact()?;
    Ok(Some(variants[idx].id.to_string()))
}
