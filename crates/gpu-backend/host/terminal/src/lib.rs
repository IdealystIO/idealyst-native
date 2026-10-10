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
/// teardown the arena TLS may already be destroyed — a panic
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
    /// Single global key handler. Receives every key PRESS (and
    /// auto-repeat) the backend didn't consume — i.e. not claimed by an
    /// app key listener (`PreventDefault`) or taken by a focused input —
    /// before the quit-check. Releases never reach it. Returning `true` suppresses default behaviour
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
/// [`TerminalKey`]. The `key` vocabulary matches the framework's
/// `KeyEvent::key` contract (web's `KeyboardEvent.key`): single chars
/// are their literal value (`" "` for Space), named keys are `"Enter"`,
/// `"Backspace"`, `"ArrowLeft"`, `"F5"`, `"Shift"`, etc.
///
/// `code` is best-effort: terminals report the character, not the
/// physical key, so it is derived from the key
/// ([`backend_terminal::code_for_key`], US layout). Keypad digits (kitty
/// protocol `KEYPAD` state) map to `NumpadN`, and modifier keys (only
/// reported under the kitty protocol) keep their side.
fn to_terminal_key(k: &KeyEvent) -> Option<TerminalKey> {
    use crossterm::event::{KeyEventState, ModifierKeyCode as M};
    let key = match k.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::Tab | KeyCode::BackTab => "Tab".to_string(),
        KeyCode::Esc => "Escape".to_string(),
        KeyCode::Left => "ArrowLeft".to_string(),
        KeyCode::Right => "ArrowRight".to_string(),
        KeyCode::Up => "ArrowUp".to_string(),
        KeyCode::Down => "ArrowDown".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::CapsLock => "CapsLock".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::Modifier(m) => match m {
            M::LeftShift | M::RightShift => "Shift",
            M::LeftControl | M::RightControl => "Control",
            M::LeftAlt | M::RightAlt => "Alt",
            M::LeftSuper | M::RightSuper | M::LeftMeta | M::RightMeta => "Meta",
            M::IsoLevel3Shift => "AltGraph",
            _ => return None,
        }
        .to_string(),
        _ => return None,
    };
    let code = match k.code {
        KeyCode::Modifier(m) => match m {
            M::LeftShift => "ShiftLeft",
            M::RightShift => "ShiftRight",
            M::LeftControl => "ControlLeft",
            M::RightControl => "ControlRight",
            M::LeftAlt => "AltLeft",
            M::RightAlt | M::IsoLevel3Shift => "AltRight",
            M::LeftSuper | M::LeftMeta => "MetaLeft",
            M::RightSuper | M::RightMeta => "MetaRight",
            _ => "",
        },
        KeyCode::Char(c) if k.state.contains(KeyEventState::KEYPAD) => match c {
            '0'..='9' => {
                const NUMPAD: [&str; 10] = [
                    "Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4", "Numpad5", "Numpad6",
                    "Numpad7", "Numpad8", "Numpad9",
                ];
                NUMPAD[(c as u8 - b'0') as usize]
            }
            '+' => "NumpadAdd",
            '-' => "NumpadSubtract",
            '*' => "NumpadMultiply",
            '/' => "NumpadDivide",
            '.' => "NumpadDecimal",
            _ => backend_terminal::code_for_key(&key),
        },
        KeyCode::Enter if k.state.contains(KeyEventState::KEYPAD) => "NumpadEnter",
        _ => backend_terminal::code_for_key(&key),
    };
    Some(TerminalKey {
        code: code.to_string(),
        key,
        pressed: k.kind != KeyEventKind::Release,
        repeat: k.kind == KeyEventKind::Repeat,
        shift: k.modifiers.contains(KeyModifiers::SHIFT),
        ctrl: k.modifiers.contains(KeyModifiers::CONTROL),
        alt: k.modifiers.contains(KeyModifiers::ALT),
        meta: k.modifiers.contains(KeyModifiers::META)
            || k.modifiers.contains(KeyModifiers::SUPER),
    })
}

