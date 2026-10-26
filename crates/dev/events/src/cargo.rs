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
//!   `Compiling <name>` line also names the crate in flight;
//! - a `compiler-artifact` that is not `fresh` is a unit cargo actually
//!   recompiled, and becomes [`DevEvent::UnitBuilt`], timed from that
//!   package's `Compiling` line;
//! - with rebuild explanation on ([`crate::process::set_explain_rebuilds`]),
//!   cargo's fingerprint log on stderr becomes [`DevEvent::UnitDirty`]
//!   instead of output lines (see "Why a unit was rebuilt").
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

//!
//! # Why a unit was rebuilt
//!
//! Cargo says why it considers a unit dirty only in its own tracing log,
//! `CARGO_LOG=cargo::core::compiler::fingerprint=info` ([`FINGERPRINT_LOG`]).
//! Each line names the unit in its span (`prepare_target{… package_id=foo
//! v0.1.0 (/path) target="foo"}`) and the reason after the module path:
//!
//! ```text
//! … cargo::core::compiler::fingerprint:     dirty: FsStatusOutdated(StaleItem(ChangedFile { …, stale: "/p/src/lib.rs", … }))
//! … cargo::core::compiler::fingerprint:     err: failed to read `/t/debug/.fingerprint/foo-1f5c…/lib-foo`
//! ```
//!
//! The second shape is a unit with no fingerprint at its current hash: the
//! first build of it with these features, flags or profile, or one whose
//! artifacts were removed. It is how a feature-unification flip between two
//! builds sharing a target dir shows up, and it is typically the whole tree.
//! An `err:` line is followed by an anyhow `Caused by:` chain, which is
//! swallowed with it. [`dirty_reason`] maps a reason to its kind. The log
//! is not a stable cargo interface, so the parse is tolerant: an unknown
//! reason keeps its raw text, and a line that does not parse is dropped
//! rather than shown.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Instant;

use serde_json::Value;

use crate::{Diagnostic, DevEvent, DirtyReason};

/// The `CARGO_LOG` filter that makes cargo log why each unit is dirty.
pub const FINGERPRINT_LOG: &str = "cargo::core::compiler::fingerprint=info";

/// The module path every fingerprint log line carries before its message.
const FINGERPRINT_MODULE: &str = "cargo::core::compiler::fingerprint:";

/// [`DirtyReason::kind`] of a unit rebuilt because a dependency was.
pub const DEPENDENCY_REBUILT: &str = "dependency_rebuilt";

/// [`DirtyReason::kind`] of a unit with no fingerprint at its current hash.
pub const NO_PRIOR_BUILD: &str = "no_prior_build";

