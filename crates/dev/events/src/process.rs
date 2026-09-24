//! Running a subprocess whose output belongs to the session.
//!
//! The dev loop's builds used to inherit the CLI's stdio, so cargo wrote
//! straight to the terminal. That made its output impossible to show
//! anywhere else, and impossible to hide: the interactive panel had to
//! redirect the CLI's own stderr to a file to keep cargo's chatter off
//! its screen. Captured here instead, every line becomes an event, and
//! the plain sink prints it exactly as the subprocess would have.

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
                let total = closure_total(&c);
                let event = stream.lock().map(|mut s| s.set_total(total));
                if let Ok(event) = event {
                    reporter.emit(event);
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
        })
        .unwrap_or_default();
    Ok((status, summary))
}

type ClosureKey = (PathBuf, Option<String>, Vec<String>);
type Stamp = (Option<SystemTime>, Option<SystemTime>);

/// The closure size for `c`, cached per manifest until `Cargo.toml` or
/// `Cargo.lock` changes: `cargo metadata` costs hundreds of
/// milliseconds on a large graph, and a dev session asks on every build.
pub fn closure_total(c: &Closure) -> Option<u32> {
    static CACHE: OnceLock<Mutex<HashMap<ClosureKey, (Stamp, Option<u32>)>>> = OnceLock::new();
    let key = (c.manifest_dir.clone(), c.platform.clone(), c.features.clone());
    let stamp = (mtime(&c.manifest_dir.join("Cargo.toml")), lock_mtime(&c.manifest_dir));
    let cache = CACHE.get_or_init(Default::default);
    if let Some((s, total)) = cache.lock().ok()?.get(&key) {
        if *s == stamp {
            return *total;
        }
    }
    let total = metadata(c).and_then(|m| crate::cargo::closure_size(&m));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, (stamp, total));
    }
    total
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
    dir.ancestors().find_map(|d| mtime(&d.join("Cargo.lock")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Queue;

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
