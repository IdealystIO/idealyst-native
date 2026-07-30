//! Render-parity tests for the terminal backend: the rule-7 gate (a
//! scene must paint the exact cells the OLD core painted) plus op-level
//! coverage of the caps adoption's flush discipline (input event →
//! staged writes → flush → paint).
//!
//! Harness notes: the tests install a queue-only scheduler (mirroring
//! `host-terminal`'s tick semantics for microtasks: drain-until-empty).
//! `install_scheduler` is process-global/first-wins, but the queue
//! state is thread-local, so each test thread drains only its own
//! tasks.

//! # The frozen corpus is the contract
//!
//! The grid-parity gates compare against a **frozen old-core grid dump**
//! committed under `tests/goldens/` (cell-exact: glyph + fg + bg,
//! run-length encoded per row), written by the old walker before it was
//! deleted. A mismatch is a real rendering change, NOT a stale artifact:
//! `IDEALYST_FREEZE_GOLDENS=1` can now only RE-BASELINE against the
//! current renderer, permanently discarding the old core's testimony —
//! see `tests/goldens/README.md`.

use std::cell::RefCell;
use std::rc::Rc;

use backend_terminal::{ClickOutcome, Grid, TerminalBackend};
use runtime_shared::{Color, Length, StyleRules, Tokenized};

// ===========================================================================
// Queue-only test scheduler (host-terminal's microtask semantics)
// ===========================================================================

