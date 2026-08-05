//! Runtime-server-client leg of the terminal host (`runtime-server`
//! feature). Instead of mounting a local app, it spawns a
//! [`RuntimeServerShell`] that connects to an `idealyst dev` host over
//! WebSocket and replays the streamed wire commands into the same
//! crossterm grid the local boot path paints.
//!
//! This path is core-agnostic by construction — no `Element`, no mount,
//! no world; the shell is a wire replayer. The one coupling left is the
//! shell's generic bound: `RuntimeServerShell<B>` in
//! `crates/dev/runtime-server-shell/src/shell.rs` still reads
//! `B: runtime_core::Backend`, inherited from `dev_client::WireBackend`.
//! Until that pair re-bounds onto `runtime_vocabulary::caps` (the change
//! `dev-client`'s own module docs describe as "contained entirely in
//! this crate"), enabling this feature does not compile. Nothing in this
//! file changes when it lands — `TerminalBackend` already adopts the
//! caps.

use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;
use std::time::{Duration, Instant};

use backend_terminal::{Grid, TerminalBackend};
use crossterm::{
    cursor,
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, MouseButton, MouseEvent,
        MouseEventKind,
    },
    execute,
    style::ResetColor,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};

use crate::{
    is_quit_key, paint_grid, scheduler, stderr_redirect, to_terminal_key, KeyEventKind,
    RunError, RunOptions, DEFAULT_RUNTIME_SERVER_CELL_SIZE, SCROLL_STEP,
};

