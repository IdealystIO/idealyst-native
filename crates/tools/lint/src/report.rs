//! Output formatters.
//!
//! Two formats share the same [`Diagnostic`] stream:
//!
//! - [`human`] — a compact, caret-underlined terminal report for
//!   `idealyst lint`.
//! - [`cargo_json`] — one `compiler-message` JSON object per line in the
//!   shape `cargo check --message-format=json` emits, so rust-analyzer's
//!   flycheck (`rust-analyzer.check.overrideCommand`) renders the findings
//!   as inline squiggles. This is the "RA extension" surface: RA doesn't
//!   load a plugin, it runs this command and parses its stdout.

use std::io::{self, Write};
use std::path::Path;

use serde_json::json;

use crate::diagnostic::{Diagnostic, Severity};
use crate::engine::LintRun;

/// Render a run as a human-readable report. Returns the number of bytes is
/// irrelevant; errors propagate from the writer.
pub fn human(run: &LintRun, w: &mut impl Write, color: bool) -> io::Result<()> {
    for d in &run.diagnostics {
        let (tag, paint) = match d.severity {
            Severity::Error => ("error", Paint::Red),
            Severity::Warn => ("warning", Paint::Yellow),
        };
        writeln!(
            w,
            "{}: {}",
            paint.wrap(color, &format!("{tag}[{}]", d.rule)),
            d.message
        )?;
        writeln!(
            w,
            "  {} {}:{}:{}",
            Paint::Blue.wrap(color, "-->"),
            d.file.display(),
            d.line_start,
            d.col_start
        )?;
        // Source line + caret underline beneath the offending span.
        if !d.source_line.is_empty() {
            let gutter = format!("{:>4} | ", d.line_start);
            writeln!(w, "{gutter}{}", d.source_line)?;
            let pad = " ".repeat(gutter.len() + d.col_start.saturating_sub(1));
            let span_len = caret_len(d);
            writeln!(w, "{pad}{}", Paint::Red.wrap(color, &"^".repeat(span_len)))?;
        }
        if let Some(help) = &d.help {
            writeln!(w, "  {} {help}", Paint::Green.wrap(color, "help:"))?;
        }
        writeln!(w)?;
    }

    for (path, err) in &run.parse_errors {
        writeln!(
            w,
            "{}: could not parse {} ({err})",
            Paint::Yellow.wrap(color, "warning[parse]"),
            path.display()
        )?;
    }

    summary(run, w, color)
}

fn caret_len(d: &Diagnostic) -> usize {
    if d.line_end == d.line_start && d.col_end > d.col_start {
        (d.col_end - d.col_start).max(1)
    } else {
        1
    }
}

fn summary(run: &LintRun, w: &mut impl Write, color: bool) -> io::Result<()> {
    let errors = run.error_count();
    let warnings = run.warn_count();
    let total = errors + warnings;
    if total == 0 && run.parse_errors.is_empty() {
        writeln!(
            w,
            "{} {} file(s), no problems",
            Paint::Green.wrap(color, "✓"),
            run.files_scanned
        )?;
        return Ok(());
    }
    let mark = if errors > 0 { Paint::Red.wrap(color, "✖") } else { Paint::Yellow.wrap(color, "⚠") };
    writeln!(
        w,
        "{mark} {total} problem(s) — {errors} error(s), {warnings} warning(s) across {} file(s)",
        run.files_scanned
    )
}

/// Emit cargo-style `--message-format=json`. Each diagnostic becomes one
/// `{"reason":"compiler-message", …}` line; a trailing
/// `{"reason":"build-finished", …}` tells rust-analyzer the run is done.
///
/// rust-analyzer deserializes these via `cargo_metadata::Message`, which
/// requires the `package_id` / `manifest_path` / `target` envelope around
/// the rustc `message`. We synthesize plausible values for those; RA only
/// uses the inner `message.spans[*]` (file + line/column) to place the
/// squiggle and `message.level` / `message.message` for its text.
pub fn cargo_json(run: &LintRun, root: &Path, w: &mut impl Write) -> io::Result<()> {
    let manifest = root.join("Cargo.toml");
    for d in &run.diagnostics {
        let msg = rustc_message(d);
        let envelope = json!({
            "reason": "compiler-message",
            "package_id": "idealyst-lint 0.0.0 (path+file:///)",
            "manifest_path": manifest.to_string_lossy(),
            "target": {
                "kind": ["lib"],
                "crate_types": ["lib"],
                "name": "idealyst-lint",
                "src_path": d.file.to_string_lossy(),
                "edition": "2021",
                "doc": false,
                "doctest": false,
                "test": false,
            },
            "message": msg,
        });
        writeln!(w, "{}", serde_json::to_string(&envelope).expect("serialize diagnostic"))?;
    }
    let finished = json!({
        "reason": "build-finished",
        "success": !run.failed(),
    });
    writeln!(w, "{}", serde_json::to_string(&finished).expect("serialize build-finished"))
}

