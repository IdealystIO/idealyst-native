//! `idealyst lint` — run the idealyst source linter over a project.
//!
//! Thin orchestration around the `lint` crate: resolve config (explicit
//! `--config`, else discover `idealyst-lint.toml`, else built-in
//! defaults), walk the target path for `.rs` files, run the rules, and
//! print the findings in the chosen format.
//!
//! Two output formats, one engine:
//!   - `human` (default) — caret-underlined terminal report.
//!   - `json` — cargo `--message-format=json` lines for rust-analyzer's
//!     `check.overrideCommand`, so the same rules surface as inline editor
//!     squiggles. See `crates/tools/lint/README.md` for the RA wiring.
//!
//! Exit status mirrors `cargo check`: non-zero when any error-level
//! diagnostic fires (or a file fails to parse), so CI gates on it.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use lint::{Config, LintRun};

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Human-readable terminal report (default).
    Human,
    /// cargo `--message-format=json` lines for rust-analyzer / CI tools.
    Json,
}

#[derive(clap::Args, Debug)]
pub struct Args {
    /// File or directory to lint. A directory is walked recursively
    /// (skipping `target/`, `.git/`, `node_modules/`).
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Output format.
    #[arg(long, value_enum, default_value = "human")]
    pub format: Format,

    /// Explicit config file. Overrides `idealyst-lint.toml` discovery.
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// List the available rules and their default levels, then exit.
    #[arg(long)]
    pub rules: bool,

    /// Treat warnings as errors for the exit status (CI strict mode).
    #[arg(long)]
    pub deny_warnings: bool,
}

pub fn run(args: Args) -> Result<()> {
    if args.rules {
        return list_rules();
    }

    // Resolve config: explicit path wins, else discover from the target.
    let loaded = match &args.config {
        Some(p) => Config::load_file(p).with_context(|| format!("loading config {}", p.display()))?,
        None => Config::discover(&args.path).context("discovering idealyst-lint.toml")?,
    };
    for unknown in &loaded.unknown_rules {
        eprintln!("[lint] warning: config names unknown rule `{unknown}` (ignored)");
    }
    let config = loaded.config;

    let run = lint::lint_path(&args.path, &config);

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match args.format {
        Format::Human => {
            let color = std::io::stdout().is_terminal();
            lint::report::human(&run, &mut out, color)?;
        }
        Format::Json => {
            // Root for the cargo envelope's manifest_path: the config dir
            // if we found one, else the lint target.
            let root = loaded
                .path
                .as_ref()
                .and_then(|p| p.parent())
                .map(PathBuf::from)
                .unwrap_or_else(|| args.path.clone());
            lint::report::cargo_json(&run, &root, &mut out)?;
        }
    }
    out.flush().ok();

    if should_fail(&run, args.deny_warnings) {
        // Match `cargo check`'s failure surface: a non-zero exit with a
        // terse summary. The detailed findings already printed above.
        std::process::exit(1);
    }
    Ok(())
}

fn should_fail(run: &LintRun, deny_warnings: bool) -> bool {
    run.failed() || (deny_warnings && run.warn_count() > 0)
}

fn list_rules() -> Result<()> {
    let mut w = std::io::stdout().lock();
    writeln!(w, "Available idealyst lint rules:\n")?;
    for r in lint::all_rules() {
        let level = match r.default_level {
            lint::Level::Off => "off",
            lint::Level::Warn => "warn",
            lint::Level::Error => "error",
        };
        writeln!(w, "  {:<24} [{level:>5}]  {}", r.id, r.summary)?;
    }
    writeln!(
        w,
        "\nOverride any rule in idealyst-lint.toml:\n\n  [rules]\n  {} = \"off\"\n",
        lint::all_rules()[0].id
    )?;
    Ok(())
}
