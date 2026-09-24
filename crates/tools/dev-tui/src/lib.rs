//! The interactive panel for `idealyst dev` (`--interactive`, or
//! `IDEALYST_DEV_UI=panel`): a Metro-style live view of the session,
//! built as an idealyst app on the framework's own terminal backend.
//!
//! ```text
//! idealyst dev · Hotreload Lab · local · http://0.0.0.0:8080 · hot patch armed
//!
//!   web             ⠹ rebuilding · cargo 212/480 ━━━━━━━━──────────── idea-ui · 12.3s
//!
//!   saves
//!   00:42  app.rs                    hot patch     446 ms  3 fn redirected  ✓ page
//!   00:31  app.rs                    overlay        12 ms  1 site  ✓ page
//!
//! r rebuild · l log · e error · c clear · q quit
//! ```
//!
//! Three layers, each tested on its own:
//!
//! - [`model`]: a pure fold of the session's [`dev_events`] stream — one
//!   state machine per target, a ring of recent saves, the log, the last
//!   build error;
//! - [`view`]: the model laid out as lines for a given size, time and
//!   spinner frame;
//! - [`panel`]: `#[component]`s rendering those lines with `ui!` and
//!   `stylesheet!`.
//!
//! [`Controller`] ties them to the live session: once per frame it drains
//! the event [`Queue`] (fed from the dev loop's worker threads), applies
//! the keys pressed since the last frame, and writes the lines into the
//! panel's signals. The reactive arena is single-threaded, which is why
//! the workers hand events over through a queue instead of touching the
//! signals themselves.
//!
//! Minimal by intent (see the terminal backend's conventions): no
//! animation, no chrome beyond what carries information. The only thing
//! that changes without an event is a busy row's spinner glyph and its
//! elapsed time.

pub mod model;
pub mod panel;
pub mod view;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use dev_events::Queue;
use runtime_core::{signal, ui, Element, Signal};

use crate::model::Model;
use crate::panel::Panel;
use crate::view::{Line, Row, Toggles};

/// Options for [`run`].
#[derive(Clone)]
pub struct RunOptions {
    /// The session's targets: a row each from the start.
    pub targets: Vec<String>,
    /// What `r` does: ask the session's watchers to rebuild.
    pub on_rebuild: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// A key the panel acts on. `q`, Esc and Ctrl-C are the terminal host's:
/// they quit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKey {
    /// `r`: rebuild now.
    Rebuild,
    /// `l`: open the log pane, then (in a session with a server) step it
    /// from the dev loop's lines to the server's output to both, then
    /// close it.
    ToggleLog,
    /// `e`: expand or collapse the last build error.
    ToggleError,
    /// `c`: clear the history, the log and the error.
    Clear,
}

impl PanelKey {
    /// The panel's meaning of a key press, if it has one.
    pub fn of(key: &host_terminal::KeyEvent) -> Option<PanelKey> {
        use host_terminal::{KeyCode, KeyModifiers};
        if key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return None;
        }
        match key.code {
            KeyCode::Char('r') | KeyCode::Char('R') => Some(PanelKey::Rebuild),
            KeyCode::Char('l') | KeyCode::Char('L') => Some(PanelKey::ToggleLog),
            KeyCode::Char('e') | KeyCode::Char('E') => Some(PanelKey::ToggleError),
            KeyCode::Char('c') | KeyCode::Char('C') => Some(PanelKey::Clear),
            _ => None,
        }
    }
}

/// The panel's signals: one per block of the screen.
#[derive(Clone, Copy)]
struct Signals {
    header: Signal<Line>,
    rows: Signal<Vec<Row>>,
    history: Signal<Vec<Line>>,
    error: Signal<Vec<Line>>,
    log: Signal<Vec<Line>>,
    footer: Signal<Line>,
}

/// Drives the panel from the session: see the crate docs.
pub struct Controller {
    events: Queue,
    on_rebuild: Option<Arc<dyn Fn() + Send + Sync>>,
    model: RefCell<Model>,
    toggles: Cell<Toggles>,
    keys: RefCell<VecDeque<PanelKey>>,
    frame: Cell<u64>,
    /// The latest event's session time, and when it was drained: the
    /// session clock between events, for elapsed times.
    clock: Cell<(u64, Instant)>,
    signals: Signals,
}

impl Controller {
    /// Must run inside the panel's world: it creates the signals.
    pub fn new(events: Queue, opts: RunOptions) -> Rc<Self> {
        Rc::new(Self {
            events,
            on_rebuild: opts.on_rebuild,
            model: RefCell::new(Model::new(&opts.targets)),
            toggles: Cell::new(Toggles::default()),
            keys: RefCell::new(VecDeque::new()),
            frame: Cell::new(0),
            clock: Cell::new((0, Instant::now())),
            signals: Signals {
                header: signal(Line::default()),
                rows: signal(Vec::new()),
                history: signal(Vec::new()),
                error: signal(Vec::new()),
                log: signal(Vec::new()),
                footer: signal(Line::default()),
            },
        })
    }

