//! Terminal host shell for `backend-terminal`.
//!
//! [`run`] boots crossterm (raw mode + alternate screen + mouse
//! capture), mounts the user's scene through
//! `backend_terminal::newcore::start`, and runs a render loop that:
//!   1. Drains terminal events (resize, keys, mouse) and dispatches.
//!   2. Asks the backend to lay out + compose a fresh
//!      [`backend_terminal::Grid`].
//!   3. Diffs the grid against the previous frame and emits the
//!      minimal ANSI escape stream to stdout.
//!   4. Sleeps until the next frame tick.
//!
//! Quits cleanly on `q`, `Esc`, or `Ctrl-C`, restoring the
//! terminal's prior state.
//!
//! The boot entries ([`run`], [`render_headless`]) live in `boot.rs`
//! and are re-exported at the crate root; [`newcore`] re-exports them
//! again under their historical path.

use std::io::{self, Write};
use std::rc::Rc;

mod boot;
#[cfg(feature = "runtime-server")]
mod runtime_server;
mod scheduler;
mod stderr_redirect;

pub use boot::{render_headless, run};
#[cfg(feature = "runtime-server")]
pub use runtime_server::run_runtime_server;

/// Compatibility path. The boot entries used to live behind a
/// `newcore` module while the framework carried two cores; every
/// caller and doc spells them `host_terminal::newcore::run` /
/// `::render_headless`. There is one core now and the entries live at
/// the crate root ([`crate::run`], [`crate::render_headless`]) — this
/// re-export keeps the historical paths resolving so callers don't
/// churn.
pub mod newcore {
    pub use crate::{render_headless, run};
}

/// Install the terminal scheduler on this thread without spinning up
/// a full crossterm-backed host. Test-only — calling `run(...)`
/// installs it automatically.
pub fn install_scheduler_for_testing() {
    scheduler::install();
}

/// Pump expired timers + raf subscribers once. Test-only companion
/// to [`install_scheduler_for_testing`]; the full `run(...)` driver
/// ticks the scheduler on every frame internally.
pub fn tick_scheduler_for_testing() {
    scheduler::tick();
}

use backend_terminal::{Grid, TerminalKey};
use crossterm::{
    cursor, queue,
    style::{Color as CtColor, SetBackgroundColor, SetForegroundColor},
};
pub use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use runtime_shared::color::Rgba;

/// Where stderr lands while the terminal session is alive. Lives
/// under the cwd's `.idealyst/` so it's easy to `tail -f` from
/// another terminal and gets ignored by the framework's `.gitignore`
/// alongside the bridge port file. Falls back to `terminal.log` in
/// cwd if `.idealyst/` can't be created.
fn default_log_path() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    cwd.join(".idealyst").join("terminal.log")
}

/// Install a panic hook so panic info lands in the log alongside
/// anything `eprintln!` writes. Without this, a runtime panic races
/// with the raw-mode teardown — the alternate-screen exit executes
/// mid-message and the terminal-log ends up with no diagnostic,
/// leaving only the host's "exited with status 101" line in the build
/// log. Shared by [`run`] and the runtime-server boot.
///
/// Defensive shape: the original panic message is written FIRST and on
/// its own try (a) so the user always sees what actually failed, even
/// if backtrace capture later panics. Backtrace capture is wrapped in
/// `catch_unwind` because `force_capture` touches TLS, and during
/// teardown the reactive-arena TLS may already be destroyed — a panic
/// in the panic hook becomes a fatal runtime abort that swallows the
/// real message (saw this when the dev-tui shutdown raced with effect
/// cleanup).
fn install_panic_log_hook(log_path: std::path::PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        // (a) Write the panic info on its own. No TLS access here
        //     beyond what `info`'s Display impl already does.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                use std::io::Write;
                let _ = writeln!(f, "[panic] {info}");
            }
        }));
        // (b) Try the backtrace too, but tolerate failure. This
        //     fires force_capture which uses TLS internally; during
        //     thread shutdown that can itself panic with
        //     AccessError. `catch_unwind` keeps the AccessError
        //     from cascading into a double-panic abort.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let bt = std::backtrace::Backtrace::force_capture();
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                use std::io::Write;
                let _ = writeln!(f, "{bt}");
            }
        }));
    }));
}

/// Default layout-px-per-cell for runtime-server clients. Picked
/// to roughly match the aspect of a typical monospace cell (~half
/// as wide as tall) so author px values land at sane cell sizes
/// when the dev-host serves a mobile/desktop app. `hello-terminal`-
/// style apps (authored in cell units) can opt out via
/// [`RunOptions::cell_size`]. Tweaking these requires a sidecar
/// reconnect — `cell_size` is captured at backend mount and the
/// reported viewport reflects it.
pub const DEFAULT_RUNTIME_SERVER_CELL_SIZE: (f32, f32) = (8.0, 16.0);

/// Rows of scroll per mouse-wheel tick. Three matches the common
/// browser default and feels right for a character-grid viewport
/// (one row per tick is too laggy; the backend clamps to content
/// bounds so over-scrolling is harmless).
const SCROLL_STEP: f32 = 3.0;