/// The [`TerminalKey`]s one crossterm key event delivers to the backend.
///
/// `releases_reported` says whether this terminal sends key releases
/// (see [`enable_key_reporting`]). When it doesn't, every press is
/// followed by a synthesized release so the app's held-key state never
/// sticks — a game on such a terminal sees taps (down+up per press and
/// per auto-repeat), not holds.
fn terminal_key_events(k: &KeyEvent, releases_reported: bool) -> Vec<TerminalKey> {
    let Some(tk) = to_terminal_key(k) else { return Vec::new() };
    if tk.pressed && !releases_reported {
        // An auto-repeat on such a terminal is indistinguishable from a
        // fresh press; after the synthesized release it IS a fresh press.
        let press = TerminalKey { repeat: false, ..tk };
        let release = press.released();
        vec![press, release]
    } else {
        vec![tk]
    }
}

/// Deliver one crossterm key event to the backend. Returns `true` if a
/// press was consumed (claimed by an app key listener or taken by a
/// focused input) — the caller then skips its `on_key` / quit handling.
/// Releases never count as consumed (the caller ignores them anyway).
fn dispatch_terminal_key(
    backend: &std::cell::RefCell<backend_terminal::TerminalBackend>,
    k: &KeyEvent,
    releases_reported: bool,
) -> bool {
    let mut consumed = false;
    for tk in terminal_key_events(k, releases_reported) {
        // `dispatch_key_in`, not `borrow_mut().dispatch_key`: the app
        // key sink runs with the backend unborrowed (its listeners may
        // add/remove listeners → `set_keyboard_sink` on this backend).
        let taken = backend_terminal::TerminalBackend::dispatch_key_in(backend, &tk);
        consumed |= taken && tk.pressed;
    }
    consumed
}

/// Turn on the terminal input reporting the app keyboard needs, after
/// raw mode is on. Returns whether key RELEASES will be reported:
///
/// - **kitty keyboard protocol** (kitty, WezTerm, foot, Ghostty, recent
///   iTerm2/Alacritty…): pushed with `REPORT_EVENT_TYPES` (press /
///   repeat / release) + `DISAMBIGUATE_ESCAPE_CODES` when
///   `supports_keyboard_enhancement()` says the terminal speaks it.
///   Popped again by [`disable_key_reporting`].
/// - **Windows console**: crossterm reports releases natively (and
///   `supports_keyboard_enhancement` is always `false` there).
/// - **anything else**: no releases — [`terminal_key_events`] synthesizes
///   them.
///
/// Focus reporting (`EnableFocusChange`) is turned on too so a terminal
/// focus-out reaches the app keyboard as `focus_lost`; terminals that
/// don't support it ignore the escape.
fn enable_key_reporting(out: &mut io::Stdout) -> (bool, bool) {
    use crossterm::event::{
        EnableFocusChange, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    };
    let _ = crossterm::execute!(out, EnableFocusChange);
    let pushed = matches!(crossterm::terminal::supports_keyboard_enhancement(), Ok(true))
        && crossterm::execute!(
            out,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        )
        .is_ok();
    (pushed || cfg!(windows), pushed)
}

