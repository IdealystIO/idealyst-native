//! Boot entries for the terminal host — the crossterm event loop wired
//! to `backend_terminal::newcore`'s world boot + dispatch-hook flush
//! driver.
//!
//! [`run`] does the whole boot (stderr redirect, panic hook, raw mode,
//! scheduler install, backend construction + global self-handle,
//! cell_size/viewport seeding, then the input/tick/paint frame loop);
//! the mount itself is `backend_terminal::newcore::start`. The flush
//! discipline needs NO loop changes:
//!
//! - **Input events** (clicks, keys, wheel) dispatch through the same
//!   backend entry points; the author callbacks they reach were wrapped
//!   at the caps layer, so each queues one deduped flush microtask.
//! - **Timers/raf** run inside `scheduler::tick`, whose fire sites
//!   invoke the backend's post-dispatch hook after each callback.
//! - `scheduler::tick()`'s trailing microtask drain commits the flush
//!   BEFORE this frame's `render_to_grid` — input event → staged
//!   writes → flush → paint, every frame.
//!
//! On quit the loop calls [`backend_terminal::newcore::NewCoreApp::stop`]
//! before restoring the terminal — reactive teardown must run while the
//! thread's TLS is intact.
//!
//! [`render_headless`] is the no-TTY twin of [`run`] for diagnostics and
//! snapshot tests.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::time::{Duration, Instant};

