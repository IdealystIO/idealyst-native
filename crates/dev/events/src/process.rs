//! Running a subprocess whose output belongs to the session.
//!
//! A build that inherits the CLI's stdio writes straight to the
//! terminal, so its output can be neither shown anywhere else nor hidden
//! (the interactive panel needs cargo's chatter off its screen).
//! Captured here instead, every line becomes an event, and the plain
//! sink prints it exactly as the subprocess would have.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use crate::cargo::CargoStream;
use crate::Reporter;
#[cfg(test)]
use crate::DevEvent;

/// Run `cmd` with both pipes captured, each line emitted as
/// [`DevEvent::Output`] from `source`. Waits for the process and for both
/// pipes to drain, so every line is emitted before this returns.
pub fn run_lines(cmd: &mut Command, reporter: &Reporter, source: &str) -> std::io::Result<ExitStatus> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let out = pump(child.stdout.take(), reporter.clone(), source.to_string());
    let err = pump(child.stderr.take(), reporter.clone(), source.to_string());
    let status = child.wait();
    let _ = out.join();
    let _ = err.join();
    status
}

fn pump(
    pipe: Option<impl Read + Send + 'static>,
    reporter: Reporter,
    source: String,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let Some(pipe) = pipe else { return };
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            reporter.output(&source, line);
        }
    })
}

/// Whether cargo should be asked for colour: `CARGO_TERM_COLOR` when it
/// says `always` or `never`, otherwise whether stderr is a terminal —
/// which is what cargo itself decided when it wrote to our stderr
/// directly.
pub fn cargo_color_wanted() -> bool {
    match std::env::var("CARGO_TERM_COLOR").ok().as_deref() {
        Some("always") => true,
        Some("never") => false,
        _ => {
            use std::io::IsTerminal;
            std::io::stderr().is_terminal()
        }
    }
}

/// What a captured cargo build reported, beyond its exit status.
#[derive(Debug, Clone, Default)]
pub struct CargoSummary {
    /// Error-level rustc diagnostics.
    pub errors: u32,
    /// Distinct packages compiled or found fresh.
    pub compiled: u32,
    /// The last executable the build produced (or found fresh) — the
    /// binary of a `--bin` build. See [`CargoStream::executable`].
    pub executable: Option<PathBuf>,
    /// The build's unit messages, raw. See [`CargoStream::unit_messages`].
    pub unit_messages: Vec<String>,
}

/// How to learn the progress bar's total for a build (see
/// [`crate::cargo`] for why it is the dependency closure).
#[derive(Debug, Clone)]
pub struct Closure {
    /// The directory holding the root package's `Cargo.toml`.
    pub manifest_dir: PathBuf,
    /// `--filter-platform`, e.g. `wasm32-unknown-unknown`.
    pub platform: Option<String>,
    /// `--features`, as passed to the build.
    pub features: Vec<String>,
}

/// Run a cargo build with its JSON message stream parsed into events:
/// progress, structured diagnostics, and every stderr line verbatim.
///
/// Appends `--message-format` and `--color` to `cmd`, so it must be a
/// `cargo build`-shaped command with no trailing `--` arguments. When
/// `closure` is given, the total is resolved from `cargo metadata` on a
/// second thread while the build runs, and arrives as a progress event
/// whenever it is ready.
///
/// The metadata call starts at cargo's FIRST message, not at spawn: by
/// then cargo has resolved the graph and written any lock-file update the
/// build needed, so the `--frozen` metadata call (which must never race
/// the build to write the lock) sees a lock it can use. Started at spawn,
/// a build that had to update its lock — a renamed package, a new
/// dependency — got no total at all.
pub fn run_cargo(
    cmd: &mut Command,
    reporter: &Reporter,
    target: &str,
    closure: Option<Closure>,
) -> std::io::Result<(ExitStatus, CargoSummary)> {
    let color = cargo_color_wanted();
    cmd.arg(if color {
        "--message-format=json-diagnostic-rendered-ansi"
    } else {
        "--message-format=json"
    })
    .arg(if color { "--color=always" } else { "--color=never" });
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stream = Arc::new(Mutex::new(CargoStream::new(target)));

    // Taken by whichever pipe sees cargo's first message.
    let pending_total = Arc::new(Mutex::new(closure));
    let start_total = {
        let stream = stream.clone();
        let reporter = reporter.clone();
        let pending_total = pending_total.clone();
        move || {
            let Some(c) = pending_total.lock().ok().and_then(|mut p| p.take()) else { return };
            let stream = stream.clone();
            let reporter = reporter.clone();
            // Not joined: metadata still resolving after the build has
            // finished has nothing left to say, and waiting on it would
            // make the build look slower than it was.
            std::thread::spawn(move || {
                let (total, gaps) = closure_facts(&c);
                let event = stream.lock().map(|mut s| s.set_total(total));
                if let Ok(event) = event {
                    reporter.emit(event);
                }
                if !gaps.is_empty() && first_warning(&c) {
                    reporter.warn("dev", lock_gap_message(&c, &gaps));
                }
            });
        }
    };

    let lines = |pipe: Option<Box<dyn Read + Send>>, stdout: bool| {
        let stream = stream.clone();
        let reporter = reporter.clone();
        let start_total = start_total.clone();
        std::thread::spawn(move || {
            let Some(pipe) = pipe else { return };
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                // Any JSON message on stdout, or a `Compiling` line: the
                // graph is resolved. (`Blocking`/`Updating` lines precede
                // resolution and do not count.)
                if stdout || crate::plain::strip_ansi(&line).trim_start().starts_with("Compiling ") {
                    start_total();
                }
                // The lock covers parse AND emit, so progress events leave
                // in the order the parser produced them.
                let Ok(mut s) = stream.lock() else { return };
                let events =
                    if stdout { s.stdout_line(&line) } else { s.stderr_line(&line) };
                for e in events {
                    reporter.emit(e);
                }
            }
        })
    };
    let out = lines(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), true);
    let err = lines(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>), false);
    let status = child.wait()?;
    let _ = out.join();
    let _ = err.join();
    let summary = stream
        .lock()
        .map(|s| CargoSummary {
            errors: s.errors(),
            compiled: s.compiled(),
            executable: s.executable().map(PathBuf::from),
            unit_messages: s.unit_messages().to_vec(),
        })
        .unwrap_or_default();
    Ok((status, summary))
}

