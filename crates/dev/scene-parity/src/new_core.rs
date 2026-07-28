//! The NEW-core harness mode: run the parity scenarios against
//! `runtime-scene` (the P1 core) and compare the structural-op sequences
//! against the SAME golden files the old walker pins.
//!
//! Three pieces mirror the old-core harness one-for-one:
//!
//! - [`SceneHost`] — a recording `runtime_scene::Host` sharing the old
//!   recorder's op-line format and `n0, n1, …` creation-order naming, so a
//!   matching run produces byte-identical step bodies.
//! - A tiny test vocabulary ([`NcView`]/[`NcText`] + [`nc_registry`]) —
//!   the scene is payload-agnostic, so the harness brings the two
//!   primitives the scenarios need. P2's real vocabulary replaces this.
//! - [`NewCx`] + [`check_new`] — the scenario driver (now with a
//!   `world.flush()` per step, per the README's re-targeting contract)
//!   and the golden comparison.
//!
//! # Sanctioned divergences (README "Where the new core is ALLOWED to
//! differ") and how each is handled
//!
//! 1. **Anchor/node naming** — the new drivers create anchors eagerly in
//!    the same creation order as the old walker, so names align and no
//!    renaming layer is needed.
//! 2. **First-fire `clear_children` on a fresh anchor** — the new Dyn/
//!    Keyed drivers deliberately DON'T clear a virgin anchor. Rather than
//!    forking the anchored goldens (the old core must keep matching them
//!    byte-for-byte), [`normalize`] strips exactly that pattern — a
//!    `clear_children nX` immediately following `create nX anchor` — from
//!    BOTH sides of the comparison. The pattern is unambiguous: a real
//!    (content-bearing) clear can never be adjacent to its anchor's
//!    creation.
//! 3. **Cross-effect firing order after an unsubscribe** — the old
//!    arena's swap-remove unsubscription scrambles notification order;
//!    `runtime-world`'s `retain`-based unlinking preserves creation
//!    order. Exactly one (scenario, mode) observes this:
//!    `nested_when_in_each_row.spliced`, final step. Its new-core
//!    expectation lives in `goldens_newcore/` as an explicit override
//!    file (each row's own remove→create→insert_at sub-sequence is
//!    unchanged — only the row interleaving and the consequent node
//!    numbering differ). [`check_new`] prefers an override file when one
//!    exists; the override set is closed (`NEWCORE_OVERRIDES`).
//! 4. **Owner-drop teardown** — as in the old harness, the final unmount
//!    (dropping the `Realized` + `World`) happens after the last
//!    snapshot, outside the pinned sequence.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use runtime_scene::{realize, Element, Host, Realized, Registry};
use runtime_world::World;

use crate::scenarios_new::{new_scenarios, NewScenario};
use crate::{golden_path, serialize_steps, Mode, PNode, Recorder, Step};

// ===========================================================================
// SceneHost — the recording Host
// ===========================================================================

/// A `runtime_scene::Host` that records the 7 structural ops into the
/// shared [`Recorder`], with the exact op-line format `ParityBackend`
/// uses. Node creation for the vocabulary goes through [`SceneHost::create`]
/// (the scene has no `create_*` methods — handlers own creation).
pub struct SceneHost {
    rec: Recorder,
    splice: bool,
}

impl SceneHost {
    /// Mint a node and record `create nK <label>` — the handler-side
    /// analogue of the old backend's `create_view`/`create_text`.
    pub fn create(&mut self, label: &str) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!("create {n} {label}"));
        n
    }
}

impl Host for SceneHost {
    type Node = PNode;

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
        self.rec
            .push(format!("insert_at {parent} <- {child} @ {index}"));
    }

    fn remove_child(&mut self, parent: &PNode, child: &PNode) {
        self.rec.push(format!("remove_child {parent} -x {child}"));
    }

    fn clear_children(&mut self, node: &PNode) {
        self.rec.push(format!("clear_children {node}"));
    }

    fn create_anchor(&mut self) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!("create {n} anchor"));
        n
    }

    fn supports_splice(&self) -> bool {
        self.splice
    }
}