/// Longest [`DirtyReason::detail`] kept: a `FeaturesChanged` on a large
/// crate lists every feature twice.
const MAX_DETAIL: usize = 600;

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
    executable: Option<String>,
    unit_messages: Vec<String>,
    /// When each package's `Compiling` line arrived, by (name, version).
    compiling: HashMap<(String, String), Instant>,
    /// The last stderr line was part of a fingerprint `err:` report, whose
    /// `Caused by:` chain follows on lines of its own.
    in_fingerprint_error: bool,
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

    /// The last executable cargo reported building (a `compiler-artifact`
    /// with a non-null `executable`) — for a `--bin` build, that binary.
    /// Fresh artifacts count: cargo reports them too, with the path.
    pub fn executable(&self) -> Option<&str> {
        self.executable.as_deref()
    }

    /// Every `compiler-artifact` and `build-script-executed` message, raw:
    /// the build's units, for a caller that records them
    /// (`build_ios::target_gc` prunes a target dir to what recorded builds
    /// use). Fresh units are reported too.
    pub fn unit_messages(&self) -> &[String] {
        &self.unit_messages
    }

    /// A verbatim line, filed under this build's target.
    fn output(&self, line: &str) -> DevEvent {
        DevEvent::Output {
            source: SOURCE.into(),
            line: line.to_string(),
            target: Some(self.target.clone()),
        }
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
        self.stdout_line_at(line, Instant::now())
    }

    fn stdout_line_at(&mut self, line: &str, now: Instant) -> Vec<DevEvent> {
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            return vec![self.output(line)];
        };
        match msg["reason"].as_str() {
            Some("build-script-executed") => {
                self.unit_messages.push(line.to_string());
                Vec::new()
            }
            Some("compiler-artifact") => {
                self.unit_messages.push(line.to_string());
                // Before the package check: a bin's artifact arrives after
                // its package's lib artifact was already counted.
                if let Some(exe) = msg["executable"].as_str() {
                    self.executable = Some(exe.to_string());
                }
                let id = msg["package_id"].as_str().unwrap_or_default().to_string();
                let built = (msg["fresh"].as_bool() == Some(false))
                    .then(|| self.unit_built(&id, &msg, now))
                    .flatten();
                let mut out: Vec<DevEvent> = built.into_iter().collect();
                // A second artifact of a package already counted (its
                // build script, or a host copy) is no progress.
                if self.seen.insert(id) {
                    // `current` is left alone: an artifact is a crate that
                    // FINISHED, and `current` names the one in flight, which
                    // only cargo's `Compiling` line says.
                    out.push(self.progress());
                }
                out
            }
            Some("compiler-message") => {
                let Some(diagnostic) = diagnostic(&msg) else { return Vec::new() };
                if diagnostic.is_error() {
                    self.errors += 1;
                }
                vec![DevEvent::Diagnostic { target: self.target.clone(), diagnostic }]
            }
            // `build-finished`, and whatever future cargo adds: nothing a consumer needs that the
            // process's exit status does not already say.
            _ => Vec::new(),
        }
    }

    /// The [`DevEvent::UnitBuilt`] for a non-fresh artifact of `id`.
    fn unit_built(&self, id: &str, msg: &Value, now: Instant) -> Option<DevEvent> {
        let (package, version) = package_name_version(id)?;
        let kind = msg["target"]["kind"][0].as_str().unwrap_or("unknown").to_string();
        let ms = self
            .compiling
            .get(&(package.clone(), version.clone()))
            .map(|t| now.saturating_duration_since(*t).as_millis() as u64);
        Some(DevEvent::UnitBuilt { target: self.target.clone(), package, version, kind, ms })
    }

    /// One line of cargo's stderr, passed through verbatim — except cargo's
    /// fingerprint log, which becomes [`DevEvent::UnitDirty`] or nothing.
    pub fn stderr_line(&mut self, line: &str) -> Vec<DevEvent> {
        self.stderr_line_at(line, Instant::now())
    }

    fn stderr_line_at(&mut self, line: &str, now: Instant) -> Vec<DevEvent> {
        // Matched on the plain text: cargo colours the verb when asked to.
        let plain = crate::plain::strip_ansi(line);
        if let Some(at) = plain.find(FINGERPRINT_MODULE) {
            let message = plain[at + FINGERPRINT_MODULE.len()..].trim();
            self.in_fingerprint_error = message.starts_with("err:");
            return self.fingerprint_line(&plain[..at], message).into_iter().collect();
        }
        if self.in_fingerprint_error {
            if is_error_chain_line(&plain) {
                return Vec::new();
            }
            self.in_fingerprint_error = false;
        }
        let mut out = vec![self.output(line)];
        // `   Compiling foo v0.1.0 (/path)` — the crate now in flight.
        if let Some(rest) = plain.trim_start().strip_prefix("Compiling ") {
            let mut words = rest.split_whitespace();
            if let Some(name) = words.next() {
                if let Some(version) = words.next().and_then(|v| v.strip_prefix('v')) {
                    self.compiling.insert((name.to_string(), version.to_string()), now);
                }
                self.current = Some(name.to_string());
                out.push(self.progress());
            }
        }
        out
    }

    /// A fingerprint log line: `span` is everything before the module
    /// path (it names the unit), `message` what follows it.
    fn fingerprint_line(&self, span: &str, message: &str) -> Option<DevEvent> {
        let reason = if let Some(r) = message.strip_prefix("dirty:") {
            dirty_reason(r.trim())
        } else if let Some(e) = message.strip_prefix("err:") {
            let e = e.trim();
            let kind = if e.starts_with("failed to read") { NO_PRIOR_BUILD } else { "fingerprint_error" };
            DirtyReason { kind: kind.into(), detail: truncate(e) }
        } else {
            // `fingerprint dirty for …`, `stale: changed …`, `(vs) …`: the
            // lines around a `dirty:` that the `dirty:` line already sums up.
            return None;
        };
        let rest = &span[span.find("package_id=")? + "package_id=".len()..];
        let mut words = rest.split_whitespace();
        let package = words.next()?.to_string();
        let version = words.next()?.trim_start_matches('v').to_string();
        let unit = span
            .find("target=\"")
            .and_then(|i| span[i + 8..].split('"').next())
            .unwrap_or(&package)
            .to_string();
        Some(DevEvent::UnitDirty { target: self.target.clone(), package, version, unit, reason })
    }
}