/// Undo [`enable_key_reporting`] on exit (`pushed` = its second value).
fn disable_key_reporting(out: &mut io::Stdout, pushed: bool) {
    use crossterm::event::{DisableFocusChange, PopKeyboardEnhancementFlags};
    if pushed {
        let _ = crossterm::execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = crossterm::execute!(out, DisableFocusChange);
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


#[cfg(test)]
mod key_tests {
    use super::*;
    use crossterm::event::{KeyEventState, ModifierKeyCode};

    fn ev(code: KeyCode, kind: KeyEventKind) -> KeyEvent {
        KeyEvent { code, modifiers: KeyModifiers::NONE, kind, state: KeyEventState::NONE }
    }

    fn summary(v: &[TerminalKey]) -> Vec<(String, String, bool, bool)> {
        v.iter().map(|k| (k.key.clone(), k.code.clone(), k.pressed, k.repeat)).collect()
    }

    /// With the kitty protocol, press / repeat / release map 1:1.
    #[test]
    fn kitty_protocol_kinds_map_to_phases() {
        let w = |kind| terminal_key_events(&ev(KeyCode::Char('w'), kind), true);
        assert_eq!(summary(&w(KeyEventKind::Press)), vec![("w".into(), "KeyW".into(), true, false)]);
        assert_eq!(summary(&w(KeyEventKind::Repeat)), vec![("w".into(), "KeyW".into(), true, true)]);
        assert_eq!(summary(&w(KeyEventKind::Release)), vec![("w".into(), "KeyW".into(), false, false)]);
    }

    /// Regression: without release reporting the host forwarded presses
    /// only, so (once the app keyboard tracked held keys) every key would
    /// stay "down" forever. Each press is now a tap.
    #[test]
    fn regression_terminal_without_releases_keys_stuck_down() {
        let tap = terminal_key_events(&ev(KeyCode::Char(' '), KeyEventKind::Press), false);
        assert_eq!(
            summary(&tap),
            vec![(" ".into(), "Space".into(), true, false), (" ".into(), "Space".into(), false, false)]
        );
        // A terminal-level auto-repeat is a fresh tap too.
        let rep = terminal_key_events(&ev(KeyCode::Up, KeyEventKind::Repeat), false);
        assert_eq!(
            summary(&rep),
            vec![
                ("ArrowUp".into(), "ArrowUp".into(), true, false),
                ("ArrowUp".into(), "ArrowUp".into(), false, false),
            ]
        );
    }

    #[test]
    fn named_modifier_and_keypad_keys() {
        let one = |e: KeyEvent| {
            let v = terminal_key_events(&e, true);
            (v[0].key.clone(), v[0].code.clone())
        };
        assert_eq!(one(ev(KeyCode::F(5), KeyEventKind::Press)), ("F5".into(), "F5".into()));
        assert_eq!(
            one(ev(KeyCode::Modifier(ModifierKeyCode::RightShift), KeyEventKind::Press)),
            ("Shift".into(), "ShiftRight".into())
        );
        assert_eq!(
            one(ev(KeyCode::Modifier(ModifierKeyCode::LeftSuper), KeyEventKind::Press)),
            ("Meta".into(), "MetaLeft".into())
        );
        let kp = KeyEvent { state: KeyEventState::KEYPAD, ..ev(KeyCode::Char('7'), KeyEventKind::Press) };
        assert_eq!(one(kp), ("7".into(), "Numpad7".into()));
        let kp_enter = KeyEvent { state: KeyEventState::KEYPAD, ..ev(KeyCode::Enter, KeyEventKind::Press) };
        assert_eq!(one(kp_enter), ("Enter".into(), "NumpadEnter".into()));
        assert_eq!(one(ev(KeyCode::Char('A'), KeyEventKind::Press)), ("A".into(), "KeyA".into()));
        assert!(terminal_key_events(&ev(KeyCode::Null, KeyEventKind::Press), true).is_empty());
    }

    /// End to end through the backend: a claimed press is consumed, the
    /// release is delivered but never "consumed".
    #[test]
    fn dispatch_reaches_backend_sink_with_both_phases() {
        use runtime_shared::primitives::key::{AppKeyEvent, KeyOutcome, KeyboardSink};
        use runtime_vocabulary::caps::AppEnvOps;
        let set_sink = |b: &std::cell::RefCell<backend_terminal::TerminalBackend>, s| {
            b.borrow_mut().set_keyboard_sink(Some(s))
        };
        let backend = std::cell::RefCell::new(backend_terminal::TerminalBackend::new());
        let log: Rc<std::cell::RefCell<Vec<AppKeyEvent>>> = Rc::default();
        let l = log.clone();
        set_sink(
            &backend,
            KeyboardSink::new(
                move |e| {
                    l.borrow_mut().push(e.clone());
                    KeyOutcome::PreventDefault
                },
                || {},
            ),
        );
        assert!(dispatch_terminal_key(&backend, &ev(KeyCode::Char('d'), KeyEventKind::Press), false));
        assert_eq!(log.borrow().len(), 2, "press + synthesized release");
    }
}