type ClosureKey = (PathBuf, Option<String>, Vec<String>);
type Stamp = (Option<SystemTime>, Option<SystemTime>);
type Facts = (Option<u32>, Vec<String>);

/// The closure size for `c`, cached per manifest until `Cargo.toml` or
/// `Cargo.lock` changes: `cargo metadata` costs hundreds of
/// milliseconds on a large graph, and a dev session asks on every build.
pub fn closure_total(c: &Closure) -> Option<u32> {
    closure_facts(c).0
}

/// [`closure_total`], plus the packages of the closure that the lock file
/// does not record ([`lock_gaps`]) — from the same `cargo metadata` call.
pub fn closure_facts(c: &Closure) -> Facts {
    static CACHE: OnceLock<Mutex<HashMap<ClosureKey, (Stamp, Facts)>>> = OnceLock::new();
    let key = (c.manifest_dir.clone(), c.platform.clone(), c.features.clone());
    let stamp = (mtime(&c.manifest_dir.join("Cargo.toml")), lock_mtime(&c.manifest_dir));
    let cache = CACHE.get_or_init(Default::default);
    if let Some((s, facts)) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        if s == stamp {
            return facts;
        }
    }
    let facts = match metadata(c) {
        Some(m) => {
            let lock = lock_path(&c.manifest_dir).and_then(|p| std::fs::read_to_string(p).ok());
            let gaps = lock.map(|l| lock_gaps(&m, &l)).unwrap_or_default();
            (crate::cargo::closure_size(&m), gaps)
        }
        None => (None, Vec::new()),
    };
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, (stamp, facts.clone()));
    }
    facts
}

/// Packages `metadata`'s resolve holds that `lock` (a `Cargo.lock`'s
/// text) does not, as `name version`, sorted.
///
/// Why: cargo writes the lock from a resolve with every feature of the
/// WORKSPACE MEMBERS on, and a feature of a dependency only if a member
/// turns it on. A dependency feature named on the command line — the
/// `--features idealyst/hot-reload,runtime-core/dev` that `idealyst dev`
/// passes — is resolved for the build but never written, so every such
/// build re-resolves its packages from the registry: CrewForge paid
/// `Updating crates.io index`, about 1.7 s and a network round trip, on
/// every rebuild, and versions that float.
pub fn lock_gaps(metadata: &serde_json::Value, lock: &str) -> Vec<String> {
    let locked: std::collections::HashSet<(String, String)> = locked_packages(lock).into_iter().collect();
    let resolved: std::collections::HashSet<&str> = metadata["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| n["id"].as_str())
        .collect();
    let mut gaps: Vec<String> = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["id"].as_str().is_some_and(|id| resolved.contains(id)))
        .filter_map(|p| Some((p["name"].as_str()?.to_string(), p["version"].as_str()?.to_string())))
        .filter(|nv| !locked.contains(nv))
        .map(|(n, v)| format!("{n} {v}"))
        .collect();
    gaps.sort();
    gaps.dedup();
    gaps
}