/// Runtime-server variant of [`crate::run`]. Same crossterm boot + frame
/// loop, but instead of mounting a local `app()` it spawns a
/// `RuntimeServerShell<TerminalBackend>` that connects to the dev-
/// host at `url` (CLI-baked via the `IDEALYST_DEV_ENDPOINT` env
/// var) and applies the streamed wire commands into the terminal
/// grid every frame.
///
/// The shell is ticked once per frame (inside the existing render
/// loop) which: (a) applies pending inbound commands, (b) sends
/// `RequestFrame` so the sidecar advances its animation clock,
/// (c) reports the current viewport on resize. The sidecar's
/// `RecordingViewOps::frame()` reads then return the actual
/// terminal cell-grid size — author code reading
/// `page_ref.frame()` sees real bounds, not the mobile-portrait
/// fallback.
pub fn run_runtime_server(url: String, opts: RunOptions) -> Result<(), RunError> {
    let mut stdout = io::stdout();
    // Same posture as `run`: redirect stderr to the project's
    // terminal log so the runtime-server shell's connect /
    // disconnect chatter doesn't corrupt the cell grid.
    let _stderr = stderr_redirect::StderrRedirect::install(&default_log_path());
    enable_raw_mode()?;
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        cursor::Hide,
        Clear(ClearType::All)
    )?;

    scheduler::install();

    let backend = Rc::new(RefCell::new(TerminalBackend::new()));
    backend_terminal::install_global_self(Rc::downgrade(&backend));

    // Runtime-server clients almost always connect to a dev-host
    // serving a mobile/desktop app whose stylesheet uses px values
    // calibrated for those densities (a 200-px planet is normal on
    // an iOS viewport). The default cell_size of (1.0, 1.0) treats
    // 1 px = 1 cell, which makes that 200-px planet render as 200
    // cells — overflowing every terminal. Default to roughly the
    // aspect ratio of a typical monospace cell so author px values
    // land at sane cell sizes; honor an explicit `opts.cell_size`
    // override for callers (`hello-terminal`-style) that wrote their
    // app in cell units.
    let (cw, ch) = opts.cell_size.unwrap_or(DEFAULT_RUNTIME_SERVER_CELL_SIZE);
    backend.borrow_mut().set_cell_size(cw, ch);

    let (cols, rows) = crossterm::terminal::size()?;
    backend.borrow_mut().set_viewport(cols, rows);

    // Spawn the shell against the shared backend Rc — same
    // `Rc<RefCell<TerminalBackend>>` we'll render from each
    // frame. The shell's apply_batch writes through this Rc;
    // the per-frame `render_to_grid` reads from it.
    //
    // Report the viewport in layout px (cells × cell_size), NOT in
    // cells. The dev-host's Taffy + `RecordingViewOps::frame()`
    // both speak px; reporting cells would tell the sidecar the
    // app has a 80-px-wide viewport and the user's 200-px planet
    // would render past the right edge before it ever reached us.
    let shell = runtime_server_shell_native::RuntimeServerShell::<TerminalBackend>::spawn_with_shared_backend(
        backend.clone(),
        url,
        runtime_server_shell_native::RuntimeServerShellOptions {
            platform: runtime_server_shell_native::WirePlatform::Other,
            device_label: Some(format!("terminal ({}×{})", cols, rows)),
            viewport: Some(runtime_server_shell_native::WireViewport {
                width: cols as f32 * cw,
                height: rows as f32 * ch,
            }),
        },
    );

    let frame_budget = Duration::from_secs_f64(1.0 / opts.target_fps as f64);
    let mut prev_grid: Option<Grid> = None;
    let mut last_viewport = (cols, rows);

    let result = (|| -> Result<(), RunError> {
        loop {
            let frame_start = Instant::now();
            let poll_budget = frame_budget;
            let mut quit = false;

            // Drain input (same shape as local-mount `run`).
            while crossterm::event::poll(Duration::from_millis(0))? {
                match crossterm::event::read()? {
                    Event::Resize(new_cols, new_rows) => {
                        backend.borrow_mut().set_viewport(new_cols, new_rows);
                        last_viewport = (new_cols, new_rows);
                        prev_grid = None;
                        execute!(stdout, Clear(ClearType::All))?;
                    }
                    Event::Mouse(MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column,
                        row,
                        ..
                    }) => {
                        let outcome = backend.borrow_mut().dispatch_click(column, row);
                        if let backend_terminal::ClickOutcome::HandlerFired(h) = outcome {
                            h();
                        }
                    }
                    Event::Mouse(MouseEvent {
                        kind: MouseEventKind::ScrollDown,
                        column,
                        row,
                        ..
                    }) => {
                        backend.borrow_mut().dispatch_scroll(column, row, 0.0, SCROLL_STEP);
                    }
                    Event::Mouse(MouseEvent {
                        kind: MouseEventKind::ScrollUp,
                        column,
                        row,
                        ..
                    }) => {
                        backend.borrow_mut().dispatch_scroll(column, row, 0.0, -SCROLL_STEP);
                    }
                    Event::Mouse(MouseEvent {
                        kind: MouseEventKind::ScrollRight,
                        column,
                        row,
                        ..
                    }) => {
                        backend.borrow_mut().dispatch_scroll(column, row, SCROLL_STEP, 0.0);
                    }
                    Event::Mouse(MouseEvent {
                        kind: MouseEventKind::ScrollLeft,
                        column,
                        row,
                        ..
                    }) => {
                        backend.borrow_mut().dispatch_scroll(column, row, -SCROLL_STEP, 0.0);
                    }
                    Event::Key(key) => {
                        if key.kind != KeyEventKind::Press
                            && key.kind != KeyEventKind::Repeat
                        {
                            continue;
                        }
                        // Focused TextInput gets first crack — the
                        // backend's TextInput primitive is local
                        // bookkeeping (focus, cursor, value); typing
                        // through the wire would round-trip every
                        // keystroke through the sidecar. Same posture
                        // as local-mount `run`: if dispatch_key returns
                        // true the input swallowed it, so don't let it
                        // also count as a quit shortcut.
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

            // Tick the runtime-server shell: apply inbound batch,
            // send `RequestFrame`, report viewport changes. The
            // shell's apply lands on the shared backend `Rc`, so
            // the next `render_to_grid` call below paints the
            // updated scene. Reported viewport is in layout px
            // (cells × cell_size); see the spawn block above for
            // the rationale.
            shell.tick(Some(runtime_server_shell_native::WireViewport {
                width: last_viewport.0 as f32 * cw,
                height: last_viewport.1 as f32 * ch,
            }));

            scheduler::tick();
            let grid = backend.borrow_mut().render_to_grid();
            paint_grid(&mut stdout, &grid, prev_grid.as_ref())?;
            stdout.flush()?;
            prev_grid = Some(grid);

            let elapsed = frame_start.elapsed();
            if elapsed < poll_budget {
                // Runtime-server mode is always "pending" — there
                // could be wire commands arriving on the next
                // worker-thread iteration. Yield rather than block
                // on poll so the next tick happens promptly.
                std::thread::sleep(poll_budget - elapsed);
            }
        }
        Ok(())
    })();

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