// ===========================================================================
// Test vocabulary — view + text, the two primitives the scenarios use
// ===========================================================================

/// Container payload: creates a `view` node and realizes children into it.
pub struct NcView;

/// Leaf payload: creates a `text "<content>"` node.
pub struct NcText(pub String);

/// The harness registry: `view` and `text` handlers in the canonical
/// handler shape (create → realize children → return node).
pub fn nc_registry() -> Registry<SceneHost> {
    let mut registry: Registry<SceneHost> = Registry::new();
    registry.register::<NcView, _>(|cx, _payload, children| {
        let mut node = cx.backend().borrow_mut().create("view");
        cx.realize_children_into(&mut node, children);
        node
    });
    registry.register::<NcText, _>(|cx, payload, _children| {
        cx.backend()
            .borrow_mut()
            .create(&format!("text {:?}", payload.0))
    });
    registry
}

// ===========================================================================
// NewCx — the scenario driver
// ===========================================================================

/// The new-core scenario driver: same `mount`/`step`/`recorder` surface as
/// the old `Cx`, with the one contract change the README documents —
/// `step` flushes the world after the mutation (the staged-commit core is
/// not synchronous).
pub struct NewCx {
    rec: Recorder,
    backend: Rc<RefCell<SceneHost>>,
    registry: Rc<Registry<SceneHost>>,
    world: World,
    // Field order = drop order: the Realized (unmount) must go before the
    // World it lives in.
    realized: Option<Realized<PNode>>,
    steps: Vec<Step>,
}

impl NewCx {
    /// Clone of the shared recorder — for cleanup markers.
    pub fn recorder(&self) -> Recorder {
        self.rec.clone()
    }

    /// Realize the tree and snapshot the mount ops as the `mount` step.
    pub fn mount(&mut self, root: Element) {
        assert!(self.realized.is_none(), "scenario mounted twice");
        let realized = realize(&self.backend, &self.registry, root);
        self.realized = Some(realized);
        self.snap("mount");
    }

    /// Run a mutation, flush the world (staged-commit core), snapshot.
    pub fn step(&mut self, label: &str, f: impl FnOnce()) {
        f();
        self.world.flush();
        self.snap(label);
    }

    fn snap(&mut self, label: &str) {
        self.steps.push(Step {
            label: label.to_string(),
            ops: self.rec.take_ops(),
        });
    }
}

/// Run `scenario` in `mode` against the new core, returning the
/// serialized (header-less) step body.
pub fn run_scenario_new(scenario: &NewScenario, mode: Mode) -> String {
    let rec = Recorder::default();
    let backend = Rc::new(RefCell::new(SceneHost {
        rec: rec.clone(),
        splice: mode.splice(),
    }));
    let world = World::new();
    let mut cx = NewCx {
        rec,
        backend,
        registry: Rc::new(nc_registry()),
        world: world.clone(),
        realized: None,
        steps: Vec::new(),
    };
    // The whole body runs with the world ambient so the free
    // `runtime_world::signal()`/`effect()` scenarios use resolve.
    world.enter(|| (scenario.run)(&mut cx));
    let steps = std::mem::take(&mut cx.steps);
    // Unmount (drop Realized, then World) AFTER taking the steps —
    // teardown is outside the pinned sequence (sanctioned divergence #4).
    drop(cx);
    drop(world);
    serialize_steps(&steps)
}

// ===========================================================================
// Golden comparison — normalization + overrides
// ===========================================================================

/// The closed set of (scenario, mode) pairs allowed a `goldens_newcore/`
/// override file — sanctioned divergence #3 (cross-effect firing order
/// after an unsubscribe) is the only member. Adding a pair here requires
/// a matching README entry.
pub const NEWCORE_OVERRIDES: &[(&str, Mode)] = &[("nested_when_in_each_row", Mode::Spliced)];

