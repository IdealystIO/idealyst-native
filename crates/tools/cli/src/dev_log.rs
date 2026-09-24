//! The `idealyst dev` session's reporting: where its events go.
//!
//! Every line the dev loop prints is a [`dev_events::DevEvent`] emitted
//! through one session [`Reporter`], installed process-wide
//! ([`dev_events::install_global`]) so code with no handle threaded to it
//! reaches the same sinks. [`start`] decides the sinks:
//!
//! - the terminal: plain lines on stderr ([`Ui::Plain`], the default and
//!   the only choice when stderr is not a TTY), or the interactive panel
//!   ([`Ui::Panel`], `--interactive` / `IDEALYST_DEV_UI=panel`), which
//!   drains a [`dev_events::Queue`] on its own frame clock;
//! - the session log, `target/idealyst/<app>/dev.log`, in every mode:
//!   every event with its session time, including the facts a plain
//!   terminal never showed (page acks, stage boundaries) — so nothing is
//!   lost when the panel hides verbose output;
//! - JSON lines, when asked (`--events json` to stdout, `--events-file
//!   <path>`): the hook for editors and tools.
//!
//! [`dlog`] is the CLI's own log call, kept as a thin emitter so the
//! launchers' many `dlog!` call sites read as they always did.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use dev_events::{DevEvent, JsonLines, PlainLines, Queue, Reporter, Sink, Verbosity};

/// How the terminal shows the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ui {
    /// Plain lines on stderr: what `idealyst dev` has always printed.
    Plain,
    /// The interactive panel (`dev-tui`).
    Panel,
}

/// The env override for the terminal UI: `panel` or `plain`.
pub const UI_ENV: &str = "IDEALYST_DEV_UI";

/// Decide the terminal UI. The flag and the env var ask; a panel still
/// needs stderr to be a TTY (piped runs, CI and the MCP dev-runner keep
/// plain lines) and must not share the terminal with a `--terminal` app.
/// The env var wins over the flag in both directions. Returns why a
/// requested panel was refused, for the one line that says so.
pub fn resolve_ui(
    flag: bool,
    env: Option<&str>,
    stderr_is_tty: bool,
    terminal_target: bool,
) -> (Ui, Option<&'static str>) {
    let asked = match env.map(str::trim) {
        Some("panel") => true,
        Some("plain") => false,
        _ => flag,
    };
    if !asked {
        return (Ui::Plain, None);
    }
    if !stderr_is_tty {
        return (Ui::Plain, Some("stderr is not a TTY"));
    }
    if terminal_target {
        return (Ui::Plain, Some("the --terminal target needs the terminal"));
    }
    (Ui::Panel, None)
}

/// A running session's reporting.
pub struct Session {
    pub reporter: Reporter,
    /// What the panel drains, in [`Ui::Panel`].
    pub panel: Option<Queue>,
    /// Every sink but the terminal's, to keep when the panel closes.
    others: Vec<Arc<dyn Sink>>,
}

impl Session {
    /// The panel has given the terminal back: plain lines on stderr from
    /// here on (the teardown lines, and anything a still-running thread
    /// says on the way out), the other sinks unchanged.
    pub fn panel_closed(&self) {
        let mut sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(PlainLines::stderr())];
        sinks.extend(self.others.iter().cloned());
        self.reporter.set_sinks(sinks);
    }
}

/// Where JSON events go.
#[derive(Debug, Clone, Default)]
pub struct EventsOut {
    /// `--events json`: JSON lines on stdout.
    pub stdout: bool,
    /// `--events-file <path>`.
    pub file: Option<PathBuf>,
}

