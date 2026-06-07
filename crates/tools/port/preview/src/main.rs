//! `port-preview` CLI.
//!
//! Usage:
//!
//! ```text
//! port-preview <path-or-git-url> [-o <output-dir>] [--workspace-root <path>]
//!                                [--root-name <Name>] [--root-file <path>]
//! ```
//!
//! Ports the given project, scaffolds a scratch Cargo crate, runs
//! `cargo check`. Reports port-level statistics and compile result.

use std::path::PathBuf;
use std::process::ExitCode;

use port_preview::{discover_workspace_root, preview, CheckResult, PreviewConfig};
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

    // Resolve the idealyst-native checkout root (the scratch crate
    // references runtime-core et al. via path deps under it). Explicit
    // `--workspace-root` wins; otherwise we walk up from the binary's
    // launch dir looking for `crates/framework/core/Cargo.toml`. This is
    // the path people running inside the checkout will hit by default.
    let workspace_root = match args.workspace_root {
        Some(p) => p,
        None => {
            let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match discover_workspace_root(&here) {
                Some(p) => p,
                None => {
                    eprintln!(
                        "port-preview: could not find the idealyst-native checkout root. \
                         Pass --workspace-root <path-to-checkout>."
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
        workspace_root: &workspace_root,
        root_name: &args.root_name,
        root_file: args.root_file.as_deref(),
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
    workspace_root: Option<PathBuf>,
    root_name: String,
    root_file: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut input: Option<String> = None;
    let mut output: Option<PathBuf> = None;
    let mut workspace_root: Option<PathBuf> = None;
    let mut root_name: Option<String> = None;
    let mut root_file: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--output" => {
                output = Some(args.next().ok_or("expected dir after -o")?.into());
            }
            "--workspace-root" => {
                workspace_root =
                    Some(args.next().ok_or("expected path after --workspace-root")?.into());
            }
            "--root-name" => {
                root_name = Some(args.next().ok_or("expected name after --root-name")?);
            }
            "--root-file" => {
                root_file = Some(args.next().ok_or("expected path after --root-file")?.into());
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
    Ok(Args {
        input,
        output,
        workspace_root,
        root_name: root_name.unwrap_or_else(|| "App".to_string()),
        root_file,
    })
}

fn help_text() -> String {
    "port-preview — port a project to Rust and check it compiles\n\n\
     usage: port-preview <path-or-git-url> [-o <output-dir>] [--workspace-root <path>]\n\
     \x20                              [--root-name <Name>] [--root-file <path>]\n\
     \n\
     - Local path: walks the tree directly.\n\
     - Git URL (https/ssh/git@): clones depth-1 to a temp dir.\n\
     - --workspace-root: idealyst-native checkout root (auto-discovered by\n\
       walking up from cwd if omitted).\n\
     - --root-name: PascalCase render-root component (default: App).\n\
     - --root-file: only match the root component in files whose path ends\n\
       with this suffix (e.g. src/App.tsx).\n\
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
