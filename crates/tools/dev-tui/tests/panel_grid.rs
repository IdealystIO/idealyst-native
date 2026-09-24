//! The panel as the terminal draws it: a scripted session rendered
//! through the framework's terminal backend at 100x30, each state frozen
//! as a golden grid, and the panel's keys driven through the same hook
//! the terminal host calls.
//!
//! The events carry fixed session times and the panel is pumped with a
//! fixed clock, so every grid is deterministic. Goldens live in
//! `tests/goldens/`; re-baseline deliberately with
//! `IDEALYST_FREEZE_GOLDENS=1 cargo test -p dev-tui` and review the diff
//! as the substance of the change.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use backend_terminal::{Grid, TerminalBackend};
use dev_events::{
    BuildCause, BuildOutcome, Decision, DevEvent, Diagnostic, Envelope, HotTier, Mode, PageAck,
    Queue, ServerKind, Sink, SCHEMA_VERSION,
};
use dev_tui::{Controller, PanelKey, RunOptions};
use host_terminal::{KeyCode, KeyEvent, KeyModifiers};

const COLS: u16 = 100;
const ROWS: u16 = 30;

mod test_scheduler {
    //! Queue-only scheduler: host-terminal's microtask semantics
    //! (drain-until-empty), no clock — the backend's parity tests use the
    //! same shape.
    use runtime_shared::scheduling::{ScheduleHandle, Scheduler};
    use std::cell::RefCell;
    use std::collections::VecDeque;

    thread_local! {
        static QUEUE: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> = RefCell::new(VecDeque::new());
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
        fn after_ms(&self, _ms: i32, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
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

    pub fn drain() {
        while let Some(task) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
            task();
        }
    }
}

/// A mounted panel and the session feeding it.
struct Harness {
    backend: Rc<RefCell<TerminalBackend>>,
    app: Option<backend_terminal::newcore::NewCoreApp>,
    controller: Rc<Controller>,
    events: Queue,
    seq: u64,
}

impl Harness {
    fn new(rebuilds: Arc<AtomicUsize>) -> Self {
        test_scheduler::ensure_installed();
        let backend = Rc::new(RefCell::new(TerminalBackend::new()));
        backend_terminal::install_global_self(Rc::downgrade(&backend));
        backend.borrow_mut().set_viewport(COLS, ROWS);
        let events = Queue::new();
        let slot: Rc<RefCell<Option<Rc<Controller>>>> = Rc::new(RefCell::new(None));
        let (for_build, queue) = (slot.clone(), events.clone());
        let app = backend_terminal::newcore::start(backend.clone(), |_| {}, move || {
            dev_tui::install_theme();
            let controller = Controller::new(
                queue,
                RunOptions {
                    targets: vec!["web".into()],
                    on_rebuild: Some(Arc::new(move || {
                        rebuilds.fetch_add(1, Ordering::SeqCst);
                    })),
                },
            );
            *for_build.borrow_mut() = Some(controller.clone());
            controller.element(false)
        });
        let controller = slot.borrow().clone().expect("mounted");
        Harness { backend, app: Some(app), controller, events, seq: 0 }
    }

    /// Emit `event` at session time `at_ms`.
    fn at(&mut self, at_ms: u64, event: DevEvent) -> &mut Self {
        self.seq += 1;
        self.events.emit(&Envelope { v: SCHEMA_VERSION, seq: self.seq, at_ms, event });
        self
    }

