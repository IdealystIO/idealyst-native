//! Shared CLI driver for per-framework porter binaries.
//!
//! Each `port-*` crate has a one-line `main.rs`:
//!
//! ```ignore
//! fn main() -> std::process::ExitCode {
//!     port_core::cli::run("port-react", port_react::port)
//! }
//! ```
//!
//! Centralizing here means arg parsing, output dispatch, and hole
//! summary stay in sync across frontends — there's no good reason
//! they would diverge.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::ir::{HoleKind, PortReport};
use crate::ParseError;

/// Signature every porter exposes from its crate root.
pub type PortFn = fn(&str) -> Result<(String, PortReport), ParseError>;

pub fn run(tool_name: &str, port_fn: PortFn) -> ExitCode {
    let args = match parse_args(tool_name) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(2);
        }
    };

    let source = match fs::read_to_string(&args.input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: cannot read {}: {}", tool_name, args.input.display(), e);
            return ExitCode::from(1);
        }
    };

    let (rendered, report) = match port_fn(&source) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}: parse failed: {}", tool_name, e);
            return ExitCode::from(1);
        }
    };

    match args.output {
        Some(path) => {
            if let Err(e) = fs::write(&path, &rendered) {
                eprintln!("{}: cannot write {}: {}", tool_name, path.display(), e);
                return ExitCode::from(1);
            }
        }
        None => {
            let _ = std::io::stdout().write_all(rendered.as_bytes());
        }
    }

    print_summary(tool_name, &report);
    ExitCode::SUCCESS
}

struct Args {
    input: PathBuf,
    output: Option<PathBuf>,
}

fn parse_args(tool_name: &str) -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--output" => {
                output = Some(args.next()
                    .ok_or_else(|| "expected path after -o".to_string())?
                    .into());
            }
            "-h" | "--help" => return Err(help_text(tool_name)),
            other if other.starts_with('-') => {
                return Err(format!("unknown flag: {}\n\n{}", other, help_text(tool_name)));
            }
            _ => {
                if input.is_some() {
                    return Err(format!("unexpected positional: {}\n\n{}", arg, help_text(tool_name)));
                }
                input = Some(arg.into());
            }
        }
    }
    let input = input.ok_or_else(|| format!("missing <input>\n\n{}", help_text(tool_name)))?;
    Ok(Args { input, output })
}

fn help_text(tool_name: &str) -> String {
    format!(
        "{tool} — translate source to idealyst Rust\n\n\
         usage: {tool} <input> [-o <output.rs>]\n\
         \n\
         Outputs Rust source using `#[component]`, `signal!`, and\n\
         `jsx!`. Unsupported spots are emitted as `todo!(\"port …\")`\n\
         stubs with the original snippet inline.",
        tool = tool_name,
    )
}

fn print_summary(tool_name: &str, report: &PortReport) {
    let total = report.holes.len();
    if total == 0 {
        eprintln!("{}: 0 holes", tool_name);
        return;
    }
    eprintln!("{}: {} hole(s) to resolve:", tool_name, total);
    for kind in [
        HoleKind::HandlerBody,
        HoleKind::AttributeValue,
        HoleKind::PropType,
        HoleKind::Unsupported,
    ] {
        let n = report.by_kind(kind);
        if n > 0 {
            eprintln!("  {n:>3} {kind}");
        }
    }
}
