//! `port-project` CLI.
//!
//! Usage:
//!
//! ```text
//! port-project <path-or-git-url> [-o <output-dir>] [--report <path>]
//! ```
//!
//! - With a local path, walks the tree directly.
//! - With a URL (https / ssh / git@), clones depth-1 into a temp
//!   directory first, then walks the clone.
//! - Output defaults to `<cwd>/port-out`.
//! - Report defaults to `<output-dir>/REPORT.md`.

use std::path::PathBuf;
use std::process::ExitCode;

use port_project::{git, port_project, PortConfig};

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(2);
        }
    };

    // Resolve the input root: clone if URL, otherwise use as-is.
    let input_root = if git::is_url(&args.input) {
        match git::clone_to_temp(&args.input) {
            Ok(p) => {
                eprintln!("port-project: cloned to {}", p.display());
                p
            }
            Err(e) => {
                eprintln!("port-project: clone failed: {}", e);
                return ExitCode::from(1);
            }
        }
    } else {
        PathBuf::from(&args.input)
    };

    if !input_root.exists() {
        eprintln!("port-project: input not found: {}", input_root.display());
        return ExitCode::from(1);
    }

    let output_root = args
        .output
        .unwrap_or_else(|| std::env::current_dir().unwrap().join("port-out"));

    let report = match port_project(&PortConfig {
        input_root: &input_root,
        output_root: &output_root,
    }) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("port-project: {}", e);
            return ExitCode::from(1);
        }
    };

    let report_path = args.report.unwrap_or_else(|| output_root.join("REPORT.md"));
    let markdown = report.render_markdown();
    if let Err(e) = std::fs::write(&report_path, &markdown) {
        eprintln!("port-project: cannot write report: {}", e);
        return ExitCode::from(1);
    }

    print_summary(&report, &input_root, &output_root, &report_path);
    ExitCode::SUCCESS
}

struct Args {
    input: String,
    output: Option<PathBuf>,
    report: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut input: Option<String> = None;
    let mut output: Option<PathBuf> = None;
    let mut report: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--output" => {
                output = Some(args.next().ok_or("expected dir after -o")?.into());
            }
            "--report" => {
                report = Some(args.next().ok_or("expected path after --report")?.into());
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
    Ok(Args { input, output, report })
}

fn help_text() -> String {
    "port-project — port a React/Vue/Svelte project to idealyst Rust\n\n\
     usage: port-project <path-or-git-url> [-o <output-dir>] [--report <path>]\n\
     \n\
     - Local path: walks the tree directly.\n\
     - Git URL (https/ssh/git@): clones depth-1 to a temp dir.\n\
     - Output defaults to ./port-out; report to <output>/REPORT.md."
        .into()
}

fn print_summary(
    report: &port_project::report::ProjectReport,
    input_root: &std::path::Path,
    output_root: &std::path::Path,
    report_path: &std::path::Path,
) {
    use port_project::report::Status;
    let total = report.files.len();
    let ok = report.files.iter().filter(|f| f.status.is_ok()).count();
    let err = report
        .files
        .iter()
        .filter(|f| matches!(f.status, Status::Error(_)))
        .count();
    let skipped = report
        .files
        .iter()
        .filter(|f| matches!(f.status, Status::Skipped(_)))
        .count();
    let holes: usize = report.files.iter().map(|f| f.holes.len()).sum();
    eprintln!();
    eprintln!("  input:  {}", input_root.display());
    eprintln!("  output: {}", output_root.display());
    eprintln!("  report: {}", report_path.display());
    eprintln!();
    eprintln!(
        "  {total} files · {ok} ok · {err} error · {skipped} skipped · {holes} holes total"
    );
}
