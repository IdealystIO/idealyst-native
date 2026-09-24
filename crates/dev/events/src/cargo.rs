//! Cargo's output, as events.
//!
//! A build that wants progress and structured diagnostics runs cargo
//! with `--message-format=json-diagnostic-rendered-ansi` (or plain `json`
//! when colour is off) and both pipes captured, then feeds each stdout
//! line to [`CargoStream::stdout_line`] and each stderr line to
//! [`CargoStream::stderr_line`]:
//!
//! - `compiler-artifact` messages advance [`DevEvent::CargoProgress`];
//! - `compiler-message` messages become [`DevEvent::Diagnostic`]s, whose
//!   plain rendering is the text cargo would have printed itself;
//! - every stderr line (`Compiling …`, `Finished …`, `error: could not
//!   compile …`) is passed through verbatim as [`DevEvent::Output`], and a
//!   `Compiling <name>` line also names the crate in flight.
//!
//! # Where `total` comes from
//!
//! Cargo reports no unit total on stable: the unit graph
//! (`--unit-graph`) is `-Z unstable-options`, and the workspace builds on
//! stable. So the total is the size of the build's dependency closure,
//! from `cargo metadata --filter-platform <target>` ([`closure_size`]):
//! the packages reachable from the root through normal and build edges.
//! Progress counts distinct packages seen in `compiler-artifact`
//! messages — a build script and its library are one package, and a
//! crate built for both host and target is counted once — so the two
//! measure the same thing. The closure is resolved with the build's
//! features where the caller passes them, but a feature enabled only
//! deeper in the graph can still add a package metadata did not list;
//! the count is clamped to the total so the bar never overflows.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde_json::Value;

use crate::{Diagnostic, DevEvent};

/// The `source` tag cargo's verbatim output lines carry.
pub const SOURCE: &str = "cargo";

/// Turns one cargo invocation's output into events.
#[derive(Debug, Default)]
pub struct CargoStream {
    target: String,
    total: Option<u32>,
    seen: HashSet<String>,
    current: Option<String>,
    errors: u32,
}

impl CargoStream {
    /// `target` is the row the events are filed under (`web`).
    pub fn new(target: impl Into<String>) -> Self {
        Self { target: target.into(), ..Self::default() }
    }

    /// Set (or learn late) the closure size. Returns the progress event
    /// to emit so a consumer sees the total as soon as it is known.
    pub fn set_total(&mut self, total: Option<u32>) -> DevEvent {
        self.total = total;
        self.progress()
    }

    /// Distinct packages compiled (or found fresh) so far.
    pub fn compiled(&self) -> u32 {
        let n = self.seen.len() as u32;
        match self.total {
            Some(t) => n.min(t),
            None => n,
        }
    }

    /// Error-level diagnostics seen so far.
    pub fn errors(&self) -> u32 {
        self.errors
    }

    fn progress(&self) -> DevEvent {
        DevEvent::CargoProgress {
            target: self.target.clone(),
            compiled: self.compiled(),
            total: self.total,
            current: self.current.clone(),
        }
    }

    /// One line of cargo's stdout: a JSON message, or (rarely) plain
    /// text from something cargo ran.
    pub fn stdout_line(&mut self, line: &str) -> Vec<DevEvent> {
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return vec![DevEvent::Output { source: SOURCE.into(), line: line.to_string() }];
        };
        match msg["reason"].as_str() {
            Some("compiler-artifact") => {
                let id = msg["package_id"].as_str().unwrap_or_default().to_string();
                if !self.seen.insert(id) {
                    // A second artifact of a package already counted (its
                    // build script, or a host copy): no progress to report.
                    return Vec::new();
                }
                // `current` is left alone: an artifact is a crate that
                // FINISHED, and `current` names the one in flight, which
                // only cargo's `Compiling` line says.
                vec![self.progress()]
            }
            Some("compiler-message") => {
                let Some(diagnostic) = diagnostic(&msg) else { return Vec::new() };
                if diagnostic.is_error() {
                    self.errors += 1;
                }
                vec![DevEvent::Diagnostic { target: self.target.clone(), diagnostic }]
            }
            // `build-script-executed`, `build-finished`, and whatever
            // future cargo adds: nothing a consumer needs that the
            // process's exit status does not already say.
            _ => Vec::new(),
        }
    }

    /// One line of cargo's stderr, passed through verbatim.
    pub fn stderr_line(&mut self, line: &str) -> Vec<DevEvent> {
        let mut out = vec![DevEvent::Output { source: SOURCE.into(), line: line.to_string() }];
        // `   Compiling foo v0.1.0 (/path)` — the crate now in flight.
        // Matched on the plain text: cargo colours the verb when asked to.
        let plain = crate::plain::strip_ansi(line);
        if let Some(rest) = plain.trim_start().strip_prefix("Compiling ") {
            if let Some(name) = rest.split_whitespace().next() {
                self.current = Some(name.to_string());
                out.push(self.progress());
            }
        }
        out
    }
}