/// `(name, version)` of every `[[package]]` in a `Cargo.lock`. A line
/// scan: the format is cargo's own and fixed (`name = "…"` then
/// `version = "…"` in each entry).
fn locked_packages(lock: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut name: Option<String> = None;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            name = None;
        } else if let Some(v) = quoted(line, "name") {
            name = Some(v);
        } else if let (Some(v), Some(n)) = (quoted(line, "version"), name.take()) {
            out.push((n, v));
        }
    }
    out
}

fn quoted(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?.trim();
    Some(rest.strip_prefix('"')?.strip_suffix('"')?.to_string())
}

/// The warning for [`lock_gaps`]: what the lock misses, and the one
/// `[features]` entry that makes the lock cover it (a member feature
/// nothing enables still counts, so the lock records its packages).
pub fn lock_gap_message(c: &Closure, gaps: &[String]) -> String {
    let deps: Vec<String> = c.features.iter().filter(|f| f.contains('/')).map(|f| format!("\"{f}\"")).collect();
    let shown: Vec<&str> = gaps.iter().take(6).map(String::as_str).collect();
    let more = gaps.len().saturating_sub(shown.len());
    format!(
        "Cargo.lock does not record {} package(s) this build's features pull in ({}{}), so cargo \
         resolves them from the registry on every build. Declare the features once in {} so the \
         lock covers them (nothing needs to enable it):\n  [features]\n  dev-session = [{}]",
        gaps.len(),
        shown.join(", "),
        if more > 0 { format!(", and {more} more") } else { String::new() },
        c.manifest_dir.join("Cargo.toml").display(),
        deps.join(", "),
    )
}

/// Whether this is the first lock-gap warning for `c` in this process.
fn first_warning(c: &Closure) -> bool {
    static WARNED: OnceLock<Mutex<std::collections::HashSet<ClosureKey>>> = OnceLock::new();
    let key = (c.manifest_dir.clone(), c.platform.clone(), c.features.clone());
    WARNED.get_or_init(Default::default).lock().map(|mut w| w.insert(key)).unwrap_or(false)
}

fn metadata(c: &Closure) -> Option<serde_json::Value> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&c.manifest_dir)
        .args(["metadata", "--format-version", "1"])
        // Never touch the lock file or the network: this runs beside a
        // build that may be resolving too, and a total is not worth
        // either. A graph that needs resolving yields no total.
        .arg("--frozen");
    if let Some(p) = &c.platform {
        cmd.args(["--filter-platform", p]);
    }
    if !c.features.is_empty() {
        cmd.arg("--features").arg(c.features.join(","));
    }
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The lock file cargo would use: this package's, or the nearest
/// enclosing workspace's.
fn lock_mtime(dir: &Path) -> Option<SystemTime> {
    lock_path(dir).and_then(|p| mtime(&p))
}