/// Absolute path of a new-core override golden.
pub fn newcore_golden_path(name: &str, mode: Mode) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("goldens_newcore")
        .join(format!("{name}.{}.golden", mode.suffix()))
}

/// Canonicalize a golden text for the new-core comparison:
///
/// 1. drop `#` header/comment lines (the shared goldens carry the
///    old-core header; the new runner emits none),
/// 2. strip first-fire anchor clears — a `clear_children nX` line
///    IMMEDIATELY following `create nX anchor` (sanctioned divergence #2;
///    the adjacency makes the pattern unambiguous: an anchor with content
///    can only be cleared in a later effect fire, never adjacent to its
///    own creation).
///
/// Applied to BOTH sides so the comparison is symmetric.
pub fn normalize(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        if let Some(prev) = out.last() {
            if let Some(id) = prev
                .strip_prefix("create ")
                .and_then(|rest| rest.strip_suffix(" anchor"))
            {
                if line == format!("clear_children {id}") {
                    continue;
                }
            }
        }
        out.push(line);
    }
    out.join("\n")
}

/// New-core test entry point: run the named scenario in `mode` against
/// `runtime-scene` and assert the (normalized) op sequence matches the
/// shared golden — or the explicit `goldens_newcore/` override for the
/// sanctioned-divergence pairs.
///
/// With `UPDATE_NEWCORE_GOLDENS=1`, (re)writes the override file instead —
/// but ONLY for pairs in [`NEWCORE_OVERRIDES`]; everything else must match
/// the shared golden, which belongs to the old core.
pub fn check_new(name: &str, mode: Mode) {
    let all = new_scenarios();
    let scenario = all
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no new-core scenario named `{name}` in the registry"));
    assert!(
        scenario.modes.contains(&mode),
        "scenario `{name}` is not registered for mode {mode:?}",
    );

    let raw = run_scenario_new(scenario, mode);
    let actual = normalize(&raw);

    let is_override = NEWCORE_OVERRIDES.contains(&(name, mode));
    if std::env::var_os("UPDATE_NEWCORE_GOLDENS").is_some() && is_override {
        let path = newcore_golden_path(name, mode);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create goldens_newcore dir");
        let header = format!(
            "# scene-parity NEW-CORE override — scenario `{name}`, mode `{mode}`\n\
             #\n\
             # Sanctioned divergence #3 (README): after an unsubscribe, the old\n\
             # arena's swap-remove scrambles sibling-effect notification order;\n\
             # runtime-world preserves creation order. Each row's own op\n\
             # sub-sequence is identical to the shared golden — only the row\n\
             # interleaving (and hence node numbering) differs, in the final step.\n\
             # Regenerate: UPDATE_NEWCORE_GOLDENS=1 cargo test -p scene-parity\n",
            mode = mode.suffix()
        );
        std::fs::write(&path, format!("{header}{raw}")).expect("write newcore golden");
        return;
    }

    let (path, expected_raw) = if is_override {
        let path = newcore_golden_path(name, mode);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "missing new-core override golden {}\nGenerate it with: \
                 UPDATE_NEWCORE_GOLDENS=1 cargo test -p scene-parity",
                path.display()
            )
        });
        (path, text)
    } else {
        let path = golden_path(name, mode);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing golden file {}", path.display()));
        (path, text)
    };
    let expected = normalize(&expected_raw);

    if expected != actual {
        panic!(
            "NEW-CORE structural-op mismatch for scenario `{name}` (mode {mode:?}).\n\
             The runtime-scene drivers diverged from the golden contract — fix the\n\
             driver, not the golden (only README-sanctioned divergences may differ,\n\
             via normalization or an explicit goldens_newcore/ override).\n\
             \n--- expected ({path}) ---\n{expected}\n--- actual (new core) ---\n{actual}",
            path = path.display(),
        );
    }
}
