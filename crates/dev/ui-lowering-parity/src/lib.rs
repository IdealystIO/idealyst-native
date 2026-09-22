//! `ui!` lowering-parity harness.
//!
//! [`record`] mounts one fixture against a [`host_mock::Harness`],
//! drives every signal the fixture exposes, and returns a
//! [`Recording`]: the per-step op log plus a final scene snapshot. Two
//! recordings of the same fixture — one per lowering — must be equal in
//! all three of the projections the suite asserts on:
//!
//! | projection                     | what it pins                        |
//! |--------------------------------|-------------------------------------|
//! | [`Recording::structural`]      | the 7-method `Host` seam + creation |
//! | [`Recording::scene`]           | the final tree (kinds + text)       |
//! | [`Recording::full`]            | every recorded capability call      |
//!
//! `structural` is the coarsest and the most stable (it is the same
//! projection `scene-parity`'s structural goldens pin); `full` adds
//! props, styles, text updates and handler installs, so it is the one
//! that catches "same shape, different style". `scene` is the
//! after-the-fact state, so it catches a divergence that happened to
//! produce the same op count.
//!
//! ## Why the fixtures live in this crate
//!
//! A fixture has to be authored ONCE and expanded TWICE, so the two
//! expansions provably come from the same tokens. That is a macro-level
//! constraint, which means the corpus is a `macro_rules!` in
//! [`fixtures`] rather than a data file.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use host_mock::{Harness, Node};
use runtime_scene::{realize, Element, Realized};

pub mod fixtures;

// ===========================================================================
// Recording
// ===========================================================================

/// Which structural strategy the backend advertises. Both are recorded
/// for every fixture: the anchored and spliced reactive-region paths
/// take different code through the scene drivers (`clear_children` +
/// `insert` vs `remove_child` + `insert_at`), so a lowering bug that
/// only shows up in one of them would otherwise hide. Same split
/// `scene-parity` makes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Anchored,
    Spliced,
}

impl Mode {
    pub const ALL: &'static [Mode] = &[Mode::Anchored, Mode::Spliced];

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

/// One driven step: its label plus every op the harness recorded while
/// it ran.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step {
    pub label: String,
    pub ops: Vec<String>,
}

/// The full record of one fixture run under one lowering.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Recording {
    /// `mount`, then one entry per drive, then `unmount`.
    pub steps: Vec<Step>,
    /// Indented snapshot of every root tree, taken after the last drive
    /// and before teardown.
    pub scene: String,
}

/// Op families that make up the STRUCTURAL projection: node creation
/// plus the 7 `runtime_scene::Host` methods. Everything else (styles,
/// text updates, handler installs, …) is `full`-only, matching
/// `scene-parity`'s split between its structural and full-op suites.
const STRUCTURAL_PREFIXES: &[&str] = &[
    "create ",
    "insert ",
    "insert_at ",
    "insert_many ",
    "remove_child ",
    "clear_children ",
];

impl Recording {
    /// Every recorded op, step by step.
    pub fn full(&self) -> String {
        serialize(&self.steps)
    }

    /// The structural projection: creation + the 7-method `Host` seam.
    pub fn structural(&self) -> String {
        let filtered: Vec<Step> = self
            .steps
            .iter()
            .map(|s| Step {
                label: s.label.clone(),
                ops: s
                    .ops
                    .iter()
                    .filter(|op| STRUCTURAL_PREFIXES.iter().any(|p| op.starts_with(p)))
                    .cloned()
                    .collect(),
            })
            .collect();
        serialize(&filtered)
    }

    /// The whole recording as one golden text: structural, then full,
    /// then the scene. One file per fixture keeps the reference
    /// self-contained.
    pub fn golden(&self) -> String {
        format!(
            "=== structural ==={}\n=== full ==={}\n=== scene ===\n{}\n",
            self.structural(),
            self.full(),
            self.scene
        )
    }
}

fn serialize(steps: &[Step]) -> String {
    let mut out = String::new();
    for step in steps {
        out.push_str(&format!("\n-- {} --\n", step.label));
        if step.ops.is_empty() {
            out.push_str("(none)\n");
        } else {
            for op in &step.ops {
                out.push_str(op);
                out.push('\n');
            }
        }
    }
    out
}

// ===========================================================================
// The runner
// ===========================================================================

