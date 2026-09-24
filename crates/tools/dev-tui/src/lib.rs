//! Interactive panel for `idealyst dev --interactive`.
//!
//! Boots the framework's terminal backend ([`host_terminal::run`]) and
//! mounts a tiny idealyst app that shows the session's event stream. The
//! CLI's session reporter feeds a [`dev_events::Queue`]; a per-frame
//! drain on the panel side hands the events to the reactive system.
//!
//! Cross-thread shape: the dev loop emits from worker threads; the
//! framework's reactive arena is TLS-bound and single-threaded. Events
//! land in the queue, and a `raf_loop` callback on the main thread drains
//! it into the panel's signals. Mirrors the pattern `RuntimeServerShell`
//! uses to bridge wire events into the reactive tree.

use std::sync::Arc;

use dev_events::Queue;

use runtime_core::{raf_loop, signal, text, view, Element, Signal};

/// One line of log output, scoped to the source that produced it.
///
/// `PartialEq` because the panel keeps the ring buffer in a `Signal`, and
/// the world kernel's signals are equality-guarded (`T: PartialEq`) —
/// a drain that produced no change must not re-render the log view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    /// The line as the plain sink renders it.
    pub message: String,
}

/// Options for [`run`].
#[derive(Clone)]
pub struct RunOptions {
    /// The session's targets.
    pub targets: Vec<String>,
    /// Called when the user asks for a rebuild.
    pub on_rebuild: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Boot the panel. Blocks until the user quits (q / Esc / Ctrl-C).
///
/// The framework's terminal host owns stdio for the lifetime of this
/// call — raw mode + alternate screen come up before mount and tear
/// down on return. `events` is the session's event queue; the panel
/// drains it once per frame.
pub fn run(events: Queue, opts: RunOptions) -> Result<(), host_terminal::RunError> {
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
        move || build_panel(events.clone(), opts.clone()),
        host_opts,
        // dev-tui's panel is built from plain primitives — no SDK scene
        // handlers to install, so the boot seam's `register` argument is
        // a no-op. (An unregistered payload panics at realize, so this is
        // only safe because the tree is builtins-only.)
        |_| {},
    )
}

/// Construct the panel's primitive tree. Called once per mount; the
/// framework's reactive system handles re-renders via signals.
fn build_panel(bus: Queue, opts: RunOptions) -> Element {
    install_theme_once();

    // Backing store for the log view. Workers push into `bus`; the
    // raf_loop below drains and writes here. Capped to a ring of the
    // most-recent N lines so we don't grow unbounded over a long dev
    // session.
    // Created in the build closure, which the terminal boot runs inside
    // `World::enter` — signal creation outside the world panics.
    let log_lines: Signal<Vec<LogLine>> = signal(Vec::new());
    const LOG_RING_CAP: usize = 2_000;

    // Per-frame drain. Returns immediately when the queue is empty so
    // idle CPU stays near zero (host-terminal blocks on event::poll
    // between frames when no animation is pending).
    {
        let bus = bus.clone();
        let log_lines = log_lines;
        let _loop = raf_loop(move || {
            let pending: Vec<LogLine> = bus
                .drain()
                .iter()
                .filter_map(|e| dev_events::plain::render(&e.event))
                .map(|m| LogLine { message: dev_events::plain::strip_ansi(&m) })
                .collect();
            if pending.is_empty() {
                return;
            }
            // `update` takes `&T` and RETURNS the next value (it composes
            // on the staged value, so two drains in one turn never lose
            // lines).
            log_lines.update(move |cur| {
                let mut next = cur.clone();
                next.extend(pending);
                if next.len() > LOG_RING_CAP {
                    let drop_n = next.len() - LOG_RING_CAP;
                    next.drain(0..drop_n);
                }
                next
            });
        });
        // RafLoop's Drop cancels the subscription; the panel needs it
        // alive for the whole session, so leak it deliberately.
        std::mem::forget(_loop);
    }

    let header_line = "idealyst dev".to_string();
    let _ = &opts.on_rebuild;
    let target_names = if opts.targets.is_empty() {
        "(no targets)".to_string()
    } else {
        opts.targets.join("  ")
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