fn lock_path(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().map(|d| d.join("Cargo.lock")).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Queue;

    /// Regression (CrewForge): `--features idealyst/hot-reload` pulled in
    /// packages the lock never records, re-resolved from the registry on
    /// every build. The real thing, against cargo: a dependency feature
    /// given on the command line resolves a package `Cargo.lock` lacks,
    /// and an app feature that names it (enabled by nothing) closes it.
    #[test]
    fn regression_a_command_line_dependency_feature_is_missing_from_the_lock() {
        let tmp = std::env::temp_dir().join(format!("dev-events-lockgap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let w = |p: &str, t: &str| {
            let p = tmp.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        };
        w("ext/opt/Cargo.toml", "[package]\nname = \"opt\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        w("ext/opt/src/lib.rs", "");
        w(
            "ext/dep/Cargo.toml",
            "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
             [dependencies]\nopt = { path = \"../opt\", optional = true }\n[features]\nx = [\"dep:opt\"]\n",
        );
        w("ext/dep/src/lib.rs", "");
        let app = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
                   [dependencies]\ndep = { path = \"../ext/dep\" }\n";
        w("app/Cargo.toml", app);
        w("app/src/lib.rs", "");
        let dir = tmp.join("app");
        let lock = || {
            let ok = Command::new("cargo").args(["generate-lockfile", "--offline"]).current_dir(&dir).status().unwrap();
            assert!(ok.success());
        };
        lock();
        let c = Closure { manifest_dir: dir.clone(), platform: None, features: vec!["dep/x".into()] };
        let gaps = lock_gaps(&metadata(&c).expect("metadata"), &std::fs::read_to_string(dir.join("Cargo.lock")).unwrap());
        assert_eq!(gaps, vec!["opt 0.1.0"]);
        let msg = lock_gap_message(&c, &gaps);
        assert!(msg.contains("dev-session = [\"dep/x\"]"), "{msg}");

        // The suggested entry, declared: the lock now covers it.
        w("app/Cargo.toml", &format!("{app}[features]\ndev-session = [\"dep/x\"]\n"));
        lock();
        let gaps = lock_gaps(&metadata(&c).expect("metadata"), &std::fs::read_to_string(dir.join("Cargo.lock")).unwrap());
        assert!(gaps.is_empty(), "{gaps:?}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_lock_scan_reads_every_package_entry() {
        let lock = "version = 4\n\n[[package]]\nname = \"a\"\nversion = \"1.0.0\"\nsource = \"registry+x\"\n\n[[package]]\nname = \"b\"\nversion = \"0.2.1\"\ndependencies = [\n \"a\",\n]\n";
        assert_eq!(locked_packages(lock), vec![("a".into(), "1.0.0".into()), ("b".into(), "0.2.1".into())]);
    }

    #[test]
    fn run_lines_emits_every_line_of_both_pipes_before_returning() {
        let r = Reporter::new();
        let q = Queue::new();
        r.add_sink(Arc::new(q.clone()));
        let status = run_lines(
            Command::new("sh").args(["-c", "echo one; echo two 1>&2; echo three"]),
            &r,
            "sh",
        )
        .unwrap();
        assert!(status.success());
        let mut lines: Vec<String> = q
            .drain()
            .into_iter()
            .map(|e| match e.event {
                DevEvent::Output { source, line, .. } => {
                    assert_eq!(source, "sh");
                    line
                }
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        lines.sort();
        assert_eq!(lines, vec!["one", "three", "two"]);
    }

    #[test]
    fn run_cargo_turns_both_pipes_into_progress_diagnostics_and_output() {
        // A stand-in for cargo: the appended `--message-format`/`--color`
        // arguments land in `$@` and are ignored.
        let script = r#"
            echo '   Compiling app v0.1.0 (/w)' 1>&2
            echo '{"reason":"compiler-artifact","package_id":"dep"}'
            printf '%s\n' '{"reason":"compiler-message","package_id":"app","message":{"level":"error","message":"boom","rendered":"error: boom\n","spans":[],"code":null}}'
            echo 'error: could not compile `app`' 1>&2
            exit 101
        "#;
        let r = Reporter::new();
        let q = Queue::new();
        r.add_sink(Arc::new(q.clone()));
        let (status, summary) =
            run_cargo(Command::new("sh").args(["-c", script, "sh"]), &r, "web", None).unwrap();
        assert_eq!(status.code(), Some(101));
        assert_eq!(summary.errors, 1);
        assert_eq!(summary.compiled, 1);
        let events: Vec<DevEvent> = q.drain().into_iter().map(|e| e.event).collect();
        assert!(events.iter().any(|e| matches!(e,
            DevEvent::Diagnostic { diagnostic, .. } if diagnostic.message == "boom")));
        assert!(events.iter().any(|e| matches!(e,
            DevEvent::CargoProgress { compiled: 1, .. })));
        assert!(events.iter().any(|e| matches!(e,
            DevEvent::Output { line, .. } if line == "error: could not compile `app`")));
    }

    /// Regression: the closure lookup started when cargo was spawned, so
    /// a build that had to write its lock file first (a renamed package, a
    /// new dependency) raced it, `cargo metadata --frozen` refused the
    /// stale lock, and the build ran with no total — the E2E caught a
    /// renamed copy of the lab reporting progress with no bar. The lookup
    /// now waits for cargo's first message.
    #[test]
    fn regression_the_total_survives_a_build_that_writes_its_lock() {
        let dir = std::env::temp_dir().join(format!("dev-events-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"lockless\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), "").unwrap();
        // A stand-in for cargo that resolves (writes the lock) a moment
        // after it starts, then reports one compiled package.
        let script = r#"
            sleep 1
            cargo generate-lockfile --offline -q
            echo '   Compiling lockless v0.1.0' 1>&2
            echo '{"reason":"compiler-artifact","package_id":"lockless"}'
            sleep 3
        "#;
        let r = Reporter::new();
        let q = crate::Queue::new();
        r.add_sink(Arc::new(q.clone()));
        let closure = Closure { manifest_dir: dir.clone(), platform: None, features: vec![] };
        run_cargo(
            Command::new("sh").current_dir(&dir).args(["-c", script, "sh"]),
            &r,
            "web",
            Some(closure),
        )
        .unwrap();
        let totals: Vec<Option<u32>> = q
            .drain()
            .into_iter()
            .filter_map(|e| match e.event {
                DevEvent::CargoProgress { total, .. } => Some(total),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(totals.contains(&Some(1)), "the total never arrived: {totals:?}");
    }
}
