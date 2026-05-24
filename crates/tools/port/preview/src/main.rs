//! `port-preview` CLI.
//!
//! Usage:
//!
//! ```text
//! port-preview <path-or-git-url> [-o <output-dir>] [--framework-path <path>]
//! ```
//!
//! Ports the given project, scaffolds a scratch Cargo crate, runs
//! `cargo check`. Reports port-level statistics and compile result.

use std::path::PathBuf;
use std::process::ExitCode;

use port_preview::{discover_runtime_core, preview, CheckResult, PreviewConfig};
use port_project::git;
use port_project::report::Status;

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(2);
        }
    };

    // Resolve the input: git clone if URL, otherwise use as-is.
    let input = if git::is_url(&args.input) {
        match git::clone_to_temp(&args.input) {
            Ok(p) => {
                eprintln!("port-preview: cloned to {}", p.display());
                p
            }
            Err(e) => {
                eprintln!("port-preview: clone failed: {}", e);
                return ExitCode::from(1);
            }
        }
    } else {
        PathBuf::from(&args.input)
    };
    if !input.exists() {
        eprintln!("port-preview: input not found: {}", input.display());
        return ExitCode::from(1);
    }

    // Resolve runtime-core. Explicit `--framework-path` wins;
    // otherwise we walk up from the binary's launch dir looking
    // for `crates/framework/core/Cargo.toml`. This is the path
    // people running inside the idealyst-native checkout will
    // hit by default.
    let framework_path = match args.framework_path {
        Some(p) => p,
        None => {
            let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match discover_runtime_core(&here) {
                Some(p) => p,
                None => {
                    eprintln!(
                        "port-preview: could not find runtime-core. \
                         Pass --framework-path <path-to-crates/framework/core>."
                    );
                    return ExitCode::from(1);
                }
            }
        }
    };

    let output_dir = args
        .output
        .unwrap_or_else(|| std::env::current_dir().unwrap().join("port-preview-out"));

    let outcome = match preview(&PreviewConfig {
        input: &input,
        output_dir: &output_dir,
        runtime_core_path: &framework_path,
    }) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("port-preview: {}", e);
            return ExitCode::from(1);
        }
    };

    print_summary(&outcome);

    match &outcome.check_result {
        CheckResult::Ok => ExitCode::SUCCESS,
        // Compile failures aren't a port-tool error per se — the
        // tool worked, the result just doesn't compile. Still
        // signal non-zero so CI / scripts can detect it.
        CheckResult::Failed { .. } => ExitCode::from(3),
        CheckResult::DidNotRun { .. } => ExitCode::from(4),
    }
}

struct Args {
    input: String,
    output: Option<PathBuf>,
    framework_path: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut input: Option<String> = None;
    let mut output: Option<PathBuf> = None;
    let mut framework_path: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--output" => {
                output = Some(args.next().ok_or("expected dir after -o")?.into());
            }
            "--framework-path" => {
                framework_path = Some(args.next().ok_or("expected path after --framework-path")?.into());
            }
            "-h" | "--help" => return Err(help_text()),
            other if other.starts_with('-') => {
                return Err(format!("unknown flag: {}\n\n{}", other, help_text()));
            }
            _ => {
                if input.is_some() {
                    return Err(format!("unexpected: {}\n\n{}", arg, help_text()));
                }
                input = Some(arg);
            }
        }
    }
    let input = input.ok_or_else(|| format!("missing <path-or-git-url>\n\n{}", help_text()))?;
    Ok(Args { input, output, framework_path })
}

fn help_text() -> String {
    "port-preview — port a project to Rust and check it compiles\n\n\
     usage: port-preview <path-or-git-url> [-o <output-dir>] [--framework-path <path>]\n\
     \n\
     - Local path: walks the tree directly.\n\
     - Git URL (https/ssh/git@): clones depth-1 to a temp dir.\n\
     - --framework-path: location of `crates/framework/core` (auto-discovered\n\
       by walking up from cwd if omitted).\n\
     - Output defaults to ./port-preview-out."
        .into()
}

fn print_summary(outcome: &port_preview::PreviewOutcome) {
    let total = outcome.project_report.files.len();
    let ok = outcome
        .project_report
        .files
        .iter()
        .filter(|f| f.status.is_ok())
        .count();
    let err = outcome
        .project_report
        .files
        .iter()
        .filter(|f| matches!(f.status, Status::Error(_)))
        .count();
    let skipped = outcome
        .project_report
        .files
        .iter()
        .filter(|f| matches!(f.status, Status::Skipped(_)))
        .count();
    let holes: usize = outcome.project_report.files.iter().map(|f| f.holes.len()).sum();

    eprintln!();
    eprintln!("  scratch crate: {}", outcome.scratch_dir.display());
    eprintln!();
    eprintln!("  port: {total} files · {ok} ok · {err} error · {skipped} skipped · {holes} holes");
    match &outcome.check_result {
        CheckResult::Ok => {
            eprintln!("  build: ok ✓");
        }
        CheckResult::Failed { stderr } => {
            let lines: Vec<&str> = stderr.lines().collect();
            let error_count = lines
                .iter()
                .filter(|l| l.starts_with("error[") || l.starts_with("error:"))
                .count();
            eprintln!("  build: FAILED ({error_count} compiler errors)");
            eprintln!();
            eprintln!("  --- cargo check stderr (first 30 lines) ---");
            for line in lines.iter().take(30) {
                eprintln!("    {}", line);
            }
            if lines.len() > 30 {
                eprintln!("    … {} more lines", lines.len() - 30);
            }
        }
        CheckResult::DidNotRun { reason } => {
            eprintln!("  build: could not run cargo check: {}", reason);
        }
    }
}
