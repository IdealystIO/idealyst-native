//! `idealyst configure vscode` — arg parsing + dispatch to the wizard
//! (interactive) or aspect-flag request builder (non-interactive).
//!
//! ## Non-interactive semantics
//!
//! Aspects are on-by-default (setting up VS Code is the point):
//!
//! - no aspect flags → enable ALL aspects
//! - `--remove`      → remove ALL idealyst-managed VS Code config
//! - `--lint` / `--lint=remove`, `--extensions[=remove]` → act on just that
//!   aspect (unnamed aspects are left untouched)
//! - `--aspect <id>[=remove]` → generic control for any registered aspect

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result};
use configure::vscode::{self, Action, AspectRequest, ConfigureReport, ConfigureRequest};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project directory. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// Skip the interactive wizard; apply the aspect flags directly. Required
    /// when stdin isn't a TTY (CI); implied then too.
    #[arg(long)]
    pub non_interactive: bool,

    /// Remove all idealyst-managed VS Code configuration.
    #[arg(long)]
    pub remove: bool,

    /// Inline lint via rust-analyzer: bare to enable, `=remove` to drop.
    #[arg(long, value_name = "remove")]
    pub lint: Option<Option<String>>,

    /// Extension recommendations: bare to enable, `=remove` to drop.
    #[arg(long, value_name = "remove")]
    pub extensions: Option<Option<String>>,

    /// Generic per-aspect instruction: `--aspect <id>[=remove]`. Repeatable.
    #[arg(long = "aspect", value_name = "id[=remove]")]
    pub aspect: Vec<String>,
}

pub fn run(args: Args) -> Result<()> {
    let dir = args.dir.clone();

    let has_flags = args.remove
        || args.lint.is_some()
        || args.extensions.is_some()
        || !args.aspect.is_empty();
    let interactive = !args.non_interactive && std::io::stdin().is_terminal();

    if interactive {
        if has_flags {
            eprintln!(
                "[configure vscode] aspect flags are ignored in interactive mode; \
                 pass --non-interactive to apply them directly."
            );
        }
        let report = super::wizard_vscode::run(&dir)?;
        print_report(&report);
        return Ok(());
    }

    let req = build_request(&args)?;
    let report = vscode::apply(&dir, &req)
        .with_context(|| format!("configure vscode in {}", dir.display()))?;
    print_report(&report);
    Ok(())
}

/// Translate flags into a request. With no aspect flags (and no `--remove`),
/// enable every aspect; otherwise act only on the named aspects.
fn build_request(args: &Args) -> Result<ConfigureRequest> {
    if args.remove {
        // Remove everything idealyst manages.
        return Ok(ConfigureRequest {
            aspects: vscode::registry()
                .iter()
                .map(|a| AspectRequest { id: a.id().to_string(), action: Action::Remove })
                .collect(),
        });
    }

    let mut aspects = Vec::new();
    if let Some(v) = &args.lint {
        aspects.push(parse_flag("lint", v.as_deref())?);
    }
    if let Some(v) = &args.extensions {
        aspects.push(parse_flag("extensions", v.as_deref())?);
    }
    for spec in &args.aspect {
        let (id, value) = match spec.split_once('=') {
            Some((id, val)) => (id, Some(val)),
            None => (spec.as_str(), None),
        };
        aspects.push(parse_flag(id, value)?);
    }

    if aspects.is_empty() {
        return Ok(vscode::enable_all_request());
    }
    Ok(ConfigureRequest { aspects })
}

/// `None` (bare) = enable; `remove` = remove; anything else is an error.
fn parse_flag(id: &str, value: Option<&str>) -> Result<AspectRequest> {
    let action = match value {
        None => Action::Enable,
        Some("remove") => Action::Remove,
        Some(other) => anyhow::bail!(
            "unknown value {other:?} for aspect {id:?}; expected bare (enable) or `remove`"
        ),
    };
    Ok(AspectRequest { id: id.to_string(), action })
}

fn print_report(report: &ConfigureReport) {
    for w in &report.warnings {
        eprintln!("[configure vscode] warning: {w}");
    }
    let mut changed = false;
    if !report.added.is_empty() {
        println!("  enabled:   {}", report.added.join(", "));
        changed = true;
    }
    if !report.removed.is_empty() {
        println!("  removed:   {}", report.removed.join(", "));
        changed = true;
    }
    if !report.unchanged.is_empty() {
        println!("  unchanged: {}", report.unchanged.join(", "));
    }
    if !changed && report.warnings.is_empty() {
        println!("[configure vscode] no changes.");
    }
    if !report.wrote.is_empty() {
        println!("  wrote:");
        for p in &report.wrote {
            println!("    {}", p.display());
        }
    }
}