/// One line of a bare `rustc --error-format=json` stream (what a replayed
/// rustc invocation writes, with no cargo around it) as a [`Diagnostic`],
/// or `None` for anything else.
pub fn rustc_diagnostic(line: &str) -> Option<Diagnostic> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v["$message_type"] != "diagnostic" {
        return None;
    }
    diagnostic_of(&v, None)
}

/// A `compiler-message` as a [`Diagnostic`].
fn diagnostic(msg: &Value) -> Option<Diagnostic> {
    diagnostic_of(&msg["message"], msg["package_id"].as_str())
}

fn diagnostic_of(m: &Value, package: Option<&str>) -> Option<Diagnostic> {
    let level = m["level"].as_str()?.to_string();
    let message = m["message"].as_str().unwrap_or_default().to_string();
    let raw = m["rendered"].as_str().unwrap_or_default();
    let rendered = crate::plain::strip_ansi(raw);
    let ansi = (rendered != raw).then(|| raw.to_string());
    let primary = m["spans"]
        .as_array()
        .and_then(|spans| spans.iter().find(|s| s["is_primary"].as_bool() == Some(true)));
    Some(Diagnostic {
        level,
        message,
        code: m["code"]["code"].as_str().map(str::to_string),
        file: primary.and_then(|s| s["file_name"].as_str()).map(str::to_string),
        line: primary.and_then(|s| s["line_start"].as_u64()).map(|n| n as u32),
        column: primary.and_then(|s| s["column_start"].as_u64()).map(|n| n as u32),
        package: package.map(str::to_string),
        rendered,
        ansi,
    })
}

/// The number of packages in the root package's dependency closure,
/// from `cargo metadata --format-version 1` output. Normal and build
/// edges only: dev-dependencies are not part of a `cargo build`.
///
/// `None` when the metadata has no resolve graph or no root (a virtual
/// manifest) — the progress bar then shows a count with no total.
pub fn closure_size(metadata: &Value) -> Option<u32> {
    let root = metadata.pointer("/resolve/root")?.as_str()?;
    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for node in metadata.pointer("/resolve/nodes")?.as_array()? {
        let Some(id) = node["id"].as_str() else { continue };
        let deps = node["deps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| {
                // `dep_kinds[].kind` is null for normal, "build", or
                // "dev". Keep a dep reachable through any non-dev kind.
                d["dep_kinds"].as_array().map_or(true, |kinds| {
                    kinds.iter().any(|k| k["kind"].as_str() != Some("dev"))
                })
            })
            .filter_map(|d| d["pkg"].as_str())
            .collect();
        edges.insert(id, deps);
    }
    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        stack.extend(edges.get(id).into_iter().flatten().copied());
    }
    Some(seen.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_rustc_diagnostic_line_parses_and_anything_else_does_not() {
        let line = r#"{"$message_type":"diagnostic","message":"mismatched types","code":{"code":"E0308"},"level":"error","spans":[{"file_name":"src/app.rs","line_start":12,"column_start":9,"is_primary":true}],"rendered":"\u001b[1merror[E0308]\u001b[0m: mismatched types\n"}"#;
        let d = rustc_diagnostic(line).expect("a diagnostic");
        assert_eq!(d.location().as_deref(), Some("src/app.rs:12:9"));
        assert_eq!(d.rendered, "error[E0308]: mismatched types\n");
        assert!(d.ansi.is_some());
        assert!(rustc_diagnostic(r#"{"$message_type":"artifact","artifact":"x.o"}"#).is_none());
        assert!(rustc_diagnostic("error: plain text").is_none());
    }
}
