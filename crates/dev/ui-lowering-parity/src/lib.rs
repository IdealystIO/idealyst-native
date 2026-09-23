//! `ui!` lowering-parity harness.
//!
//! [`record`] mounts one fixture against a [`host_mock::Harness`],
//! drives every signal the fixture exposes, and returns a
//! [`Recording`]: the per-step op log plus a final scene snapshot. A
//! recording must equal the fixture's frozen golden in all three of the
//! projections the suite asserts on:
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
//! A fixture is `ui!` tokens plus the signals that drive them, and the
//! overlay suites also need each fixture's SOURCE text (the build-time
//! descriptor is produced from source). One `macro_rules!` in
//! [`fixtures`] gives both from the same tokens, which a data file
//! could not.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use host_mock::{Harness, Node};
use runtime_scene::{realize, Element, Realized};

pub mod edits;
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
    /// Every tagged node of the built tree, paired with its nearest
    /// tagged ancestor, depth-first.
    ///
    /// Only under `ui-overlay`, and deliberately not part of any golden:
    /// a tag's site key folds this file's own path and line, so pinning
    /// it would make an edit above a fixture a test failure. What the
    /// suite asserts on it is the relation — see
    /// `runtime_vocabulary::overlay::tag_tree`.
    #[cfg(feature = "ui-overlay")]
    pub tags: Vec<(Option<runtime_scene::NodeTag>, runtime_scene::NodeTag)>,
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

    #[cfg(feature = "ui-overlay")]
    let mut tags: Vec<(Option<runtime_scene::NodeTag>, runtime_scene::NodeTag)> = Vec::new();

    let realized: Realized<Node> = h.world.enter(|| {
        let st = make();
        let element = build(&st);
        *state.borrow_mut() = Some(st);
        // Read the tags off the BUILT tree, before realize consumes it.
        #[cfg(feature = "ui-overlay")]
        {
            tags = runtime_vocabulary::overlay::tag_tree(&element);
        }
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

    Recording {
        steps,
        scene,
        #[cfg(feature = "ui-overlay")]
        tags,
    }
}

/// Mount one `Element` against a fresh harness and return the scene
/// snapshot it produces.
///
/// The overlay suite's assertion surface: a patch's whole job is to
/// change what ends up on screen, so what a test compares is the
/// rendered tree, not the `Element` that produced it. Separate from
/// [`record`] because a patch test builds its tree at the call site
/// (it has to stage a patch between two builds of the same `ui!`)
/// rather than handing over a `fn`.
pub fn mount_scene(build: impl FnOnce() -> Element) -> String {
    let h = Harness::new();
    h.record_all();
    let realized: Realized<Node> = h.world.enter(|| {
        let element = build();
        realize(&h.backend, &h.registry, element)
    });
    h.flush();
    let roots = h.live_roots();
    let scene = if roots.is_empty() {
        "(empty)".to_string()
    } else {
        roots.iter().map(|r| h.live_tree(*r)).collect::<Vec<_>>().join("\n")
    };
    drop(realized);
    scene
}

/// A scene snapshot with backend node IDs removed.
///
/// A live patch mints FRESH nodes for anything it inserts, so a patched
/// tree and a recompiled one are identical in every way a user could
/// observe and different in the numbers the mock happens to hand out.
/// Comparing shape + content is the honest comparison; comparing IDs
/// would be asserting that a patch reuses allocations, which is not a
/// property anything wants.
pub fn scene_shape(scene: &str) -> String {
    scene
        .lines()
        .map(|line| {
            // `  n3 text "one"` -> `  text "one"`: drop the `nN` token,
            // keep the indentation, which is what carries the structure.
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            match line.trim_start().split_once(' ') {
                Some((first, tail)) if first.starts_with('n') => format!("{indent}{tail}"),
                _ => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A harness with a mounted tree, kept alive so a test can patch it
/// LIVE and then look at the scene again.
///
/// [`mount_scene`] answers "what did this build produce"; this answers
/// "what happens to it afterwards", which is the whole question the live
/// path exists for. The `Realized` is held rather than dropped, because
/// dropping it IS unmount.
#[cfg(feature = "ui-overlay")]
pub struct Mounted {
    pub harness: Harness,
    pub realized: Realized<Node>,
}

#[cfg(feature = "ui-overlay")]
impl Mounted {
    /// Mount `build` and keep everything alive.
    pub fn new(build: impl FnOnce() -> Element) -> Mounted {
        let harness = Harness::new();
        harness.record_all();
        let realized = harness.world.enter(|| {
            let element = build();
            realize(&harness.backend, &harness.registry, element)
        });
        harness.flush();
        Mounted { harness, realized }
    }

    /// Apply edits to what is mounted, through the backend seam.
    pub fn apply_live(
        &mut self,
        site: u64,
        edits: &[runtime_template::Edit],
    ) -> runtime_vocabulary::overlay::Outcome {
        let outcome = self.harness.world.enter(|| {
            runtime_vocabulary::overlay::apply_live_to(
                &self.harness.backend,
                &self.harness.registry,
                site,
                edits,
            )
        });
        self.harness.flush();
        outcome
    }

    /// The site key the mounted tree's outermost tagged node carries.
    ///
    /// Read off the LIVE tree, which is what a dev server does with a
    /// tag it received — there is no way to hash a `SiteId` backwards
    /// into the number the compiled code holds.
    pub fn site(&self) -> u64 {
        fn first<N>(live: &runtime_scene::LiveNode<N>) -> Option<u64> {
            match live {
                runtime_scene::LiveNode::Item { origin, children, .. } => origin
                    .as_ref()
                    .map(|o| o.tag.site)
                    .or_else(|| children.iter().find_map(first)),
                runtime_scene::LiveNode::Fragment(children) => children.iter().find_map(first),
                _ => None,
            }
        }
        first(&self.realized.root).expect("a tagged node in the mounted tree")
    }

    /// Run `f` inside the harness world — for driving a signal.
    pub fn drive(&self, f: impl FnOnce()) {
        self.harness.world.enter(f);
        self.harness.flush();
    }

    /// What the backend would actually be showing — current text,
    /// released nodes gone.
    ///
    /// Not [`scene_snapshot`], which prints creation-time kinds and
    /// keeps released nodes as stray roots because that is what the
    /// frozen goldens pin. A live patch changes exactly those two
    /// things, so asserting it against the frozen projection would be
    /// asserting against the one view that cannot see it.
    pub fn scene(&self) -> String {
        let roots = self.harness.live_roots();
        if roots.is_empty() {
            return "(empty)".to_string();
        }
        roots.iter().map(|r| self.harness.live_tree(*r)).collect::<Vec<_>>().join("\n")
    }
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

/// One corpus entry: a name, its source, and a recorder.
pub struct Fixture {
    pub name: &'static str,
    /// The fixture's `ui!` body as SOURCE.
    ///
    /// The same tokens the macro expanded, kept so the suite can hand
    /// them to `runtime_macros_parse` and check that the descriptor it
    /// produces numbers nodes the way the expansion tagged them. Without
    /// it there would be no way to run both halves on one input — the
    /// corpus is a `macro_rules!` precisely so a fixture is authored
    /// once, and `stringify!` is how that single authoring reaches the
    /// library half.
    pub body: &'static str,
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