    /// Press a key the way the terminal host delivers it. Returns whether
    /// the panel took it (the host quits on the ones it does not).
    fn press(&self, c: char) -> bool {
        (self.controller.key_handler())(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    /// One frame at `now_ms`, then the grid, one string per row.
    fn frame(&self, now_ms: u64) -> Vec<String> {
        rows(&self.grid(now_ms))
    }

    /// One frame at `now_ms`, then the grid itself (for colours).
    fn grid(&self, now_ms: u64) -> Grid {
        let app = self.app.as_ref().unwrap();
        app.world().enter(|| self.controller.pump_at(now_ms, COLS as usize, ROWS as usize));
        backend_terminal::newcore::flush_sync();
        test_scheduler::drain();
        self.backend.borrow_mut().render_to_grid()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(app) = self.app.take() {
            app.stop();
        }
    }
}

fn rows(grid: &Grid) -> Vec<String> {
    (0..grid.rows)
        .map(|r| {
            let line: String = (0..grid.cols)
                .map(|c| {
                    let g = grid.cell(c, r).map(|cell| cell.glyph).unwrap_or(' ');
                    if g.is_control() || g == '\0' { ' ' } else { g }
                })
                .collect();
            line.trim_end().to_string()
        })
        .collect()
}

/// The foreground colour of the first cell showing `glyph`.
fn fg_of(grid: &Grid, glyph: char) -> Option<(u8, u8, u8)> {
    for r in 0..grid.rows {
        for c in 0..grid.cols {
            let cell = grid.cell(c, r)?;
            if cell.glyph == glyph {
                return cell.fg.map(|f| (f.r, f.g, f.b));
            }
        }
    }
    None
}

fn golden(name: &str, grid: &[String]) {
    let text = grid.join("\n") + "\n";
    let goldens = parity_goldens::Goldens::new(env!("CARGO_MANIFEST_DIR"));
    goldens.freeze_text(name, &text);
    goldens.check_text(name, &text);
}

fn web() -> String {
    "web".into()
}

fn diag(line: u32) -> DevEvent {
    DevEvent::Diagnostic {
        target: web(),
        diagnostic: Diagnostic {
            level: "error".into(),
            message: "mismatched types".into(),
            code: Some("E0308".into()),
            file: Some("src/app.rs".into()),
            line: Some(line),
            column: Some(25),
            package: None,
            rendered: format!(
                "error[E0308]: mismatched types\n  --> src/app.rs:{line}:25\n   |\n{line} |     let broken: u32 = \"not a number\";\n   |                 ---   ^^^^^^^^^^^^^^ expected `u32`, found `&str`\n   |                 |\n   |                 expected due to this\n"
            ),
            ansi: None,
        },
    }
}

/// The scripted session: start, a cold build, an overlay save, a hot
/// patch, a rebuild in flight, a failed build.
#[test]
fn the_panel_through_a_session() {
    let rebuilds = Arc::new(AtomicUsize::new(0));
    let mut h = Harness::new(rebuilds.clone());

    h.at(0, DevEvent::SessionStarted {
        app: "Hotreload Lab".into(),
        targets: vec![web()],
        mode: Mode::Local,
        hot_tier: HotTier::Armed,
        log_file: Some("/work/target/idealyst/hotreload-lab/dev.log".into()),
        server: None,
    })
    .at(30, DevEvent::ServerReady {
        target: "session".into(),
        kind: ServerKind::Events,
        url: "http://127.0.0.1:54151/__idealyst/events".into(),
    })
    .at(40, DevEvent::BuildStarted { target: web(), cause: BuildCause::Initial })
    .at(41, DevEvent::Log { source: "build-web".into(), line: "cargo build --target wasm32-unknown-unknown".into() })
    .at(55_500, DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Ready { gen: 1 }, ms: 55_460 })
    .at(55_600, DevEvent::ServerReady {
        target: web(),
        kind: ServerKind::Livereload,
        url: "http://0.0.0.0:8080".into(),
    })
    .at(55_700, DevEvent::Watching { target: web(), roots: vec!["/work/src".into()], rewatch: false });
    golden("panel_idle.txt", &h.frame(56_000));

    // An overlay save, acked by the page.
    h.at(90_000, DevEvent::ChangeDetected {
        target: web(),
        paths: vec!["/work/src/app.rs".into()],
        crates: vec!["hotreload-lab".into()],
        folded: 0,
    })
    .at(90_005, DevEvent::Decided { target: web(), decision: Decision::Overlay { sites: 1 } })
    .at(90_012, DevEvent::OverlayPushed { target: web(), sites: 1, ms: 12 })
    .at(90_030, DevEvent::PageAck { target: web(), ack: PageAck::Overlay { applied: Some(1), refused: Some(0) } });
    golden("panel_after_overlay.txt", &h.frame(90_100));

    // A hot patch, acked.
    h.at(120_000, DevEvent::ChangeDetected {
        target: web(),
        paths: vec!["/work/src/app.rs".into()],
        crates: vec!["hotreload-lab".into()],
        folded: 0,
    })
    .at(120_004, DevEvent::Decided {
        target: web(),
        decision: Decision::HotPatch { crates: vec!["hotreload_lab".into()], files: vec!["src/app.rs".into()] },
    })
    .at(120_446, DevEvent::PatchBuilt {
        target: web(),
        files: vec!["src/app.rs".into()],
        crates: vec![],
        redirected: 64,
        steps: vec![],
        skipped: vec![],
        bytes: 18_433,
        ms: 446,
    })
    .at(120_520, DevEvent::PageAck { target: web(), ack: PageAck::HotPatch { redirected: Some(64), carried: Some(12) } });
    golden("panel_after_hot_patch.txt", &h.frame(120_600));

    // A shape edit: a rebuild in flight, cargo part-way.
    h.at(150_000, DevEvent::ChangeDetected {
        target: web(),
        paths: vec!["/work/src/app.rs".into()],
        crates: vec!["hotreload-lab".into()],
        folded: 0,
    })
    .at(150_004, DevEvent::Decided {
        target: web(),
        decision: Decision::Rebuild { reason: Some("src/app.rs changed outside its function bodies".into()) },
    })
    .at(150_010, DevEvent::BuildStarted { target: web(), cause: BuildCause::Save { folded: 0 } })
    .at(150_012, DevEvent::StageStarted { target: web(), stage: "cargo".into() })
    .at(158_000, DevEvent::CargoProgress { target: web(), compiled: 212, total: Some(480), current: Some("idea-ui".into()) });
    golden("panel_mid_build.txt", &h.frame(162_310));
    // Colour carries the state: a busy row is amber.
    assert_eq!(fg_of(&h.grid(162_310), '━'), Some((0xe5, 0xb4, 0x43)));

    // It fails: the first error, collapsed, then expanded with `e`.
    h.at(165_000, diag(118))
        .at(165_300, DevEvent::Output {
            source: "cargo".into(),
            line: "error: could not compile `hotreload-lab` (lib) due to 1 previous error".into(),
            target: Some(web()),
        })
        .at(165_400, DevEvent::BuildFinished {
            target: web(),
            outcome: BuildOutcome::Failed { error: "cargo exited with exit status: 101".into() },
            ms: 15_390,
        });
    let collapsed = h.frame(165_500);
    assert!(collapsed.iter().any(|r| r.contains("✗ src/app.rs:118:25  mismatched types  e: expand")), "{collapsed:#?}");
    assert_eq!(fg_of(&h.grid(165_500), '✗'), Some((0xe5, 0x67, 0x6b)), "a failure is red");
    assert!(h.press('e'), "the panel takes `e`");
    golden("panel_error_expanded.txt", &h.frame(165_600));

    // `l` opens the log pane; `e` again collapses the error.
    assert!(h.press('l'));
    assert!(h.press('e'));
    golden("panel_log_open.txt", &h.frame(165_700));

    // `r` asks for a rebuild; `c` clears the history, the log and the
    // error; `q` is not the panel's — the host quits on it.
    assert!(h.press('r'));
    assert!(h.press('c'));
    let cleared = h.frame(165_800);
    assert_eq!(rebuilds.load(Ordering::SeqCst), 1, "`r` reached the session's rebuild hook");
    assert!(cleared.iter().any(|r| r.trim() == "no saves yet"), "{cleared:#?}");
    assert!(!cleared.iter().any(|r| r.contains("mismatched types  e: expand")), "{cleared:#?}");
    assert!(!h.press('q'), "q is the host's quit key");
    assert_eq!(
        PanelKey::of(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        None,
        "Ctrl-C quits; it is not `c`"
    );
    assert_eq!(cleared.last().map(String::as_str), Some("r rebuild · l log · e error · c clear · q quit"));
}

/// The spinner is the only thing that moves without an event: one glyph
/// per frame on a busy row, and nothing at all on an idle one.
#[test]
fn only_a_busy_row_changes_between_frames() {
    let mut h = Harness::new(Arc::new(AtomicUsize::new(0)));
    h.at(0, DevEvent::SessionStarted {
        app: "Lab".into(),
        targets: vec![web()],
        mode: Mode::Local,
        hot_tier: HotTier::Armed,
        log_file: None,
        server: None,
    })
    .at(10, DevEvent::Watching { target: web(), roots: vec![], rewatch: false });
    let a = h.frame(1_000);
    let b = h.frame(1_000);
    assert_eq!(a, b, "an idle panel is still");

    h.at(2_000, DevEvent::BuildStarted { target: web(), cause: BuildCause::Forced });
    let c = h.frame(2_000);
    let d = h.frame(2_000);
    let row = |g: &[String]| g.iter().find(|r| r.contains("rebuilding")).cloned().unwrap();
    assert_ne!(row(&c), row(&d), "the spinner advances a glyph per frame");
    assert_eq!(
        c.iter().filter(|r| !r.contains("rebuilding")).collect::<Vec<_>>(),
        d.iter().filter(|r| !r.contains("rebuilding")).collect::<Vec<_>>(),
        "and nothing else moves"
    );
}

fn server() -> String {
    dev_events::SERVER_TARGET.into()
}

fn server_out(line: &str) -> DevEvent {
    DevEvent::Output {
        source: dev_events::SERVER_OUTPUT_SOURCE.into(),
        line: line.into(),
        target: Some(server()),
    }
}

/// A full-stack session (CrewForge's shape): the web bundle and the
/// project's own server as two rows, built at once, then a web-only hot
/// patch, then a save in the server's sources — and the log pane's `l`
/// stepping from the dev loop's lines to the server's output.
#[test]
fn a_full_stack_session_shows_the_server_as_its_own_row() {
    let mut h = Harness::new(Arc::new(AtomicUsize::new(0)));
    h.at(0, DevEvent::SessionStarted {
        app: "CrewForge".into(),
        targets: vec![web()],
        mode: Mode::Local,
        hot_tier: HotTier::Armed,
        log_file: None,
        server: Some(
            dev_events::SessionServer::named("crewforge-server")
                .with_log_file("/cf/crates/app-main/target/idealyst/crewforge-main/server.log"),
        ),
    });
    // The first frame, before either build has said a word: both rows.
    let first = h.frame(0);
    assert!(first.iter().any(|r| r.trim_start().starts_with("crewforge-server") && r.contains("○ queued")), "{first:#?}");

    // Both building at once, each with its own progress.
    h.at(20, DevEvent::BuildStarted { target: web(), cause: BuildCause::Initial })
        .at(25, DevEvent::BuildStarted { target: server(), cause: BuildCause::Initial })
        .at(30, DevEvent::StageStarted { target: web(), stage: "cargo".into() })
        .at(31, DevEvent::StageStarted { target: server(), stage: "cargo".into() })
        .at(21_000, DevEvent::CargoProgress { target: web(), compiled: 212, total: Some(480), current: Some("idea-ui".into()) })
        .at(21_500, DevEvent::CargoProgress { target: server(), compiled: 301, total: Some(512), current: Some("sqlx-postgres".into()) });
    golden("panel_full_stack_both_building.txt", &h.frame(22_000));

    // Both done: the server runs on its URL, the web bundle is ready.
    h.at(46_200, DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Ready { gen: 1 }, ms: 46_180 })
        .at(61_000, DevEvent::BuildFinished { target: server(), outcome: BuildOutcome::Ready { gen: 1 }, ms: 60_975 })
        .at(61_300, server_out("crewforge-server listening on http://127.0.0.1:3100"))
        .at(61_400, DevEvent::ServerReady { target: web(), kind: ServerKind::FullStack, url: "http://127.0.0.1:3100".into() })
        .at(61_500, DevEvent::Watching { target: web(), roots: vec!["/cf/crates".into()], rewatch: false })
        .at(61_600, DevEvent::Watching { target: server(), roots: vec!["/cf/crates/api-server/src".into()], rewatch: false });
    golden("panel_full_stack_idle.txt", &h.frame(62_000));

    // A web-only hot patch: the server's row does not move.
    h.at(90_000, DevEvent::ChangeDetected { target: web(), paths: vec!["/cf/crates/app-main/src/screens/landing/mod.rs".into()], crates: vec![], folded: 0 })
        .at(90_004, DevEvent::Decided {
            target: web(),
            decision: Decision::HotPatch { crates: vec!["crewforge_main".into()], files: vec![] },
        })
        .at(99_300, DevEvent::PatchBuilt {
            target: web(),
            files: vec![],
            crates: vec![],
            redirected: 38,
            steps: vec![],
            skipped: vec![],
            bytes: 1,
            ms: 9_300,
        })
        .at(99_700, DevEvent::PageAck { target: web(), ack: PageAck::HotPatch { redirected: Some(38), carried: Some(4) } })
        .at(99_800, server_out("GET / 200 1.2ms"));
    golden("panel_full_stack_server_running_web_patched.txt", &h.frame(100_000));

    // A save in the server's own sources: the server rebuilds while the
    // web row stays as it was.
    h.at(120_000, DevEvent::ChangeDetected { target: server(), paths: vec!["/cf/crates/api-server/src/routes.rs".into()], crates: vec![], folded: 0 })
        .at(120_450, DevEvent::BuildStarted { target: server(), cause: BuildCause::Save { folded: 0 } })
        .at(120_451, DevEvent::StageStarted { target: server(), stage: "cargo".into() })
        .at(126_000, DevEvent::CargoProgress { target: server(), compiled: 509, total: Some(512), current: Some("crewforge-api-server".into()) })
        .at(126_100, DevEvent::Output { source: "cargo".into(), line: "   Compiling crewforge-api-server v0.1.0".into(), target: Some(server()) })
        .at(126_200, server_out("GET /api/me 200 3.4ms"));
    golden("panel_full_stack_server_rebuilding.txt", &h.frame(128_300));

    // `l` opens the dev loop's lines — no request lines — then the
    // server's output, then both.
    assert!(h.press('l'));
    let dev = h.frame(128_400);
    assert!(dev.iter().any(|r| r.contains("Compiling crewforge-api-server")), "{dev:#?}");
    assert!(!dev.iter().any(|r| r.contains("GET /")), "server chatter in the dev view: {dev:#?}");
    assert!(h.press('l'));
    golden("panel_full_stack_server_log.txt", &h.frame(128_500));
    assert!(h.press('l'));
    let all = h.frame(128_600);
    assert!(all.iter().any(|r| r.contains("GET /api/me")) && all.iter().any(|r| r.contains("Compiling crewforge-api-server")), "{all:#?}");
    assert!(h.press('l'));
    let closed = h.frame(128_700);
    assert!(!closed.iter().any(|r| r.contains("  log")), "{closed:#?}");
}