    /// Queue a key for the next frame.
    pub fn key(&self, key: PanelKey) {
        self.keys.borrow_mut().push_back(key);
    }

    /// The terminal host's key hook for this controller: the panel's keys
    /// are queued and consumed (`true`); anything else — the quit keys —
    /// goes on to the host (`false`).
    pub fn key_handler(self: &Rc<Self>) -> Rc<dyn Fn(&host_terminal::KeyEvent) -> bool> {
        let this = self.clone();
        Rc::new(move |key| match PanelKey::of(key) {
            Some(k) => {
                this.key(k);
                true
            }
            None => false,
        })
    }

    /// One live frame: the session clock and the terminal's size.
    pub fn pump(&self) {
        let (at, drained) = self.clock.get();
        let now = at + drained.elapsed().as_millis() as u64;
        let size = runtime_core::viewport_size().peek();
        self.pump_at(now, size.width.max(1.0) as usize, size.height.max(1.0) as usize);
    }

    /// One frame at session time `now_ms` for a `width` x `height`
    /// terminal: drain events, apply keys, publish the screen. Tests call
    /// this directly, with a fixed clock.
    pub fn pump_at(&self, now_ms: u64, width: usize, height: usize) {
        let events = self.events.drain();
        {
            let mut model = self.model.borrow_mut();
            for e in &events {
                model.apply(e);
            }
        }
        if let Some(last) = events.last() {
            self.clock.set((last.at_ms, Instant::now()));
        }
        let keys: Vec<PanelKey> = self.keys.borrow_mut().drain(..).collect();
        for key in keys {
            let mut t = self.toggles.get();
            match key {
                PanelKey::Rebuild => {
                    if let Some(rebuild) = &self.on_rebuild {
                        rebuild();
                    }
                }
                PanelKey::ToggleLog => {
                    t.log = t.log.next(self.model.borrow().server.is_some())
                }
                PanelKey::ToggleError => t.expanded = !t.expanded,
                PanelKey::Clear => {
                    self.model.borrow_mut().clear();
                    t.expanded = false;
                }
            }
            self.toggles.set(t);
        }
        let frame = self.frame.get();
        self.frame.set(frame + 1);
        let model = self.model.borrow();
        let now_ms = now_ms.max(model.now_ms);
        let screen = view::screen(&model, self.toggles.get(), now_ms, frame, width, height);
        // Equality-guarded: an idle frame writes nothing and re-renders
        // nothing.
        let s = &self.signals;
        s.header.set(screen.header);
        s.rows.set(screen.rows);
        s.history.set(screen.history);
        s.error.set(screen.error);
        s.log.set(screen.log);
        s.footer.set(screen.footer);
    }

    /// The panel element. `live` runs [`Self::pump`] every frame; without
    /// it the caller pumps (tests).
    pub fn element(self: &Rc<Self>, live: bool) -> Element {
        let s = self.signals;
        let on_frame: Option<Rc<dyn Fn()>> = live.then(|| {
            let this = self.clone();
            Rc::new(move || this.pump()) as Rc<dyn Fn()>
        });
        ui! {
            Panel(
                header = s.header.read_only(),
                rows = s.rows.read_only(),
                history = s.history.read_only(),
                error = s.error.read_only(),
                log = s.log.read_only(),
                footer = s.footer.read_only(),
                on_frame = on_frame,
            )
        }
    }
}

/// Boot the panel on this terminal. Blocks until the user quits (q, Esc,
/// Ctrl-C).
///
/// The framework's terminal host owns stdio for the call: raw mode and
/// the alternate screen come up before mount and go down on return.
/// `events` is the session's event queue.
pub fn run(events: Queue, opts: RunOptions) -> Result<(), host_terminal::RunError> {
    // The controller is made inside the mount (its signals need the
    // world), but the host takes its key hook before it mounts; the hook
    // reaches the controller through this slot.
    let slot: Rc<RefCell<Option<Rc<Controller>>>> = Rc::new(RefCell::new(None));
    let for_keys = slot.clone();
    let on_key: Rc<dyn Fn(&host_terminal::KeyEvent) -> bool> =
        Rc::new(move |key| match for_keys.borrow().as_ref() {
            Some(c) => (c.key_handler())(key),
            None => false,
        });
    let host_opts = host_terminal::RunOptions {
        target_fps: 30,
        on_key: Some(on_key),
        // One layout pixel per cell: the panel is authored in cells.
        cell_size: None,
    };
    host_terminal::run(
        move || {
            install_theme();
            let controller = Controller::new(events, opts);
            *slot.borrow_mut() = Some(controller.clone());
            controller.element(true)
        },
        host_opts,
        // Builtins only: no SDK payloads to register.
        |_| {},
    )
}

/// The framework panics on first render without an installed theme (see
/// [[project_install_theme_required]]); the panel uses none of its
/// tokens, so the default light palette is as good as any. Public for
/// tests that mount the panel without [`run`].
pub fn install_theme() {
    // Idempotent: a later install replaces the active theme.
    idea_ui::install_idea_theme(idea_ui::light_theme());
}