/// Build the session reporter with its sinks and install it
/// process-wide. `log_path` is created (truncated) for this session.
pub fn start(ui: Ui, log_path: &Path, events: &EventsOut) -> anyhow::Result<Session> {
    let reporter = Reporter::new();
    let panel = match ui {
        Ui::Plain => {
            reporter.add_sink(Arc::new(PlainLines::stderr()));
            None
        }
        Ui::Panel => {
            let q = Queue::new();
            reporter.add_sink(Arc::new(q.clone()));
            Some(q)
        }
    };
    let mut others: Vec<Arc<dyn Sink>> = Vec::new();
    match open_log(log_path) {
        Ok(file) => others.push(Arc::new(PlainLines::new(file, Verbosity::Verbose))),
        // The log is a convenience; a session that cannot write it still
        // runs, and says so.
        Err(e) => reporter.warn(
            "dev",
            format!("cannot write the session log {}: {e}", log_path.display()),
        ),
    }
    if events.stdout {
        anyhow::ensure!(
            ui == Ui::Plain,
            "--events json writes to stdout, which the interactive panel paints on; \
             use --events-file <path> with the panel"
        );
        others.push(Arc::new(JsonLines::stdout()));
    }
    if let Some(path) = &events.file {
        let sink = JsonLines::file(path).map_err(|e| {
            anyhow::anyhow!("cannot write the events file {}: {e}", path.display())
        })?;
        others.push(Arc::new(sink));
    }
    // The session's event stream for HTTP subscribers (`/__idealyst/events`
    // on every dev server, and on its own port — see `serve_events`).
    let broadcast = dev_events::broadcast::Broadcast::new();
    others.push(broadcast.clone());
    let _ = BROADCAST.set(broadcast);
    for sink in &others {
        reporter.add_sink(sink.clone());
    }
    // First wins: a process runs one dev session.
    dev_events::install_global(reporter.clone());
    Ok(Session { reporter, panel, others })
}

static BROADCAST: OnceLock<Arc<dev_events::broadcast::Broadcast>> = OnceLock::new();

/// The session's event stream, once [`start`] has run.
pub fn session_events() -> Option<Arc<dev_events::broadcast::Broadcast>> {
    BROADCAST.get().cloned()
}

/// Run the stand-alone events endpoint on an ephemeral loopback port and
/// write its URL to `<project>/.idealyst/events.url`, so a tool finds a
/// session — even one with no web target — with nothing but the project
/// directory. Returns the URL.
pub fn serve_events(project_dir: &Path) -> Option<String> {
    let events = session_events()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    drop(listener);
    let url = format!("http://127.0.0.1:{port}{}", dev_http::EVENTS_URL);
    std::thread::Builder::new()
        .name("dev-events-http".into())
        .spawn(move || {
            if let Err(e) = dev_http::serve_events("127.0.0.1", port, events) {
                dev_events::global().warn("dev", format!("event stream stopped: {e:#}"));
            }
        })
        .ok()?;
    let file = project_dir.join(".idealyst").join("events.url");
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&file, format!("{url}\n")) {
        dlog("dev", format!("could not write {}: {e}", file.display()));
    }
    emit(DevEvent::ServerReady {
        target: "session".into(),
        kind: dev_events::ServerKind::Events,
        url: url.clone(),
    });
    Some(url)
}

/// Wire a page-facing reload signal into the session: pages get the
/// build state (`dev-state`) and the event stream (`/__idealyst/events`)
/// through it, their acks come back as events, and the panel's rebuild
/// key reaches its watcher.
pub fn attach_signal(signal: &Arc<dev_reload::ReloadSignal>) {
    let reporter = dev_events::global();
    reporter.subscribe(Box::new(dev_reload::PageSink::new(signal.clone())));
    signal.report_acks_to(reporter);
    if let Some(events) = session_events() {
        signal.serve_events(events);
    }
    register_rebuild(signal.clone());
}

/// The session log, opened for append so the fd-2 fallback (see
/// `cmd::dev`'s `StderrToLog`) and this sink interleave whole lines
/// instead of overwriting each other.
pub fn open_log(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Truncate once for the new session, then append.
    std::fs::File::create(path)?;
    std::fs::OpenOptions::new().append(true).open(path)
}

