//! New-core adoption tests for the terminal backend: cross-core render
//! parity (rule-7 gate — the same scene must paint the same cells on
//! both cores) plus op-level coverage of the caps adoption's flush
//! discipline (input event → staged writes → flush → paint).
//!
//! Harness notes: the tests install a queue-only scheduler (mirroring
//! `host-terminal`'s tick semantics for microtasks: drain-until-empty).
//! `install_scheduler` is process-global/first-wins, but the queue
//! state is thread-local, so each test thread drains only its own
//! tasks.

#![cfg(feature = "new-core")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_terminal::{ClickOutcome, Grid, TerminalBackend};
use runtime_core::{Color, Length, StyleApplication, StyleRules, StyleSheet, Tokenized};

// ===========================================================================
// Queue-only test scheduler (host-terminal's microtask semantics)
// ===========================================================================

mod test_scheduler {
    use runtime_core::scheduling::{ScheduleHandle, Scheduler};
    use std::cell::RefCell;
    use std::collections::VecDeque;

    thread_local! {
        static QUEUE: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> =
            RefCell::new(VecDeque::new());
    }

    struct NoopHandle;
    impl ScheduleHandle for NoopHandle {
        fn cancel(&mut self) {}
    }

    struct QueueScheduler;
    impl Scheduler for QueueScheduler {
        fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
            QUEUE.with(|q| q.borrow_mut().push_back(f));
        }
        fn after_animation_frame(&self, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
        fn after_ms(&self, _delay_ms: i32, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
        fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
    }

    pub fn ensure_installed() {
        if !runtime_core::scheduling::is_scheduler_installed() {
            runtime_core::scheduling::install_scheduler(Box::new(QueueScheduler));
        }
    }

    /// Drain-until-empty — the same posture as host-terminal's tick.
    pub fn drain() {
        loop {
            let next = QUEUE.with(|q| q.borrow_mut().pop_front());
            match next {
                Some(task) => task(),
                None => break,
            }
        }
    }
}

// ===========================================================================
// Harness
// ===========================================================================

const COLS: u16 = 48;
const ROWS: u16 = 10;

fn fresh_backend() -> Rc<RefCell<TerminalBackend>> {
    test_scheduler::ensure_installed();
    let backend = Rc::new(RefCell::new(TerminalBackend::new()));
    backend_terminal::install_global_self(Rc::downgrade(&backend));
    backend.borrow_mut().set_viewport(COLS, ROWS);
    backend
}

/// Flatten a grid into one trimmed string per row (the host's
/// `grid_to_rows` diagnostic shape), plus the raw cells for the strict
/// color-level compare.
fn grid_rows(grid: &Grid) -> Vec<String> {
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

/// Cell-exact comparison (glyph + fg + bg), with a readable rows dump on
/// divergence.
fn assert_grids_identical(name: &str, old: &Grid, new: &Grid) {
    assert_eq!((old.cols, old.rows), (new.cols, new.rows), "{name}: grid size");
    for r in 0..old.rows {
        for c in 0..old.cols {
            let a = old.cell(c, r).copied().unwrap_or_default();
            let b = new.cell(c, r).copied().unwrap_or_default();
            if a != b {
                panic!(
                    "{name}: cell divergence at ({c},{r}): old {a:?} vs new {b:?}\n\
                     old rows:\n{}\nnew rows:\n{}",
                    grid_rows(old).join("\n"),
                    grid_rows(new).join("\n"),
                );
            }
        }
    }
}

fn render_old(app: impl Fn() -> runtime_core::Element + 'static) -> Grid {
    let backend = fresh_backend();
    let owner = runtime_core::mount(backend.clone(), app);
    test_scheduler::drain();
    let grid = backend.borrow_mut().render_to_grid();
    drop(owner);
    grid
}

fn render_new(build: impl FnOnce() -> runtime_scene::Element) -> Grid {
    let backend = fresh_backend();
    let app = backend_terminal::newcore::start(backend.clone(), |_| {}, build);
    test_scheduler::drain();
    let grid = backend.borrow_mut().render_to_grid();
    app.stop();
    grid
}

fn test_rules(width: f32, background: &str) -> StyleRules {
    StyleRules {
        width: Some(Tokenized::Literal(Length::Px(width))),
        background: Some(Tokenized::Literal(Color(background.to_string()))),
        ..Default::default()
    }
}

fn static_style(width: f32, background: &str) -> StyleApplication {
    StyleApplication::new(Rc::new(StyleSheet::r#static(test_rules(width, background))))
}

// ===========================================================================
// 1. Full-scene render snapshot: same tree, both cores, identical cells
// ===========================================================================

/// The rule-7 gate for this port: a torture scene (styled views, text,
/// button chrome `[ label ]`, toggle glyphs, pressable, colored
/// backgrounds) composed on the OLD core (walker + `runtime_core::mount`)
/// and the NEW core (`newcore::start`, vocabulary handlers) must paint
/// cell-identical grids — glyphs AND colors.
#[test]
fn newcore_full_scene_grid_parity() {
    let old = render_old(|| {
        use runtime_core::{button, pressable, signal, text, toggle, view, IntoElement};
        let on = signal(true);
        view(vec![
            text("hello terminal").into_element(),
            text("styled")
                .with_style(static_style(20.0, "#334455"))
                .into_element(),
            button("Go", || {}).into_element(),
            toggle(on, |_| {}).into_element(),
            pressable(vec![text("press me").into_element()], || {}).into_element(),
        ])
        .with_style(static_style(40.0, "#101020"))
        .into_element()
    });
    let new = render_new(|| {
        use runtime_vocabulary::builders::{button, pressable, text, toggle, view};
        use runtime_world::signal;
        let on = signal(true);
        view()
            .style(test_rules(40.0, "#101020"))
            .child(text().content("hello terminal"))
            .child(text().content("styled").style(test_rules(20.0, "#334455")))
            .child(button().label("Go").on_press(|| {}))
            .child(toggle().value(on).on_change(|_| {}))
            .child(pressable(|| {}).child(text().content("press me")))
            .build()
    });
    assert_grids_identical("full_scene", &old, &new);
    // Sanity: the scene is live (button chrome + toggle glyph painted).
    let rows = grid_rows(&old).join("\n");
    assert!(rows.contains("[ Go ]"), "button chrome painted:\n{rows}");
    assert!(rows.contains('\u{25cf}'), "toggle on-glyph painted:\n{rows}");
    assert!(rows.contains("hello terminal"), "text painted:\n{rows}");
}

// ===========================================================================
// 2. Input events → staged writes → flush → paint (the driver contract)
// ===========================================================================

/// REGRESSION (flush discipline): on the new core a click handler's
/// `Signal::set` is STAGED — nothing observable until the flush driver
/// commits. The caps layer wraps every author callback so firing the
/// handler `dispatch_click` returns queues one deduped flush microtask;
/// the host's drain then commits it before the next paint. Without the
/// wrap the count text would stay stale forever.
#[test]
fn newcore_click_commits_via_flush_before_next_paint() {
    let backend = fresh_backend();
    let app = backend_terminal::newcore::start(backend.clone(), |_| {}, move || {
        use runtime_vocabulary::builders::{button, text, view};
        use runtime_world::signal;
        let count = signal(0i32);
        view()
            .child(text().content(move || format!("count: {}", count.get())))
            .child(button().label("inc").on_press(move || {
                count.update(|c| c + 1);
            }))
            .build()
    });
    test_scheduler::drain();
    let grid = backend.borrow_mut().render_to_grid();
    let rows = grid_rows(&grid).join("\n");
    assert!(rows.contains("count: 0"), "initial paint:\n{rows}");

    // Find the button row and click it (frame cache is fresh from the
    // render above — the dispatch_click precondition).
    let rows_vec = grid_rows(&grid);
    let button_row = rows_vec
        .iter()
        .position(|r| r.contains("[ inc ]"))
        .expect("button row painted") as u16;
    let button_col = rows_vec[button_row as usize].find("[ inc ]").unwrap() as u16 + 2;
    let outcome = backend.borrow_mut().dispatch_click(button_col, button_row);
    let ClickOutcome::HandlerFired(h) = outcome else {
        panic!("click must land on the button, got {outcome:?}");
    };
    // Fire with the backend borrow released (host contract). The
    // wrapped handler stages the write and queues the flush.
    h();
    // Before the drain the write is UNCOMMITTED — the staged-write
    // model's whole point.
    let grid = backend.borrow_mut().render_to_grid();
    assert!(
        grid_rows(&grid).join("\n").contains("count: 0"),
        "write must stay staged until the flush commits"
    );
    // The host's per-frame drain runs the queued flush; the next paint
    // sees the committed value.
    test_scheduler::drain();
    let grid = backend.borrow_mut().render_to_grid();
    let rows = grid_rows(&grid).join("\n");
    assert!(rows.contains("count: 1"), "flush committed before paint:\n{rows}");

    app.stop();
}

/// REGRESSION: the toggle's press path routes through the backend's
/// global-self handle into the author `on_change` — which the caps
/// layer wrapped, so a toggle press also commits via the deduped flush
/// (glyph flips by the next paint).
#[test]
fn newcore_toggle_press_flushes_and_flips_glyph() {
    let backend = fresh_backend();
    let app = backend_terminal::newcore::start(backend.clone(), |_| {}, || {
        use runtime_vocabulary::builders::{toggle, view};
        use runtime_world::signal;
        let on = signal(false);
        view()
            .child(toggle().value(on).on_change(move |v| on.set(v)))
            .build()
    });
    test_scheduler::drain();
    let grid = backend.borrow_mut().render_to_grid();
    assert!(
        !grid_rows(&grid).join("\n").contains('\u{25cf}'),
        "toggle starts off"
    );
    // The toggle paints "[ x ]" at its frame; click its center. Find
    // the bracket.
    let rows_vec = grid_rows(&grid);
    let row = rows_vec.iter().position(|r| r.contains('[')).expect("toggle painted") as u16;
    let col = rows_vec[row as usize].find('[').unwrap() as u16 + 1;
    let outcome = backend.borrow_mut().dispatch_click(col, row);
    if let ClickOutcome::HandlerFired(h) = outcome {
        h();
    } else {
        panic!("toggle click must fire its handler, got {outcome:?}");
    }
    test_scheduler::drain();
    let grid = backend.borrow_mut().render_to_grid();
    assert!(
        grid_rows(&grid).join("\n").contains('\u{25cf}'),
        "toggle glyph flipped after flush:\n{}",
        grid_rows(&grid).join("\n")
    );
    app.stop();
}

// ===========================================================================
// 3. Op-level caps adoption
// ===========================================================================

/// Host's structural seam must track the Backend splice contract (both
/// currently `false` — anchored reactive regions). If either half flips
/// independently, anchor placement diverges between cores.
#[test]
fn newcore_host_splice_matches_backend() {
    use runtime_core::Backend;
    use runtime_scene::Host;
    let b = TerminalBackend::new();
    assert_eq!(
        Host::supports_splice(&b),
        Backend::supports_child_splice(&b),
        "Host::supports_splice must delegate to the Backend contract"
    );
}

/// Boot bookkeeping: `is_booted` tracks start/stop, and `stop` tears
/// down the flush driver so a post-stop `schedule_flush` is a no-op
/// (no dangling world).
#[test]
fn newcore_stop_uninstalls_flush_driver() {
    let backend = fresh_backend();
    assert!(!backend_terminal::newcore::is_booted());
    let app = backend_terminal::newcore::start(backend.clone(), |_| {}, || {
        use runtime_vocabulary::builders::{text, view};
        view().child(text().content("x")).build()
    });
    assert!(backend_terminal::newcore::is_booted());
    app.stop();
    assert!(!backend_terminal::newcore::is_booted());
    // Safe after teardown: queues nothing that can touch a dead world.
    backend_terminal::newcore::schedule_flush();
    test_scheduler::drain();
}

/// Viewport forwarding: a post-boot `set_viewport` reaches the mounted
/// world's viewport signal (cells in, cells out) once the deduped flush
/// commits — the resize path's staged-write discipline.
#[test]
fn newcore_set_viewport_forwards_into_world() {
    let backend = fresh_backend();
    let seen: Rc<RefCell<Vec<(f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
    let seen_for_build = seen.clone();
    let app = backend_terminal::newcore::start(backend.clone(), |_| {}, move || {
        use runtime_vocabulary::builders::{text, view};
        let seen = seen_for_build.clone();
        let vp = runtime_vocabulary::viewport::viewport_ctx().size_signal();
        runtime_world::effect(move || {
            let s = vp.get();
            seen.borrow_mut().push((s.width, s.height));
        });
        view().child(text().content("x")).build()
    });
    test_scheduler::drain();
    assert_eq!(
        seen.borrow().last().copied(),
        Some((COLS as f32, ROWS as f32)),
        "boot seeds the world ctx from the pre-mount set_viewport"
    );
    backend.borrow_mut().set_viewport(60, 20);
    test_scheduler::drain();
    assert_eq!(
        seen.borrow().last().copied(),
        Some((60.0, 20.0)),
        "post-boot resize forwards through the viewport sink"
    );
    app.stop();
}