/// Whether `line` continues an anyhow error report (`Caused by:` and its
/// indented causes) rather than being cargo's next status line. Cargo
/// right-aligns its status verb to column 12 (`   Compiling`, `    Finished`),
/// which no indented cause sentence matches.
fn is_error_chain_line(line: &str) -> bool {
    if line.trim().is_empty() || line.trim() == "Caused by:" {
        return true;
    }
    if !line.starts_with(' ') {
        return false;
    }
    let status_verb = line.len() > 12
        && line.as_bytes()[12] == b' '
        && line[..12].trim_start().chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && line[..12].trim_start().chars().all(|c| c.is_ascii_alphabetic());
    !status_verb
}

/// A fingerprint `dirty:` reason as a [`DirtyReason`].
///
/// Kinds: `file_changed` (detail: the path), `file_missing`,
/// `file_unreadable`, `env_changed` (a tracked env var; detail: its name),
/// [`DEPENDENCY_REBUILT`], `outdated` (another stale-input shape), and
/// otherwise the snake_case of cargo's variant — `features_changed`,
/// `rustflags_changed`, `profile_configuration_changed`,
/// `target_configuration_changed`, `rustc_changed`,
/// `unit_dependency_info_changed`, `env_var_changed`, … — with cargo's
/// text as the detail. A unit with no fingerprint at all is
/// [`NO_PRIOR_BUILD`], from an `err:` line rather than a `dirty:` one.
pub fn dirty_reason(reason: &str) -> DirtyReason {
    let make = |kind: &str, detail: &str| DirtyReason { kind: kind.into(), detail: truncate(detail) };
    if let Some(inner) = reason.strip_prefix("FsStatusOutdated(") {
        return if inner.starts_with("StaleItem(ChangedFile") {
            make("file_changed", &field(inner, "stale").unwrap_or_else(|| inner.to_string()))
        } else if inner.starts_with("StaleItem(MissingFile") {
            make("file_missing", &first_quoted(inner).unwrap_or_else(|| inner.to_string()))
        } else if inner.starts_with("StaleItem(ChangedEnv") {
            make("env_changed", &field(inner, "var").unwrap_or_else(|| inner.to_string()))
        } else if inner.starts_with("StaleItem(UnableToReadFile")
            || inner.starts_with("StaleItem(FailedToReadMetadata")
        {
            make("file_unreadable", &first_quoted(inner).unwrap_or_else(|| inner.to_string()))
        } else if inner.starts_with("StaleDepFingerprint") || inner.starts_with("StaleDependency") {
            make(DEPENDENCY_REBUILT, &field(inner, "name").unwrap_or_default())
        } else {
            make("outdated", inner.trim_end_matches(')'))
        };
    }
    let ident: String = reason.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if ident.is_empty() {
        return make("unknown", reason);
    }
    make(&snake_case(&ident), reason)
}