use backend_terminal::newcore::{SceneElement, SceneRegistry};
use backend_terminal::{Grid, TerminalBackend};
use crossterm::{
    cursor,
    event::{DisableMouseCapture, EnableMouseCapture, Event, MouseButton, MouseEvent, MouseEventKind},
    execute,
    style::ResetColor,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::{
    grid_to_rows, is_quit_key, paint_grid, scheduler, stderr_redirect, to_terminal_key,
    KeyEventKind, RunError, RunOptions, SCROLL_STEP,
};

/// Mount `build` and render it once, headless — no TTY, no raw mode, no
/// alternate screen. Drives `frames` scheduler ticks between renders so
/// deferred microtasks (and the flush driver's queued flushes) actually
/// run before the snapshot. Returns the final grid as one trimmed
/// `String` per row.
///
/// This is the no-TTY counterpart to [`run`] for diagnostics and
/// snapshot-style tests: the live `run` loop needs a controlling
/// terminal (raw mode + crossterm input), which isn't available in CI or
/// a piped shell. Use ≥2 frames to capture content that only appears
/// after the first post-mount microtask drain.
pub fn render_headless<F, R>(
    build: F,
    register: R,
    cols: u16,
    rows: u16,
    cell_size: Option<(f32, f32)>,
    frames: usize,
) -> Vec<String>
where
    F: FnOnce() -> SceneElement,
    R: FnOnce(&mut SceneRegistry<TerminalBackend>),
{
    scheduler::install();
    let backend = Rc::new(RefCell::new(TerminalBackend::new()));
    backend_terminal::install_global_self(Rc::downgrade(&backend));
    if let Some((w, h)) = cell_size {
        backend.borrow_mut().set_cell_size(w, h);
    }
    backend.borrow_mut().set_viewport(cols, rows);

    let app = backend_terminal::newcore::start(backend.clone(), register, build);

    let mut out = Vec::new();
    for _ in 0..frames.max(1) {
        scheduler::tick();
        let grid = backend.borrow_mut().render_to_grid();
        out = grid_to_rows(&grid);
    }
    app.stop();
    out
}

/// Boot crossterm, mount `build`, and drive the render loop until the
/// user quits. Restores the terminal state on return.
///
/// `register` runs after `runtime_vocabulary::register_builtins` against
/// the scene registry — this is where SDK leaf crates (navigators,
/// third-party `External` providers) install their handlers, mirroring
/// the web/iOS/Android wrappers' `register_scene_extensions` seam. Pass
/// `|_| {}` if the app has no SDKs to register.
pub fn run<F, R>(build: F, opts: RunOptions, register: R) -> Result<(), RunError>
where
    F: FnOnce() -> SceneElement,
    R: FnOnce(&mut SceneRegistry<TerminalBackend>),
{
    let mut stdout = io::stdout();
    // Steal stderr BEFORE raw mode so any framework / hot-reload /
    // runtime-server `eprintln!` lands in the log file instead of
    // stomping on crossterm's paint stream (see `stderr_redirect.rs`),
    // then install the panic hook so panics land in the same log.
    let _stderr = stderr_redirect::StderrRedirect::install(&crate::default_log_path());
    crate::install_panic_log_hook(crate::default_log_path());

    enable_raw_mode()?;
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        cursor::Hide,
        Clear(ClearType::All)
    )?;

    // Scheduler BEFORE mount — the flush driver rides
    // `schedule_microtask` and must not hit the buffering fallback
    // after boot.
    scheduler::install();

    let backend = Rc::new(RefCell::new(TerminalBackend::new()));
    backend_terminal::install_global_self(Rc::downgrade(&backend));

    // cell_size + viewport BEFORE mount (measure_fns capture cell_size
    // at install; the world's viewport ctx seeds from the TLS value the
    // set_viewport call writes).
    if let Some((w, h)) = opts.cell_size {
        backend.borrow_mut().set_cell_size(w, h);
    }
    let (cols, rows) = crossterm::terminal::size()?;
    backend.borrow_mut().set_viewport(cols, rows);

    // Mount: world boot + dispatch-hook flush driver
    // (backend_terminal::newcore module docs). Held in a local for the
    // whole loop; `stop()` runs the reactive teardown BEFORE the
    // terminal restore below (TLS-order rationale in the module docs).
    let app = backend_terminal::newcore::start(backend.clone(), register, build);

    let frame_budget = Duration::from_secs_f64(1.0 / opts.target_fps as f64);
    let mut prev_grid: Option<Grid> = None;

    let result = (|| -> Result<(), RunError> {
        loop {
            let frame_start = Instant::now();
            let poll_budget = frame_budget;
            let mut quit = false;

            // 1. Drain input. The handlers the backend hands back are
            //    the caps-wrapped closures, so plain `h()` queues the
            //    flush.
            while crossterm::event::poll(Duration::from_millis(0))? {
                match crossterm::event::read()? {
                    Event::Resize(new_cols, new_rows) => {
                        backend.borrow_mut().set_viewport(new_cols, new_rows);
                        prev_grid = None;
                        execute!(stdout, Clear(ClearType::All))?;
                    }
                    Event::Mouse(MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column,
                        row,
                        ..
                    }) => {
                        // Borrow discipline: the handler fires AFTER the
                        // backend borrow drops — it typically writes a
                        // signal, which re-enters the backend.
                        let outcome = backend.borrow_mut().dispatch_click(column, row);
                        if let backend_terminal::ClickOutcome::HandlerFired(h) = outcome {
                            h();
                        }
                    }
                    Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollDown, column, row, .. }) => {
                        backend.borrow_mut().dispatch_scroll(column, row, 0.0, SCROLL_STEP);
                    }
                    Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollUp, column, row, .. }) => {
                        backend.borrow_mut().dispatch_scroll(column, row, 0.0, -SCROLL_STEP);
                    }
                    Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollRight, column, row, .. }) => {
                        backend.borrow_mut().dispatch_scroll(column, row, SCROLL_STEP, 0.0);
                    }
                    Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollLeft, column, row, .. }) => {
                        backend.borrow_mut().dispatch_scroll(column, row, -SCROLL_STEP, 0.0);
                    }
                    Event::Key(key) => {
                        if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
                            continue;
                        }
                        if let Some(tk) = to_terminal_key(&key) {
                            if backend.borrow_mut().dispatch_key(&tk) {
                                continue;
                            }
                        }
                        if let Some(cb) = opts.on_key.as_ref() {
                            if cb(&key) {
                                continue;
                            }
                        }
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

            // 2. Pump the scheduler: timers/raf fire (each followed by
            //    the post-dispatch hook), then the trailing microtask
            //    drain commits any queued flush — staged writes land
            //    before this frame's compose.
            scheduler::tick();

            // 3-4. Compose + diff-paint.
            let grid = backend.borrow_mut().render_to_grid();
            paint_grid(&mut stdout, &grid, prev_grid.as_ref())?;
            {
                use std::io::Write;
                stdout.flush()?;
            }
            prev_grid = Some(grid);

            // 5. Sleep until the next frame tick — but only if no
            //    animation is in flight. With no pending work, blocking
            //    on `poll` for the rest of the budget keeps idle CPU
            //    near zero.
            let elapsed = frame_start.elapsed();
            if elapsed < poll_budget {
                if scheduler::has_pending() {
                    std::thread::sleep(poll_budget - elapsed);
                } else {
                    let _ = crossterm::event::poll(poll_budget - elapsed);
                }
            }
        }
        Ok(())
    })();

    // Reactive teardown FIRST (while TLS is intact), then restore the
    // terminal — even on error.
    app.stop();
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
