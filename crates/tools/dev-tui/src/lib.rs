//! Interactive panel for `idealyst dev --interactive`.
//!
//! Boots the framework's terminal backend ([`host_terminal::run`]) and
//! mounts a tiny idealyst app that surfaces per-target build/run state
//! plus the live `[dev …]` log stream the CLI is otherwise printing to
//! stderr. Worker threads in `cmd/dev.rs` push events through a
//! [`DevBus`]; a per-frame drain on the panel side hands them to the
//! reactive system.
//!
//! Scope of this scaffold:
//!   - Render a header strip, a static target list, and a scrolling
//!     log view fed by [`DevBus::log`].
//!   - Quit on `q` / `Esc` / `Ctrl-C` (handled by host-terminal).
//!
//! Out of scope until follow-up:
//!   - Reactive per-target state machine (queued / building / running).
//!   - Rebuild keybindings, log filter cycling, expand-error overlay.
//!
//! Cross-thread shape: workers run off-main; the framework's reactive
//! arena is TLS-bound and single-threaded. Workers push into a
//! `Mutex<Vec<…>>` inside [`DevBus`], and a `raf_loop` callback on the
//! main thread drains it into the panel's `Signal<Vec<…>>`. Mirrors
//! the pattern `RuntimeServerShell` uses to bridge wire events into
//! the reactive tree.

use std::sync::{Arc, Mutex};

use runtime_core::{raf_loop, text, view, Primitive, Signal};

/// One line of log output, scoped to the target that produced it.
#[derive(Clone, Debug)]
pub struct LogLine {
    /// Target tag — `"dev"`, `"dev web"`, `"host"`, etc. Mirrors the
    /// existing `[dev …]` prefix the CLI prints today so the user sees
    /// the same shape they're used to.
    pub tag: String,
    /// The line itself, sans trailing newline.
    pub message: String,
}

/// Targets the panel knows how to display. The CLI passes these in at
/// startup so the target list reflects what's actually running this
/// session.
#[derive(Clone, Debug)]
pub struct TargetInfo {
    /// Human-readable name — `"web"`, `"ios"`, `"android"`, etc.
    pub name: String,
}

/// Cross-thread event bus shared between the CLI's worker threads and
/// the panel app running on the main thread. Push side is `Send`;
/// drain side is called only from the panel's `raf_loop`.
#[derive(Clone, Default)]
pub struct DevBus {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Worker threads append here; the panel drains on each frame.
    log_queue: Mutex<Vec<LogLine>>,
}

impl DevBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a log line. Safe to call from any thread.
    pub fn log(&self, tag: impl Into<String>, message: impl Into<String>) {
        let line = LogLine {
            tag: tag.into(),
            message: message.into(),
        };
        if let Ok(mut q) = self.inner.log_queue.lock() {
            q.push(line);
        }
    }

    fn drain(&self) -> Vec<LogLine> {
        match self.inner.log_queue.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => Vec::new(),
        }
    }
}

/// Options for [`run`].
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Project name shown in the header bar.
    pub project_name: String,
    /// Active targets to surface in the target list.
    pub targets: Vec<TargetInfo>,
    /// `true` when `--runtime-server` is on; the header reflects it.
    pub runtime_server: bool,
}

/// Boot the panel. Blocks until the user quits (q / Esc / Ctrl-C).
///
/// The framework's terminal host owns stdio for the lifetime of this
/// call — raw mode + alternate screen come up before mount and tear
/// down on return. The `DevBus` is the only conduit for log/state
/// data; CLI workers must publish through it (direct `eprintln!`
/// during this window would corrupt the cell grid; the host installs
/// an `StderrRedirect` to `.idealyst/terminal.log` so stray prints
/// land in a file instead).
pub fn run(bus: DevBus, opts: RunOptions) -> Result<(), host_terminal::RunError> {
    // Capture for the app closure. `host_terminal::run` requires
    // `Fn() -> Primitive + 'static`; the bus + opts get cloned in
    // once and the closure is re-invokable.
    let bus_for_app = bus.clone();
    let opts_for_app = opts.clone();

    let host_opts = host_terminal::RunOptions {
        // ASCII redraw is cheap and the log stream's perceived
        // smoothness is what matters here. 30 fps matches the
        // host-terminal default.
        target_fps: 30,
        on_key: None,
        // 1 cell = 1 layout px keeps text + framing predictable for a
        // panel authored at terminal scale.
        cell_size: None,
    };

    host_terminal::run(
        move || build_panel(bus_for_app.clone(), opts_for_app.clone()),
        host_opts,
    )
}

/// Construct the panel's primitive tree. Called once per mount; the
/// framework's reactive system handles re-renders via signals.
fn build_panel(bus: DevBus, opts: RunOptions) -> Primitive {
    install_theme_once();

    // Backing store for the log view. Workers push into `bus`; the
    // raf_loop below drains and writes here. Capped to a ring of the
    // most-recent N lines so we don't grow unbounded over a long dev
    // session.
    let log_lines: Signal<Vec<LogLine>> = Signal::new(Vec::new());
    const LOG_RING_CAP: usize = 2_000;

    // Per-frame drain. Returns immediately when the queue is empty so
    // idle CPU stays near zero (host-terminal blocks on event::poll
    // between frames when no animation is pending).
    {
        let bus = bus.clone();
        let log_lines = log_lines;
        let _loop = raf_loop(move || {
            let pending = bus.drain();
            if pending.is_empty() {
                return;
            }
            log_lines.update(|cur| {
                cur.extend(pending);
                if cur.len() > LOG_RING_CAP {
                    let drop_n = cur.len() - LOG_RING_CAP;
                    cur.drain(0..drop_n);
                }
            });
        });
        // RafLoop's Drop cancels the subscription; the panel needs it
        // alive for the whole session, so leak it deliberately.
        std::mem::forget(_loop);
    }

    let header_line = format!(
        "idealyst dev · {} · {}",
        opts.project_name,
        if opts.runtime_server { "runtime-server" } else { "local" },
    );
    let target_names = if opts.targets.is_empty() {
        "(no targets)".to_string()
    } else {
        opts.targets
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join("  ")
    };

    let footer_line = "q quit · ↑/↓ scroll · ? help";

    view(vec![
        text(header_line).into(),
        text(format!("targets: {}", target_names)).into(),
        text("─".repeat(60)).into(),
        // Log view. Reactive — re-renders when `log_lines` changes.
        view(vec![text(move || render_log(&log_lines.get())).into()]).into(),
        text("─".repeat(60)).into(),
        text(footer_line).into(),
    ])
    .into()
}

/// Flatten the ring buffer to a single string. v1 just joins by
/// newline; the host-terminal renderer breaks text on `\n` into
/// separate cells. Follow-up will replace this with a scrollable
/// view-per-line so we can apply per-target colors.
fn render_log(lines: &[LogLine]) -> String {
    let mut out = String::new();
    for line in lines {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push('[');
        out.push_str(&line.tag);
        out.push_str("] ");
        out.push_str(&line.message);
    }
    out
}

/// Idempotent theme install. The framework panics on first render
/// without a theme installed (see [[project_install_theme_required]]).
/// Multiple `install_theme` calls are safe — later calls replace the
/// active theme, which is fine because this scaffold doesn't drive
/// any theme tokens itself.
fn install_theme_once() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        idea_ui::install_idea_theme(idea_ui::light_theme());
    });
}