/// `name: "value"` in a Debug rendering, unquoted.
fn field(text: &str, name: &str) -> Option<String> {
    let at = text.find(&format!("{name}: \""))? + name.len() + 3;
    text[at..].split('"').next().map(str::to_string)
}

fn first_quoted(text: &str) -> Option<String> {
    text.split('"').nth(1).map(str::to_string)
}

fn snake_case(ident: &str) -> String {
    let mut out = String::with_capacity(ident.len() + 4);
    for (i, c) in ident.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn truncate(text: &str) -> String {
    if text.len() <= MAX_DETAIL {
        return text.to_string();
    }
    let mut end = MAX_DETAIL;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// `(name, version)` of a cargo package id, in either spelling:
/// `path+file:///p/foo#0.1.0` (the name is the last path segment),
/// `registry+https://…#serde@1.0.219`, or the pre-1.77
/// `serde 1.0.219 (registry+…)`.
pub fn package_name_version(id: &str) -> Option<(String, String)> {
    if let Some((url, fragment)) = id.rsplit_once('#') {
        if let Some((name, version)) = fragment.rsplit_once('@') {
            return Some((name.to_string(), version.to_string()));
        }
        let path = url.split('?').next()?;
        let name = path.trim_end_matches('/').rsplit('/').next()?;
        return Some((name.to_string(), fragment.to_string()));
    }
    let mut words = id.split_whitespace();
    Some((words.next()?.to_string(), words.next()?.to_string()))
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

    /// Every unit message is kept raw — both kinds, fresh or not, and
    /// including a package's second artifact — so a caller can record the
    /// build's exact unit set; nothing else is.
    #[test]
    fn unit_messages_keep_every_artifact_and_build_script_message() {
        let mut s = CargoStream::new("web");
        let lines = [
            r#"{"reason":"compiler-artifact","package_id":"a","filenames":["/t/deps/liba-1111111111111111.rlib"],"fresh":true}"#,
            r#"{"reason":"build-script-executed","package_id":"a","out_dir":"/t/build/a-2222222222222222/out"}"#,
            r#"{"reason":"compiler-artifact","package_id":"a","filenames":["/t/a"],"executable":"/t/a","fresh":false}"#,
        ];
        for l in lines {
            s.stdout_line(l);
        }
        s.stdout_line(r#"{"reason":"build-finished","success":true}"#);
        s.stdout_line("plain text");
        assert_eq!(s.unit_messages(), &lines.map(String::from));
    }

    /// The full-stack server's build finds the binary it restarts from
    /// cargo's messages. The bin's artifact comes after its package's lib
    /// artifact already counted the package, so the path must be taken
    /// before the "already seen" early return — the first version of this
    /// took it after and found nothing for any server with a lib.
    #[test]
    fn the_executable_is_the_last_bin_artifact_even_when_its_package_was_counted() {
        let mut s = CargoStream::new("server");
        s.stdout_line(r#"{"reason":"compiler-artifact","package_id":"srv","executable":null}"#);
        assert_eq!(s.executable(), None);
        let events = s.stdout_line(
            r#"{"reason":"compiler-artifact","package_id":"srv","executable":"/t/debug/srv"}"#,
        );
        assert!(events.is_empty(), "the package was already counted: {events:?}");
        assert_eq!(s.executable(), Some("/t/debug/srv"));
        assert_eq!(s.compiled(), 1);
    }

    /// The lines cargo 1.97 wrote for `CARGO_LOG=` [`FINGERPRINT_LOG`] after
    /// `dep/src/lib.rs` was edited under `app` (paths shortened).
    const DIRTY_LOG: &[&str] = &[
        r#"   0.004924083s  INFO prepare_target{force=false package_id=app v0.1.0 (/p/app) target="app"}: cargo::core::compiler::fingerprint: stale: changed "/p/dep/src/lib.rs""#,
        r#"   0.005371875s  INFO prepare_target{force=false package_id=app v0.1.0 (/p/app) target="app"}: cargo::core::compiler::fingerprint:           (vs) "/p/app/target/debug/.fingerprint/dep-7402d1a9dc2552a2/dep-lib-dep""#,
        r#"   0.005512250s  INFO prepare_target{force=false package_id=app v0.1.0 (/p/app) target="app"}: cargo::core::compiler::fingerprint: fingerprint dirty for app v0.1.0 (/p/app)/Build/TargetInner { name: "app", doc: true, ..: with_path("/p/app/src/main.rs", Edition2024) }"#,
        r#"   0.005537583s  INFO prepare_target{force=false package_id=app v0.1.0 (/p/app) target="app"}: cargo::core::compiler::fingerprint:     dirty: FsStatusOutdated(StaleDepFingerprint { unit: UnitIndex(1) })"#,
        r#"   0.006232041s  INFO prepare_target{force=false package_id=dep v0.1.0 (/p/dep) target="dep"}: cargo::core::compiler::fingerprint:     dirty: FsStatusOutdated(StaleItem(ChangedFile { reference: "/p/app/target/debug/.fingerprint/dep-7402d1a9dc2552a2/dep-lib-dep", reference_mtime: FileTime { seconds: 1793025734, nanos: 28256991 }, stale: "/p/dep/src/lib.rs", stale_mtime: FileTime { seconds: 1793025734, nanos: 591502387 } }))"#,
    ];

    fn dirty(events: &[DevEvent]) -> Vec<(String, String, String)> {
        events
            .iter()
            .filter_map(|e| match e {
                DevEvent::UnitDirty { package, reason, .. } => {
                    Some((package.clone(), reason.kind.clone(), reason.detail.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// The fingerprint log becomes one `unit_dirty` per unit and never
    /// reaches the terminal as output: a `--metrics` session's terminal
    /// must look like any other's.
    #[test]
    fn the_fingerprint_log_becomes_unit_dirty_events_and_no_output() {
        let mut s = CargoStream::new("web");
        let events: Vec<DevEvent> = DIRTY_LOG.iter().flat_map(|l| s.stderr_line(l)).collect();
        assert!(
            !events.iter().any(|e| matches!(e, DevEvent::Output { .. })),
            "a log line leaked as output: {events:?}"
        );
        assert_eq!(
            dirty(&events),
            vec![
                ("app".into(), DEPENDENCY_REBUILT.into(), String::new()),
                ("dep".into(), "file_changed".into(), "/p/dep/src/lib.rs".into()),
            ]
        );
        let DevEvent::UnitDirty { version, unit, .. } = &events[0] else { unreachable!() };
        assert_eq!((version.as_str(), unit.as_str()), ("0.1.0", "app"));
    }

    /// A unit with no fingerprint at its hash (a features or flags change)
    /// is logged as an `err:` with an anyhow cause chain on the lines after
    /// it. The chain is swallowed, and cargo's next status line is not.
    #[test]
    fn a_missing_fingerprint_is_no_prior_build_and_its_cause_chain_is_swallowed() {
        let mut s = CargoStream::new("web");
        let lines = [
            r#"   0.014232042s  INFO prepare_target{force=false package_id=dep v0.1.0 (/p/dep) target="dep"}: cargo::core::compiler::fingerprint: fingerprint error for dep v0.1.0 (/p/dep)/Build/TargetInner { .. }"#,
            r#"   0.014248167s  INFO prepare_target{force=false package_id=dep v0.1.0 (/p/dep) target="dep"}: cargo::core::compiler::fingerprint:     err: failed to read `/p/app/target/debug/.fingerprint/dep-1f5c564beb32be41/lib-dep`"#,
            "",
            "Caused by:",
            "    No such file or directory (os error 2)",
            "   Compiling dep v0.1.0 (/p/dep)",
        ];
        let events: Vec<DevEvent> = lines.iter().flat_map(|l| s.stderr_line(l)).collect();
        assert_eq!(dirty(&events)[0].1, NO_PRIOR_BUILD);
        let output: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                DevEvent::Output { line, .. } => Some(line.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(output, vec!["   Compiling dep v0.1.0 (/p/dep)"]);
    }

    #[test]
    fn dirty_reasons_are_classified() {
        let k = |r: &str| dirty_reason(r);
        assert_eq!(k(r#"FeaturesChanged { old: "[]", new: "[\"x\"]" }"#).kind, "features_changed");
        assert_eq!(k("RustflagsChanged { old: [], new: [\"-Cx\"] }").kind, "rustflags_changed");
        assert_eq!(k("ProfileConfigurationChanged").kind, "profile_configuration_changed");
        let env = k(r#"FsStatusOutdated(StaleItem(ChangedEnv { var: "IDEALYST_X", previous: None, current: Some("1") }))"#);
        assert_eq!((env.kind.as_str(), env.detail.as_str()), ("env_changed", "IDEALYST_X"));
        let dep = k(r#"FsStatusOutdated(StaleDependency { name: "serde", dep_mtime: FileTime { .. }, max_mtime: FileTime { .. } })"#);
        assert_eq!((dep.kind.as_str(), dep.detail.as_str()), (DEPENDENCY_REBUILT, "serde"));
        assert!(dep.is_propagated());
        assert!(!k("FeaturesChanged { .. }").is_propagated());
        assert_eq!(k(r#"FsStatusOutdated(StaleItem(MissingFile("/p/build.rs")))"#).kind, "file_missing");
        let long = "X".repeat(MAX_DETAIL * 2);
        assert!(k(&format!("FeaturesChanged {{ {long} }}")).detail.len() <= MAX_DETAIL + "…".len());
    }

    /// A non-fresh artifact is a unit cargo rebuilt, timed from its
    /// package's `Compiling` line. A fresh one is reported only as progress.
    #[test]
    fn a_recompiled_unit_is_timed_from_its_compiling_line_and_a_fresh_one_is_not_reported() {
        let mut s = CargoStream::new("web");
        let t0 = Instant::now();
        s.stderr_line_at("   Compiling dep v0.1.0 (/p/dep)", t0);
        let built = s.stdout_line_at(
            r#"{"reason":"compiler-artifact","package_id":"path+file:///p/dep#0.1.0","target":{"kind":["lib"],"name":"dep"},"fresh":false}"#,
            t0 + std::time::Duration::from_millis(1250),
        );
        assert_eq!(
            built[0],
            DevEvent::UnitBuilt {
                target: "web".into(),
                package: "dep".into(),
                version: "0.1.0".into(),
                kind: "lib".into(),
                ms: Some(1250),
            }
        );
        assert!(matches!(built[1], DevEvent::CargoProgress { compiled: 1, .. }));
        let fresh = s.stdout_line(
            r#"{"reason":"compiler-artifact","package_id":"registry+https://github.com/rust-lang/crates.io-index#serde@1.0.219","target":{"kind":["lib"]},"fresh":true}"#,
        );
        assert!(!fresh.iter().any(|e| matches!(e, DevEvent::UnitBuilt { .. })), "{fresh:?}");
    }

    #[test]
    fn package_ids_parse_in_every_spelling() {
        let p = |id: &str| package_name_version(id).unwrap();
        assert_eq!(p("path+file:///p/dep#0.1.0"), ("dep".into(), "0.1.0".into()));
        assert_eq!(p("path+file:///p/crates/x#runtime-core@4.2.0"), ("runtime-core".into(), "4.2.0".into()));
        assert_eq!(
            p("registry+https://github.com/rust-lang/crates.io-index#serde@1.0.219"),
            ("serde".into(), "1.0.219".into())
        );
        assert_eq!(p("serde 1.0.219 (registry+https://x)"), ("serde".into(), "1.0.219".into()));
    }

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
