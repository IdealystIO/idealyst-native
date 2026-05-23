//! Terminal host shell for `backend-terminal`.
//!
//! Boots crossterm (raw mode + alternate screen + mouse capture),
//! mounts the user's `app()`, runs a render loop that:
//!   1. Drains terminal events (resize, keys, mouse) and dispatches.
//!   2. Asks the backend to lay out + compose a fresh
//!      [`backend_terminal::Grid`].
//!   3. Diffs the grid against the previous frame and emits the
//!      minimal ANSI escape stream to stdout.
//!   4. Sleeps until the next frame tick.
//!
//! Quits cleanly on `q`, `Esc`, or `Ctrl-C`, restoring the
//! terminal's prior state.

use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;
use std::time::{Duration, Instant};

mod scheduler;

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

use backend_terminal::{Grid, TerminalBackend, TerminalKey};
use crossterm::{
    cursor,
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, MouseButton, MouseEvent,
        MouseEventKind,
    },
    execute, queue,
    style::{Color as CtColor, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
pub use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use framework_core::color::Rgba;
use framework_core::Primitive;

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

/// Boot crossterm, mount `app`, and drive the render loop until the
/// user quits. Restores the terminal state on return.
pub fn run<F>(app: F, opts: RunOptions) -> Result<(), RunError>
where
    F: Fn() -> Primitive + 'static,
{
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        cursor::Hide,
        Clear(ClearType::All)
    )?;

    // Install the framework's `Scheduler` *before* the first
    // `mount(...)` — animation timers, `after_ms`, presence-anim
    // unmount delays all read this on first construction. The native
    // fallback (synchronous fire-now) would otherwise stick.
    scheduler::install();

    let backend = Rc::new(RefCell::new(TerminalBackend::new()));
    // Install the self-handle the backend's `Toggle` click handler
    // and `ActivityIndicator` rAF loop use to reach back into the
    // backend without capturing it directly. Mirrors the
    // `install_global_self` pattern in `backend-macos`.
    backend_terminal::install_global_self(Rc::downgrade(&backend));

    // Apply the host's chosen cell_size BEFORE the first mount —
    // measure_fns capture the value at install time, so changing
    // it mid-session wouldn't apply to already-mounted text.
    if let Some((w, h)) = opts.cell_size {
        backend.borrow_mut().set_cell_size(w, h);
    }

    // Initial viewport snapshot.
    let (cols, rows) = crossterm::terminal::size()?;
    backend.borrow_mut().set_viewport(cols, rows);

    // Mount the user's app — same posture as host-appkit: `mount`
    // adopts the framework's root scope so `effect!` / `signal!`
    // / `Ref` declarations inside the user's component bodies stay
    // alive for the whole session.
    //
    // Bind the `Owner` to a local that lives for the whole run loop
    // and drop it explicitly *before* the backend Rc + the TLS-bound
    // reactive arena get torn down. The macOS host gets away with
    // `mem::forget` because `nsapp.run()` never returns — but our
    // terminal host returns cleanly on quit, and if the framework's
    // reactive arena TLS is destroyed before the Owner's drop walks
    // it, you get "cannot access TLS during destruction" panics
    // after the user already saw their shell prompt.
    let _owner = framework_core::mount(backend.clone(), app);

    let frame_budget = Duration::from_secs_f64(1.0 / opts.target_fps as f64);
    let mut prev_grid: Option<Grid> = None;

    let result = (|| -> Result<(), RunError> {
        loop {
            let frame_start = Instant::now();

            // 1. Drain pending input. Block for at most one frame's
            //    worth so we still tick the render loop when the user
            //    is idle.
            let poll_budget = frame_budget;
            let mut quit = false;
            while crossterm::event::poll(Duration::from_millis(0))? {
                match crossterm::event::read()? {
                    Event::Resize(new_cols, new_rows) => {
                        backend.borrow_mut().set_viewport(new_cols, new_rows);
                        // Force a full repaint on resize.
                        prev_grid = None;
                        execute!(stdout, Clear(ClearType::All))?;
                    }
                    Event::Mouse(MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column,
                        row,
                        ..
                    }) => {
                        // Backend walks the tree deepest-first and
                        // *returns* the handler instead of firing it
                        // — the click closure typically calls
                        // `Signal::set`, which re-enters the backend
                        // via the framework's reactive effect chain.
                        // Releasing the borrow before invoking it is
                        // the only way to avoid a "RefCell already
                        // borrowed" panic. Same pattern the original
                        // `hit_test` shape used.
                        let outcome = backend.borrow_mut().dispatch_click(column, row);
                        if let backend_terminal::ClickOutcome::HandlerFired(h) = outcome {
                            h();
                        }
                    }
                    Event::Key(key) => {
                        if key.kind != KeyEventKind::Press
                            && key.kind != KeyEventKind::Repeat
                        {
                            continue;
                        }
                        // 1. Focused TextInput gets first crack. If
                        //    it consumes the key, suppress everything
                        //    downstream (global on_key, quit
                        //    detection) so typing 'q' into an input
                        //    doesn't kill the app.
                        if let Some(tk) = to_terminal_key(&key) {
                            if backend.borrow_mut().dispatch_key(&tk) {
                                continue;
                            }
                        }
                        // 2. Author's global handler.
                        if let Some(cb) = opts.on_key.as_ref() {
                            if cb(&key) {
                                continue;
                            }
                        }
                        // 3. Built-in quit shortcuts.
                        if is_quit_key(&key) {
                            quit = true;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if quit {
                break;
            }

            // 2. Pump the framework's scheduler. This fires expired
            //    `after_ms` callbacks, next-frame one-shots, and any
            //    `raf_loop` subscribers — including the per-frame
            //    writes from `AnimatedValue::bind`. The walker's
            //    reactive effects re-fire automatically when a
            //    backend method (`set_animated_f32`, `update_text`,
            //    etc.) writes through a signal.
            scheduler::tick();

            // 3. Compose the next frame.
            let grid = backend.borrow_mut().render_to_grid();

            // 4. Paint via diff against prev_grid.
            paint_grid(&mut stdout, &grid, prev_grid.as_ref())?;
            stdout.flush()?;
            prev_grid = Some(grid);

            // 5. Sleep until the next frame tick — but only if no
            //    animation is in flight. If `has_pending()` is true,
            //    we want the loop to spin (capped at `target_fps`)
            //    so animations actually advance. With no pending
            //    work, blocking on `poll` for the rest of the budget
            //    keeps idle CPU near zero.
            let elapsed = frame_start.elapsed();
            if elapsed < poll_budget {
                if scheduler::has_pending() {
                    // Cooperative yield — just long enough to keep
                    // us under the FPS cap. Don't block on `poll`
                    // since we want to come right back to advance
                    // animations.
                    std::thread::sleep(poll_budget - elapsed);
                } else {
                    let _ = crossterm::event::poll(poll_budget - elapsed);
                }
            }
        }
        Ok(())
    })();

    // Always restore the terminal, even on error.
    let _ = execute!(
        stdout,
        ResetColor,
        cursor::Show,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
    result
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