/// Whether stderr is a terminal.
pub fn stderr_is_tty() -> bool {
    std::io::stderr().is_terminal()
}

/// Emit a CLI log line, `[tag] message` in plain output.
pub fn dlog(tag: &str, message: impl AsRef<str>) {
    dev_events::global().log(tag, message.as_ref());
}

/// Emit an event on the session reporter.
pub fn emit(event: DevEvent) {
    dev_events::global().emit(event);
}

/// The session's rebuild triggers — one per watcher that can rebuild on
/// request. The panel's `r` fires them all.
fn rebuilders() -> &'static Mutex<Vec<Arc<dev_reload::ReloadSignal>>> {
    static R: OnceLock<Mutex<Vec<Arc<dev_reload::ReloadSignal>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Register a watcher's signal as a rebuild target.
pub fn register_rebuild(signal: Arc<dev_reload::ReloadSignal>) {
    if let Ok(mut r) = rebuilders().lock() {
        r.push(signal);
    }
}

/// Ask every registered watcher to rebuild. Returns how many took the
/// request.
pub fn request_rebuild() -> usize {
    let n = rebuilders()
        .lock()
        .map(|r| r.iter().filter(|s| s.request_rebuild()).count())
        .unwrap_or(0);
    if n == 0 {
        dlog("dev", "rebuild requested, but no watcher is running (a --no-build session?)");
    }
    n
}

/// `dlog!("tag", "fmt {}", arg)` — shorthand for the common
/// `format!`-then-`dlog` pattern.
#[macro_export]
macro_rules! dlog {
    ($tag:expr, $($arg:tt)*) => {
        $crate::dev_log::dlog($tag, format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_is_the_default_and_the_panel_needs_a_tty() {
        assert_eq!(resolve_ui(false, None, true, false), (Ui::Plain, None));
        assert_eq!(resolve_ui(true, None, true, false), (Ui::Panel, None));
        assert_eq!(resolve_ui(true, None, false, false), (Ui::Plain, Some("stderr is not a TTY")));
        assert_eq!(
            resolve_ui(true, None, true, true),
            (Ui::Plain, Some("the --terminal target needs the terminal"))
        );
    }

    #[test]
    fn the_env_override_wins_over_the_flag_both_ways() {
        assert_eq!(resolve_ui(false, Some("panel"), true, false).0, Ui::Panel);
        assert_eq!(resolve_ui(true, Some("plain"), true, false).0, Ui::Plain);
        // Anything else leaves the flag in charge.
        assert_eq!(resolve_ui(true, Some("fancy"), true, false).0, Ui::Panel);
        // And the env var cannot force a panel onto a pipe.
        assert_eq!(resolve_ui(false, Some("panel"), false, false).0, Ui::Plain);
    }

    #[test]
    fn json_on_stdout_is_refused_under_the_panel() {
        let dir = tempfile::tempdir().unwrap();
        let events = EventsOut { stdout: true, file: None };
        let err = start(Ui::Panel, &dir.path().join("dev.log"), &events).err().unwrap();
        assert!(err.to_string().contains("--events-file"), "{err}");
    }

    #[test]
    fn the_session_log_and_the_events_file_both_receive_every_event() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("t/idealyst/app/dev.log");
        let json = dir.path().join("events.jsonl");
        let s = start(Ui::Panel, &log, &EventsOut { stdout: false, file: Some(json.clone()) })
            .unwrap();
        s.reporter.log("dev web", "hello");
        s.reporter.emit(DevEvent::PageAck {
            target: "web".into(),
            ack: dev_events::PageAck::Connected { gen: 1 },
        });
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("[dev web] hello"), "{text}");
        assert!(text.contains("[web page] connected (gen 1)"), "{text}");
        assert_eq!(std::fs::read_to_string(&json).unwrap().lines().count(), 2);
        assert_eq!(s.panel.unwrap().len(), 2, "the panel sees them too");
    }
}