/// Mount `build`'s tree in a fresh world, drive each entry of `drives`,
/// snapshot the scene, then unmount — recording every op along the way.
///
/// The world is entered ONCE around state construction, the build and
/// the realize, because the DIRECT lowering evaluates its hoisted slots
/// at build time: a signal read (or a `memo`) in a prop position has to
/// see the same ambient scope it would inside a real component.
pub fn record<St: 'static>(
    make: fn() -> St,
    build: fn(&St) -> Element,
    drives: &'static [(&'static str, fn(&St))],
    mode: Mode,
) -> Recording {
    let h = Harness::new();
    // Verbose tier: props, styles, handler installs and lifecycle calls
    // are exactly where a lowering divergence would hide.
    h.record_all();
    h.shared.splice.set(mode.splice());

    let mut steps: Vec<Step> = Vec::new();
    let state: Rc<RefCell<Option<St>>> = Rc::new(RefCell::new(None));

    let realized: Realized<Node> = h.world.enter(|| {
        let st = make();
        let element = build(&st);
        *state.borrow_mut() = Some(st);
        realize(&h.backend, &h.registry, element)
    });
    h.flush();
    steps.push(Step { label: "mount".into(), ops: h.take_log() });

    for (label, drive) in drives {
        h.world.enter(|| {
            let borrowed = state.borrow();
            drive(borrowed.as_ref().expect("state built"));
        });
        h.flush();
        steps.push(Step { label: (*label).to_string(), ops: h.take_log() });
    }

    let scene = scene_snapshot(&h);

    // Teardown is recorded: a lowering that leaks (or over-owns) a scope
    // shows up as a different unmount sequence.
    drop(realized);
    drop(state);
    steps.push(Step { label: "unmount".into(), ops: h.take_log() });

    Recording { steps, scene }
}

/// Every root tree in creation order. Roots are the nodes the harness
/// minted that nothing ever inserted into — derived rather than assumed
/// to be `n0`, because a tree whose root is a reactive region mints its
/// anchor first.
fn scene_snapshot(h: &Harness) -> String {
    let kinds = h.shared.kinds.borrow();
    let parents = h.shared.parent.borrow();
    let mut roots: Vec<Node> = kinds.keys().copied().filter(|n| !parents.contains_key(n)).collect();
    roots.sort_unstable();
    drop(kinds);
    drop(parents);
    if roots.is_empty() {
        return "(empty)".to_string();
    }
    roots.iter().map(|r| h.tree(*r)).collect::<Vec<_>>().join("\n")
}

// ===========================================================================
// Fixture registry
// ===========================================================================

/// One corpus entry: a name plus a recorder per lowering.
pub struct Fixture {
    pub name: &'static str,
    /// Record the fixture's mounted scene.
    pub direct: fn(Mode) -> Recording,
}

// ===========================================================================
// Goldens
// ===========================================================================

fn goldens_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens")
}

/// Absolute path of a fixture's golden file.
pub fn golden_path(name: &str, mode: Mode) -> PathBuf {
    goldens_dir().join(format!("{name}.{}.golden", mode.suffix()))
}

/// Compare `recording` against the fixture's golden, or write it when
/// `UPDATE_UI_PARITY_GOLDENS=1`.
///
/// The goldens are the pre-slot-rewrite DIRECT emission. They are the
/// reference the slot-list rewrite is measured against, so re-baselining
/// them discards that reference — do it only when a divergence has been
/// reviewed and accepted (and record the reason in the README's
/// divergence list).
pub fn check_golden(name: &str, mode: Mode, recording: &Recording) {
    let path = golden_path(name, mode);
    let actual = recording.golden();
    if std::env::var("UPDATE_UI_PARITY_GOLDENS").as_deref() == Ok("1") {
        std::fs::create_dir_all(goldens_dir()).expect("create goldens dir");
        std::fs::write(&path, &actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden {}: {e}\nrun with UPDATE_UI_PARITY_GOLDENS=1 to create it",
            path.display()
        )
    });
    if expected != actual {
        panic!(
            "golden mismatch for `{name}` [{mode:?}] ({})\n\n--- expected ---\n{expected}\n\
             --- actual ---\n{actual}",
            path.display()
        );
    }
}

/// Assert two recordings are identical, reporting the coarsest
/// differing projection first (structural → full → scene), because that
/// is the order in which a divergence is diagnosable.
pub fn assert_parity(name: &str, a: &Recording, b: &Recording) {
    if a.structural() != b.structural() {
        panic!(
            "`{name}`: structural op sequences differ\n\n--- a ---\n{}\n--- b ---\n{}",
            a.structural(),
            b.structural()
        );
    }
    if a.full() != b.full() {
        panic!(
            "`{name}`: full op streams differ\n\n--- a ---\n{}\n--- b ---\n{}",
            a.full(),
            b.full()
        );
    }
    if a.scene != b.scene {
        panic!(
            "`{name}`: final scenes differ\n\n--- a ---\n{}\n--- b ---\n{}",
            a.scene, b.scene
        );
    }
}
