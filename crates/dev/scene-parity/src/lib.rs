//! # scene-parity — structural-op golden-record harness (P1 exit gate)
//!
//! Pins the EXACT sequence of **structural** backend operations the
//! current walker (`runtime-core/src/walker/*`) emits for a fixed set of
//! reactive scenarios. The idea-lite scene core (`runtime-scene`, phase
//! P1 of the migration plan) must reproduce these sequences on the same
//! scenarios — see `README.md` for the contract and the places where a
//! deliberate divergence is allowed.
//!
//! Three pieces:
//!
//! - [`ParityBackend`] — a `runtime_core::Backend` that records ONLY the
//!   7 structural ops (`insert`, `insert_many`, `insert_at`,
//!   `remove_child`, `clear_children`, `create_reactive_anchor`, node
//!   creation) into an ordered, human-readable log. Prop/style updates
//!   are deliberately invisible: structure parity is the gate, and
//!   ignoring the rest keeps the goldens stable across style/feature
//!   churn in the old core. Node names are `n0, n1, …` in creation
//!   order — deterministic because the walker is single-threaded and
//!   synchronous.
//! - [`Scenario`] + [`Cx`] — a scenario builds a tree via runtime-core's
//!   PUBLIC element constructors, then performs scripted signal
//!   mutations, snapshotting the op log after each step. Scenario bodies
//!   are intentionally dumb (build → mutate → snapshot) so they can be
//!   re-targeted at the new core by swapping `Cx::mount`'s internals.
//! - [`check`] — runs a scenario in a given [`Mode`] and diffs the
//!   serialized steps against `goldens/<name>.<mode>.golden`. Regenerate
//!   with `UPDATE_GOLDENS=1 cargo test -p scene-parity`.
//!
//! The FULL-OP suite (P2b exit gate) lives in [`full`]/[`full_new`] +
//! [`scenarios_full`]/[`scenarios_full_new`]: a recorder that logs the
//! COMPLETE backend-call stream (create/update/style/release calls, not
//! just structure), pinning the vocabulary handlers against the walker.
//! See README §"The FULL-OP suite".

use std::cell::RefCell;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{Backend, Element, Owner, StyleRules};

mod scenarios;
pub use scenarios::scenarios;

pub mod full;
pub mod full_new;
pub mod scenarios_full;
pub mod scenarios_full_new;

pub mod new_core;
pub mod scenarios_new;

// ===========================================================================
// Node + Recorder
// ===========================================================================

/// The recording backend's node: a creation-order index. `n0` is the
/// first node the walker asked the backend to create, `n1` the second, …
/// Deterministic across runs (single-threaded, synchronous walker).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PNode(pub u32);

impl fmt::Display for PNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "n{}", self.0)
    }
}

#[derive(Default)]
struct RecorderInner {
    next_id: u32,
    ops: Vec<String>,
}

/// Shared, ordered op log + monotonic node-id counter. Cloning is an
/// `Rc`-clone: the backend, the harness, and any scenario-registered
/// cleanup hooks all append to the same log.
#[derive(Clone, Default)]
pub struct Recorder {
    inner: Rc<RefCell<RecorderInner>>,
}

impl Recorder {
    pub(crate) fn mint(&self) -> PNode {
        let mut inner = self.inner.borrow_mut();
        let id = inner.next_id;
        inner.next_id += 1;
        PNode(id)
    }

    pub(crate) fn push(&self, op: String) {
        self.inner.borrow_mut().ops.push(op);
    }

    /// Scenario-visible marker: a scope cleanup fired. Scenarios register
    /// these via `runtime_core::on_cleanup` inside a branch/row scope so
    /// the goldens pin *dispose ordering* relative to the structural ops
    /// (e.g. "remove_child BEFORE the row's cleanup" in `each`'s unmount,
    /// "cleanup BEFORE clear_children" in the anchored `when`).
    pub fn note_cleanup(&self, label: &str) {
        self.push(format!("cleanup {label}"));
    }

    /// Drain the ops recorded since the last call.
    pub fn take_ops(&self) -> Vec<String> {
        std::mem::take(&mut self.inner.borrow_mut().ops)
    }
}

// ===========================================================================
// ParityBackend
// ===========================================================================

