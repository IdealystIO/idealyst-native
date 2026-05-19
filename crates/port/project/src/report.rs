//! Project-level port report.
//!
//! Aggregates per-file results into a markdown summary the user
//! can read to evaluate "is this porter useful for my project?"

use std::collections::BTreeMap;
use std::path::PathBuf;

use port_core::ir::{Hole, HoleKind};

use crate::route::Frontend;

#[derive(Debug)]
pub struct ProjectReport {
    pub files: Vec<FilePort>,
}

#[derive(Debug)]
pub struct FilePort {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub frontend: Option<Frontend>,
    pub status: Status,
    pub holes: Vec<Hole>,
    pub bytes_in: usize,
    pub bytes_out: usize,
    /// Names of exported components the porter found in this file
    /// (PascalCase, as written in the source). Empty for error or
    /// skipped files. Used by `port-preview` to locate a root
    /// component for the generated `main.rs`.
    pub components: Vec<String>,
}

#[derive(Debug)]
pub enum Status {
    Ok,
    Skipped(String),
    Error(String),
}

impl Status {
    pub fn is_ok(&self) -> bool {
        matches!(self, Status::Ok)
    }
    pub fn label(&self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Skipped(_) => "skipped",
            Status::Error(_) => "error",
        }
    }
}

impl ProjectReport {
    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Port report\n\n");

        // Top-level counts.
        let total = self.files.len();
        let ok = self.files.iter().filter(|f| f.status.is_ok()).count();
        let err = self
            .files
            .iter()
            .filter(|f| matches!(f.status, Status::Error(_)))
            .count();
        let skip = self
            .files
            .iter()
            .filter(|f| matches!(f.status, Status::Skipped(_)))
            .count();
        out.push_str(&format!("**Files**: {total} total · {ok} ok · {err} error · {skip} skipped\n\n"));

        // By-frontend breakdown.
        let mut by_frontend: BTreeMap<&'static str, (usize, usize, usize)> = BTreeMap::new();
        for f in &self.files {
            let label = f.frontend.map(|f| f.label()).unwrap_or("unknown");
            let entry = by_frontend.entry(label).or_insert((0, 0, 0));
            entry.0 += 1;
            if f.status.is_ok() {
                entry.1 += 1;
            }
            entry.2 += f.holes.len();
        }
        out.push_str("## By frontend\n\n");
        out.push_str("| Frontend | Files | OK | Holes |\n");
        out.push_str("| --- | ---: | ---: | ---: |\n");
        for (label, (total, ok, holes)) in &by_frontend {
            out.push_str(&format!("| {} | {} | {} | {} |\n", label, total, ok, holes));
        }
        out.push('\n');

        // Hole histogram by kind.
        let mut by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
        for f in &self.files {
            for h in &f.holes {
                *by_kind.entry(kind_label(h.kind)).or_insert(0) += 1;
            }
        }
        if !by_kind.is_empty() {
            out.push_str("## Holes by kind\n\n");
            out.push_str("| Kind | Count |\n| --- | ---: |\n");
            let mut sorted: Vec<_> = by_kind.iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(a.1));
            for (k, n) in sorted {
                out.push_str(&format!("| {} | {} |\n", k, n));
            }
            out.push('\n');
        }

        // Top recurring hole reasons (which idioms are biting most often).
        let mut by_reason: BTreeMap<String, usize> = BTreeMap::new();
        for f in &self.files {
            for h in &f.holes {
                *by_reason.entry(h.reason.clone()).or_insert(0) += 1;
            }
        }
        if !by_reason.is_empty() {
            out.push_str("## Top hole reasons\n\n");
            let mut sorted: Vec<_> = by_reason.iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(a.1));
            for (reason, n) in sorted.iter().take(10) {
                out.push_str(&format!("- **{}×** {}\n", n, reason));
            }
            out.push('\n');
        }

        // Per-file table.
        out.push_str("## Files\n\n");
        out.push_str("| Status | Frontend | Holes | Input | Output |\n");
        out.push_str("| --- | --- | ---: | --- | --- |\n");
        for f in &self.files {
            let status = f.status.label();
            let frontend = f.frontend.map(|f| f.label()).unwrap_or("-");
            let input = f.input.display();
            let output = f.output.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "-".into());
            out.push_str(&format!(
                "| {} | {} | {} | `{}` | `{}` |\n",
                status,
                frontend,
                f.holes.len(),
                input,
                output,
            ));
        }
        out.push('\n');

        // Failure detail.
        let failures: Vec<_> = self
            .files
            .iter()
            .filter(|f| matches!(f.status, Status::Error(_)))
            .collect();
        if !failures.is_empty() {
            out.push_str("## Errors\n\n");
            for f in failures {
                if let Status::Error(msg) = &f.status {
                    out.push_str(&format!("- `{}`: {}\n", f.input.display(), msg));
                }
            }
            out.push('\n');
        }

        out
    }
}

fn kind_label(k: HoleKind) -> &'static str {
    match k {
        HoleKind::HandlerBody => "handler-body",
        HoleKind::AttributeValue => "attribute-value",
        HoleKind::PropType => "prop-type",
        HoleKind::Unsupported => "unsupported",
    }
}