mod test_scheduler {
    use runtime_shared::scheduling::{ScheduleHandle, Scheduler};
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
        if !runtime_shared::scheduling::is_scheduler_installed() {
            runtime_shared::scheduling::install_scheduler(Box::new(QueueScheduler));
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

// ---------------------------------------------------------------------------
// Frozen-artifact gate
// ---------------------------------------------------------------------------

fn goldens() -> parity_goldens::Goldens {
    parity_goldens::Goldens::new(env!("CARGO_MANIFEST_DIR"))
}

fn color_token(c: Option<runtime_shared::color::Rgba>) -> String {
    match c {
        None => "-".to_string(),
        Some(c) => format!("{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a),
    }
}

/// Run-length encode one row's color channel so the dump stays small and
/// reviewable while remaining lossless.
fn rle(tokens: impl Iterator<Item = String>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut cur: Option<(String, u32)> = None;
    for t in tokens {
        match &mut cur {
            Some((v, n)) if *v == t => *n += 1,
            _ => {
                if let Some((v, n)) = cur.take() {
                    out.push(if n == 1 { v } else { format!("{n}*{v}") });
                }
                cur = Some((t, 1));
            }
        }
    }
    if let Some((v, n)) = cur {
        out.push(if n == 1 { v } else { format!("{n}*{v}") });
    }
    out.join(" ")
}

/// Canonical, lossless serialization of a grid: size header, then per
/// row a glyph line (with a `|` fence so trailing spaces survive) and
/// run-length-encoded fg/bg lines. Cell-exact — this is the same
/// information `assert_grids_identical` compares.
fn grid_dump(grid: &Grid) -> String {
    let mut out = format!("cols={} rows={}\n", grid.cols, grid.rows);
    for r in 0..grid.rows {
        let mut glyphs = String::with_capacity(grid.cols as usize);
        for c in 0..grid.cols {
            let g = grid.cell(c, r).map(|cell| cell.glyph).unwrap_or(' ');
            glyphs.push(if g.is_control() || g == '\0' { ' ' } else { g });
        }
        out.push_str(&format!("r{r:02} glyph |{glyphs}|\n"));
        out.push_str(&format!(
            "r{r:02} fg    {}\n",
            rle((0..grid.cols).map(|c| color_token(grid.cell(c, r).and_then(|x| x.fg))))
        ));
        out.push_str(&format!(
            "r{r:02} bg    {}\n",
            rle((0..grid.cols).map(|c| color_token(grid.cell(c, r).and_then(|x| x.bg))))
        ));
    }
    out
}

/// The gate: the rendered grid must match the frozen old-core dump
/// cell-for-cell (glyph + fg + bg), with a readable rows dump on
/// divergence.
fn check_new_grid(name: &str, new: &Grid) {
    goldens().check_text(name, &grid_dump(new));
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

// ===========================================================================
// 1. Full-scene render snapshot vs the frozen old-core grid
// ===========================================================================

/// The rule-7 gate for this port: a torture scene (styled views, text,
/// button chrome `[ label ]`, toggle glyphs, pressable, colored
/// backgrounds) must paint the exact cells the OLD core painted —
/// glyphs AND colors.
#[test]
fn newcore_full_scene_grid_parity() {
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
    check_new_grid("full_scene.grid", &new);
    // Sanity: the scene is live (button chrome + toggle glyph painted).
    let rows = grid_rows(&new).join("\n");
    assert!(rows.contains("[ Go ]"), "button chrome painted:\n{rows}");
    assert!(rows.contains('\u{25cf}'), "toggle on-glyph painted:\n{rows}");
    assert!(rows.contains("hello terminal"), "text painted:\n{rows}");
}

// ===========================================================================
// 1b. Caps-breadth scene (guards the de-trait pass's default resolution)
// ===========================================================================

/// COVERAGE BREADTH: the leaf/container primitives the torture scene
/// above does not reach, in one grid — image, icon, `link` (a real
/// `NodeKind::Pressable` here, NOT the trait default), activity
/// indicator, controlled text input, a `scroll_view` clipping an
/// oversized child — plus `slider` and `text_area`, which this backend
/// does NOT implement and therefore resolve to **trait defaults**.
///
/// Why this scene exists: the deletion wave moved this backend's caps
/// impls off the `Backend` trait, so every default-resolved method
/// stopped resolving to a `Backend` default and now resolves to a
/// **caps** default. A frozen grid that actually paints these primitives
/// makes a silently-differing default fail loudly. See
/// `docs/runtime-v2-deletion-baseline.md` for this backend's
/// default-resolved list.
#[test]
fn newcore_caps_breadth_grid_parity() {
    fn icon_data() -> runtime_shared::primitives::icon::IconData {
        runtime_shared::primitives::icon::IconData {
            view_box: (24, 24),
            paths: &["M4 4 L20 20"],
            fill_rule: runtime_shared::FillRule::NonZero,
            filled: false,
        }
    }

    let new = render_new(|| {
        use runtime_vocabulary::builders::{
            activity_indicator, icon, image, link, scroll_view, slider, text, text_area,
            text_input, view,
        };
        use runtime_world::signal;
        let typed = signal(String::from("typed value"));
        let notes = signal(String::from("notes"));
        let amount = signal(0.25f32);
        view()
            .style(test_rules(44.0, "#101020"))
            .child(
                image()
                    .src("https://example.test/a.png")
                    .style(test_rules(10.0, "#223344")),
            )
            .child(icon().data(icon_data()).style(test_rules(6.0, "#443322")))
            .child(
                link()
                    .url("https://example.test/")
                    .external(true)
                    .child(text().content("link text")),
            )
            .child(activity_indicator())
            .child(text_input().value(typed))
            .child(text_area().value(notes))
            .child(slider().value(amount))
            .child(
                scroll_view()
                    .style(test_rules(12.0, "#112211"))
                    .child(text().content("clipped overflowing content")),
            )
            .build()
    });
    check_new_grid("caps_breadth.grid", &new);
    // Liveness: the grid actually painted something.
    let rows = grid_rows(&new).join("\n");
    assert!(
        rows.chars().any(|c| c != ' ' && c != '\n'),
        "caps-breadth scene painted nothing:\n{rows}"
    );
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

/// The structural seam must stay ANCHORED (`supports_splice == false`).
#[test]
fn newcore_host_splice_is_anchored() {
    use runtime_scene::Host;
    let b = TerminalBackend::new();
    // `supports_child_splice` used to be a `Backend` trait DEFAULT here
    // (this backend never overrode it); `Host` makes it REQUIRED, so
    // `newcore.rs` now carries an explicit body reproducing that default
    // — see docs/runtime-v2-deletion-baseline.md §2.2. Pin the value so a
    // silent flip to spliced (which would move reactive regions out from
    // under their anchor and change every frozen grid dump) fails here
    // rather than in a rendering diff.
    assert!(
        !Host::supports_splice(&b),
        "the terminal backend renders ANCHORED (the frozen grid dumps pin it)"
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