/// A `Backend` that records structural ops only. Everything the trait
/// defaults cover is left at the default; the required non-structural
/// methods (`update_text`, `apply_style`, `finish`) are explicit no-ops
/// so prop/style churn never leaks into the goldens.
pub struct ParityBackend {
    rec: Recorder,
    splice: bool,
}

impl Backend for ParityBackend {
    type Node = PNode;

    // --- Node creation (recorded with kind + creation-order name) ---

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!("create {n} view"));
        n
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!("create {n} text {content:?}"));
        n
    }

    fn create_button(
        &mut self,
        label: &str,
        _on_click: &runtime_core::Action,
        _leading_icon: Option<&runtime_core::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_core::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!("create {n} button {label:?}"));
        n
    }

    /// Overridden (the trait default delegates to `create_view`) so
    /// anchors are distinguishable from real views in the goldens. The
    /// new core's `Host::create_anchor` maps here.
    fn create_reactive_anchor(&mut self) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!("create {n} anchor"));
        n
    }

    // --- The structural mutation set (the future Host trait) ---

    fn insert(&mut self, parent: &mut PNode, child: PNode) {
        self.rec.push(format!("insert {parent} <- {child}"));
    }

    fn insert_many(&mut self, parent: &mut PNode, children: Vec<PNode>) {
        let list = children
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        self.rec.push(format!("insert_many {parent} <- [{list}]"));
    }

    fn insert_at(&mut self, parent: &mut PNode, child: PNode, index: usize) {
        self.rec.push(format!("insert_at {parent} <- {child} @ {index}"));
    }

    fn remove_child(&mut self, parent: &PNode, child: &PNode) {
        self.rec.push(format!("remove_child {parent} -x {child}"));
    }

    fn clear_children(&mut self, node: &PNode) {
        self.rec.push(format!("clear_children {node}"));
    }

    fn supports_child_splice(&self) -> bool {
        self.splice
    }

    // --- Required but non-structural: explicit no-ops ---

    fn update_text(&mut self, _node: &PNode, _content: &str) {}
    fn apply_style(&mut self, _node: &PNode, _style: &Rc<StyleRules>) {}
    fn finish(&mut self, _root: PNode) {}
}

// ===========================================================================
// Mode
// ===========================================================================

/// Which structural strategy the backend reports.
///
/// - `Anchored` — `supports_child_splice() == false`: reactive regions
///   nest under a `create_reactive_anchor` wrapper and swap subtrees via
///   `clear_children` + `insert` (SSR + web-hydration + legacy native).
/// - `Spliced` — `supports_child_splice() == true`: style-less reactive
///   regions splice directly into the real parent via `remove_child` +
///   `insert_at(base_index)` (web CSR + splice-capable native).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Anchored,
    Spliced,
}

impl Mode {
    pub fn suffix(self) -> &'static str {
        match self {
            Mode::Anchored => "anchored",
            Mode::Spliced => "spliced",
        }
    }

    fn splice(self) -> bool {
        matches!(self, Mode::Spliced)
    }
}

// ===========================================================================
// Scenario + Cx
// ===========================================================================

/// One recorded step: the label plus every structural op it produced.
pub struct Step {
    pub label: String,
    pub ops: Vec<String>,
}

/// A parity scenario: name (drives the golden filename), a header
/// explaining exactly what behavior the golden pins (written into the
/// golden file), the modes it runs in, and the body.
pub struct Scenario {
    pub name: &'static str,
    /// Header lines written into the golden file — what behavior this
    /// golden pins and why the new core must reproduce it.
    pub about: &'static [&'static str],
    pub modes: &'static [Mode],
    pub run: fn(&mut Cx),
}

/// The scenario driver handed to each scenario body. Keep usage to
/// `mount` + `step` (+ `recorder` for cleanup markers) so scenario
/// bodies stay trivially re-targetable at the new core.
pub struct Cx {
    rec: Recorder,
    backend: Rc<RefCell<ParityBackend>>,
    owner: Option<Owner>,
    steps: Vec<Step>,
}

impl Cx {
    /// Clone of the shared recorder — for `on_cleanup` markers.
    pub fn recorder(&self) -> Recorder {
        self.rec.clone()
    }

    /// Render the tree and snapshot the mount ops as the `mount` step.
    pub fn mount(&mut self, root: Element) {
        assert!(self.owner.is_none(), "scenario mounted twice");
        let owner = runtime_core::render(self.backend.clone(), root);
        self.owner = Some(owner);
        self.snap("mount");
    }