/// Build the inner rustc `Diagnostic` JSON for one finding.
fn rustc_message(d: &Diagnostic) -> serde_json::Value {
    let code = format!("idealyst::{}", d.rule);
    let level = d.severity.as_rustc_level();

    let span = json!({
        "file_name": d.file.to_string_lossy(),
        "byte_start": d.byte_start,
        "byte_end": d.byte_end,
        "line_start": d.line_start,
        "line_end": d.line_end,
        "column_start": d.col_start,
        "column_end": d.col_end,
        "is_primary": true,
        "text": [{
            "text": d.source_line,
            "highlight_start": d.col_start,
            "highlight_end": d.col_end.max(d.col_start + 1),
        }],
        "label": serde_json::Value::Null,
        "suggested_replacement": serde_json::Value::Null,
        "suggestion_applicability": serde_json::Value::Null,
        "expansion": serde_json::Value::Null,
    });

    // A `help` child mirrors how rustc attaches help text, so RA shows it
    // in the diagnostic's related-information.
    let children = match &d.help {
        Some(help) => json!([{
            "message": help,
            "code": serde_json::Value::Null,
            "level": "help",
            "spans": [],
            "children": [],
            "rendered": serde_json::Value::Null,
        }]),
        None => json!([]),
    };

    json!({
        "message": d.message,
        "code": { "code": code, "explanation": serde_json::Value::Null },
        "level": level,
        "spans": [span],
        "children": children,
        "rendered": rendered(d),
    })
}

/// A plain-text rendering RA can show on hover and CI logs can echo.
fn rendered(d: &Diagnostic) -> String {
    let mut s = format!(
        "{}[idealyst::{}]: {}\n --> {}:{}:{}\n",
        d.severity.as_rustc_level(),
        d.rule,
        d.message,
        d.file.display(),
        d.line_start,
        d.col_start,
    );
    if let Some(help) = &d.help {
        s.push_str(&format!("help: {help}\n"));
    }
    s
}

/// Minimal ANSI color, gated on a `color` flag so piped/CI output stays
/// plain. Kept local to avoid a dependency for five escape codes.
#[derive(Clone, Copy)]
enum Paint {
    Red,
    Yellow,
    Green,
    Blue,
}

impl Paint {
    fn code(self) -> &'static str {
        match self {
            Paint::Red => "31",
            Paint::Yellow => "33",
            Paint::Green => "32",
            Paint::Blue => "34",
        }
    }

    fn wrap(self, color: bool, s: &str) -> String {
        if color {
            format!("\u{1b}[1;{}m{s}\u{1b}[0m", self.code())
        } else {
            s.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::engine::lint_source;

    fn run_for(src: &str) -> LintRun {
        let diags = lint_source(src, Path::new("src/app.rs"), &Config::default()).unwrap();
        LintRun { diagnostics: diags, files_scanned: 1, parse_errors: Vec::new() }
    }

    #[test]
    fn json_emits_compiler_message_and_build_finished() {
        let run = run_for("fn f() { Signal::new(0); }");
        let mut buf = Vec::new();
        cargo_json(&run, Path::new("/proj"), &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "one message + build-finished");

        let msg: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(msg["reason"], "compiler-message");
        assert_eq!(msg["message"]["level"], "warning");
        assert_eq!(msg["message"]["code"]["code"], "idealyst::prefer-signal-macro");
        assert_eq!(msg["message"]["spans"][0]["is_primary"], true);
        assert_eq!(msg["message"]["spans"][0]["line_start"], 1);
        // Help text rides along as a child.
        assert_eq!(msg["message"]["children"][0]["level"], "help");

        let fin: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(fin["reason"], "build-finished");
        assert_eq!(fin["success"], true); // warning-only run does not fail
    }

    #[test]
    fn json_build_finished_false_on_error() {
        // component-pascal-case defaults to error.
        let run = run_for("#[component]\nfn my_thing() -> Element { todo!() }");
        let mut buf = Vec::new();
        cargo_json(&run, Path::new("/proj"), &mut buf).unwrap();
        let last = String::from_utf8(buf).unwrap().lines().last().unwrap().to_string();
        let fin: serde_json::Value = serde_json::from_str(&last).unwrap();
        assert_eq!(fin["success"], false);
    }

    #[test]
    fn human_output_contains_rule_and_location() {
        let run = run_for("fn f() { Signal::new(0); }");
        let mut buf = Vec::new();
        human(&run, &mut buf, false).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("warning[prefer-signal-macro]"));
        assert!(text.contains("src/app.rs:1:10"));
        assert!(text.contains("help:"));
        assert!(text.contains("1 warning"));
    }
}