#[derive(Clone)]
pub struct RunOptions {
    /// Cap on how many times per second the render loop wakes up.
    /// 30 is plenty for ASCII; lower if you want to save CPU.
    pub target_fps: u32,
    /// Single global key handler. Receives every key event before the
    /// quit-check. Returning `true` suppresses default behaviour
    /// (including quit-on-q). Useful for demos that want the full
    /// keyboard.
    pub on_key: Option<Rc<dyn Fn(&KeyEvent) -> bool>>,
    /// Optional layout-px-per-cell scaling factor `(w, h)`. None
    /// keeps the default `(1.0, 1.0)` (1 px = 1 cell, suits
    /// terminal-native UIs). Mobile/desktop layouts whose stylesheet
    /// uses larger px values should set this so author values don't
    /// overflow the cell viewport — `(8.0, 16.0)` is a reasonable
    /// starting point.
    pub cell_size: Option<(f32, f32)>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            target_fps: 30,
            on_key: None,
            cell_size: None,
        }
    }
}

#[derive(Debug)]
pub enum RunError {
    Io(io::Error),
}

impl From<io::Error> for RunError {
    fn from(e: io::Error) -> Self {
        RunError::Io(e)
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Io(e) => write!(f, "terminal host io error: {e}"),
        }
    }
}

impl std::error::Error for RunError {}

/// Flatten a [`Grid`] into one trimmed `String` per row (control / null
/// glyphs rendered as spaces). Diagnostic helper for [`render_headless`].
fn grid_to_rows(grid: &Grid) -> Vec<String> {
    (0..grid.rows)
        .map(|r| {
            let mut line = String::with_capacity(grid.cols as usize);
            for c in 0..grid.cols {
                let g = grid.cell(c, r).map(|cell| cell.glyph).unwrap_or(' ');
                line.push(if g.is_control() || g == '\0' { ' ' } else { g });
            }
            line.trim_end().to_string()
        })
        .collect()
}

fn is_quit_key(key: &KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
    {
        return true;
    }
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
}

/// Stream `grid` to stdout as ANSI. When `prev` is supplied, only
/// cells that changed are rewritten — same posture every TUI uses to
/// keep paint cost flat.
fn paint_grid(
    out: &mut io::Stdout,
    grid: &Grid,
    prev: Option<&Grid>,
) -> Result<(), RunError> {
    let same_size = prev
        .map(|p| p.cols == grid.cols && p.rows == grid.rows)
        .unwrap_or(false);

    let mut last_fg: Option<Option<Rgba>> = None;
    let mut last_bg: Option<Option<Rgba>> = None;
    let mut last_row: Option<u16> = None;
    let mut last_col: Option<u16> = None;

    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let cell = grid.cell(col, row).copied().unwrap_or_default();
            if same_size {
                if let Some(p) = prev {
                    if p.cell(col, row).copied().unwrap_or_default() == cell {
                        continue;
                    }
                }
            }
            // Move cursor only when we have to (skipped cells leave
            // gaps).
            let need_move = match (last_row, last_col) {
                (Some(r), Some(c)) if r == row && c + 1 == col => false,
                _ => true,
            };
            if need_move {
                queue!(out, cursor::MoveTo(col, row))?;
            }
            if last_fg != Some(cell.fg) {
                match cell.fg {
                    Some(c) => queue!(out, SetForegroundColor(to_ct(c)))?,
                    None => queue!(out, SetForegroundColor(CtColor::Reset))?,
                }
                last_fg = Some(cell.fg);
            }
            if last_bg != Some(cell.bg) {
                match cell.bg {
                    Some(c) => queue!(out, SetBackgroundColor(to_ct(c)))?,
                    None => queue!(out, SetBackgroundColor(CtColor::Reset))?,
                }
                last_bg = Some(cell.bg);
            }
            // Encode the char manually to avoid SetAttribute's String
            // allocation.
            let mut buf = [0u8; 4];
            out.write_all(cell.glyph.encode_utf8(&mut buf).as_bytes())?;
            last_row = Some(row);
            last_col = Some(col);
        }
    }
    Ok(())
}

/// Convert a crossterm `KeyEvent` to the backend's portable
/// [`TerminalKey`]. The string vocabulary matches the framework's
/// `KeyEvent::key` contract (web's `KeyboardEvent.key`): single chars
/// are their literal value, named keys are `"Enter"`, `"Backspace"`,
/// `"ArrowLeft"`, etc.
fn to_terminal_key(k: &KeyEvent) -> Option<TerminalKey> {
    let key = match k.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Esc => "Escape".to_string(),
        KeyCode::Left => "ArrowLeft".to_string(),
        KeyCode::Right => "ArrowRight".to_string(),
        KeyCode::Up => "ArrowUp".to_string(),
        KeyCode::Down => "ArrowDown".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        _ => return None,
    };
    Some(TerminalKey {
        key,
        shift: k.modifiers.contains(KeyModifiers::SHIFT),
        ctrl: k.modifiers.contains(KeyModifiers::CONTROL),
        alt: k.modifiers.contains(KeyModifiers::ALT),
        meta: k.modifiers.contains(KeyModifiers::META),
    })
}

fn to_ct(c: Rgba) -> CtColor {
    // ANSI true-color RGB. Modern terminals (kitty, iTerm2, Alacritty,
    // VS Code's integrated terminal, Apple Terminal in recent
    // macOS) all support this.
    CtColor::Rgb {
        r: c.r,
        g: c.g,
        b: c.b,
    }
}