    /// Run a mutation and snapshot the structural ops it produced.
    /// The old core is synchronous (and `schedule_microtask` falls back
    /// to synchronous execution off-web with no scheduler installed), so
    /// every op lands inside `f` — no flush call needed here. When these
    /// scenarios are re-targeted at the staged-commit core, `step`
    /// gains a `world.flush()` after `f`; the scenario bodies themselves
    /// don't change.
    pub fn step(&mut self, label: &str, f: impl FnOnce()) {
        f();
        self.snap(label);
    }

    fn snap(&mut self, label: &str) {
        self.steps.push(Step {
            label: label.to_string(),
            ops: self.rec.take_ops(),
        });
    }
}

// ===========================================================================
// Runner + golden comparison
// ===========================================================================

/// Run `scenario` in `mode`, returning the serialized golden text.
pub fn run_scenario(scenario: &Scenario, mode: Mode) -> String {
    let rec = Recorder::default();
    let backend = Rc::new(RefCell::new(ParityBackend {
        rec: rec.clone(),
        splice: mode.splice(),
    }));
    let mut cx = Cx {
        rec,
        backend,
        owner: None,
        steps: Vec::new(),
    };
    (scenario.run)(&mut cx);
    let steps = std::mem::take(&mut cx.steps);
    // Drop the Owner (unmount) AFTER taking the steps — teardown ops from
    // the final drop are outside the pinned sequence by design (the new
    // core's drop-as-teardown is expected to differ mechanically there).
    drop(cx);
    serialize(scenario, mode, &steps)
}

fn serialize(scenario: &Scenario, mode: Mode, steps: &[Step]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# scene-parity golden — scenario `{}`, mode `{}`\n#\n",
        scenario.name,
        mode.suffix()
    ));
    for line in scenario.about {
        out.push_str(&format!("# {line}\n"));
    }
    out.push_str(
        "#\n\
         # Structural ops only. Node names are creation-order (n0, n1, ...);\n\
         # `cleanup <label>` lines are scenario-registered on_cleanup markers\n\
         # pinning dispose ordering. Regenerate after an INTENDED behavior\n\
         # change with: UPDATE_GOLDENS=1 cargo test -p scene-parity\n",
    );
    out.push_str(&serialize_steps(steps));
    out
}

/// The header-less step body of a golden — shared by the old-core golden
/// writer and the new-core comparison (which strips headers on both sides
/// before diffing; see `new_core`).
pub fn serialize_steps(steps: &[Step]) -> String {
    let mut out = String::new();
    for step in steps {
        out.push_str(&format!("\n== {} ==\n", step.label));
        if step.ops.is_empty() {
            out.push_str("(no structural ops)\n");
        } else {
            for op in &step.ops {
                out.push_str(op);
                out.push('\n');
            }
        }
    }
    out
}

/// Absolute path of a scenario's golden file.
pub fn golden_path(name: &str, mode: Mode) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("goldens")
        .join(format!("{name}.{}.golden", mode.suffix()))
}

/// Test entry point: run the named scenario in `mode` and assert the
/// output matches its checked-in golden. With `UPDATE_GOLDENS=1` in the
/// environment, (re)writes the golden instead and passes.
pub fn check(name: &str, mode: Mode) {
    let all = scenarios();
    let scenario = all
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no scenario named `{name}` in the registry"));
    assert!(
        scenario.modes.contains(&mode),
        "scenario `{name}` is not registered for mode {mode:?}",
    );

    let actual = run_scenario(scenario, mode);
    let path = golden_path(name, mode);

    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("create goldens dir");
        std::fs::write(&path, &actual).expect("write golden");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden file {}\nGenerate it with: UPDATE_GOLDENS=1 cargo test -p scene-parity",
            path.display()
        )
    });

    if expected != actual {
        panic!(
            "structural-op golden mismatch for scenario `{name}` (mode {mode:?}).\n\
             If the walker's behavior changed INTENTIONALLY, regenerate with\n\
             UPDATE_GOLDENS=1 cargo test -p scene-parity and review the diff.\n\
             \n--- expected ({path}) ---\n{expected}\n--- actual ---\n{actual}",
            path = path.display(),
        );
    }
}
