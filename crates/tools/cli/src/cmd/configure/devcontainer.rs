//! `idealyst configure devcontainer` — arg parsing + dispatch to the wizard
//! (interactive) or flag-driven request builder (non-interactive).
//!
//! ## Non-interactive flag semantics
//!
//! One optional-value flag per service, plus a generic `--service id[=…]`
//! escape hatch for services without a dedicated flag:
//!
//! - `--minio`              → enable (no-op + warning if already configured)
//! - `--minio=remove`       → remove
//! - `--minio=reconfigure`  → reset to default variant (blank config)
//! - `--database=postgres`  → ensure the `postgres` variant (adds or switches)
//!
//! `remove` / `reconfigure` are reserved verbs; any other value is treated as
//! a variant id (validated against the service). Adding a service to the
//! registry only requires a new field here if you want a dedicated `--<id>`
//! flag — `--service <id>` works for it immediately.

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result};
use configure::devcontainer::{self, Action, ConfigureReport, ConfigureRequest, ServiceRequest};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project directory. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,

    /// Skip the interactive wizard; apply the service flags directly. Required
    /// when stdin isn't a TTY (CI); implied then too.
    #[arg(long)]
    pub non_interactive: bool,

    /// Database sidecar: bare to enable (default `postgres`), `=mysql` to pick
    /// a variant, `=remove` / `=reconfigure` to drop / reset.
    #[arg(long, value_name = "postgres|mysql|remove|reconfigure")]
    pub database: Option<Option<String>>,

    /// Redis sidecar: bare to enable, `=remove` / `=reconfigure`.
    #[arg(long, value_name = "remove|reconfigure")]
    pub redis: Option<Option<String>>,

    /// MinIO (S3) sidecar: bare to enable, `=remove` / `=reconfigure`.
    #[arg(long, value_name = "remove|reconfigure")]
    pub minio: Option<Option<String>>,

    /// Generic per-service instruction for any registered service:
    /// `--service <id>[=variant|remove|reconfigure]`. Repeatable.
    #[arg(long = "service", value_name = "id[=variant|remove|reconfigure]")]
    pub service: Vec<String>,
}

pub fn run(args: Args) -> Result<()> {
    let dir = args.dir.clone();

    let has_flags = args.database.is_some()
        || args.redis.is_some()
        || args.minio.is_some()
        || !args.service.is_empty();
    let interactive = !args.non_interactive && std::io::stdin().is_terminal();

    if interactive {
        // The wizard fully determines the end state; flags are ignored in
        // interactive mode (warn if the user passed some).
        if has_flags {
            eprintln!(
                "[configure devcontainer] service flags are ignored in interactive mode; \
                 pass --non-interactive to apply them directly."
            );
        }
        let report = super::wizard::run(&dir)?;
        print_report(&report);
        return Ok(());
    }

    // Non-interactive: build the request from flags. If the user asked for the
    // wizard but there's no TTY, tell them how to proceed.
    if !args.non_interactive && !std::io::stdin().is_terminal() {
        eprintln!(
            "[configure devcontainer] no interactive terminal detected — running \
             non-interactively. Pass service flags (e.g. --database --redis) or \
             --non-interactive to silence this notice."
        );
    }

    let req = build_request(&args)?;
    let report = devcontainer::apply(&dir, &req)
        .with_context(|| format!("configure devcontainer in {}", dir.display()))?;
    print_report(&report);
    Ok(())
}

/// Translate the flag set into a [`ConfigureRequest`].
fn build_request(args: &Args) -> Result<ConfigureRequest> {
    let mut services = Vec::new();

    if let Some(v) = &args.database {
        services.push(parse_flag("database", v.as_deref())?);
    }
    if let Some(v) = &args.redis {
        services.push(parse_flag("redis", v.as_deref())?);
    }
    if let Some(v) = &args.minio {
        services.push(parse_flag("minio", v.as_deref())?);
    }
    for spec in &args.service {
        let (id, value) = match spec.split_once('=') {
            Some((id, val)) => (id, Some(val)),
            None => (spec.as_str(), None),
        };
        services.push(parse_flag(id, value)?);
    }

    Ok(ConfigureRequest { services })
}

/// Map one flag value to a [`ServiceRequest`]. `None` (bare flag) = enable;
/// `remove`/`reconfigure` = those actions; anything else = a variant (which
/// reconfigures to exactly that variant, adding the service if absent).
fn parse_flag(id: &str, value: Option<&str>) -> Result<ServiceRequest> {
    let (action, variant) = match value {
        None => (Action::Enable, None),
        Some("remove") => (Action::Remove, None),
        Some("reconfigure") => (Action::Reconfigure, None),
        Some(variant) => (Action::Reconfigure, Some(variant.to_string())),
    };
    Ok(ServiceRequest { id: id.to_string(), variant, action })
}

fn print_report(report: &ConfigureReport) {
    for w in &report.warnings {
        eprintln!("[configure devcontainer] warning: {w}");
    }
    let mut changed = false;
    if !report.added.is_empty() {
        println!("  added:        {}", report.added.join(", "));
        changed = true;
    }
    if !report.reconfigured.is_empty() {
        println!("  reconfigured: {}", report.reconfigured.join(", "));
        changed = true;
    }
    if !report.removed.is_empty() {
        println!("  removed:      {}", report.removed.join(", "));
        changed = true;
    }
    if !report.unchanged.is_empty() {
        println!("  unchanged:    {}", report.unchanged.join(", "));
    }
    if !changed && report.warnings.is_empty() {
        println!("[configure devcontainer] no changes.");
    }
    if !report.wrote.is_empty() {
        println!("  wrote:");
        for p in &report.wrote {
            println!("    {}", p.display());
        }
    }
}
