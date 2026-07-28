//! FULL-OP harness, NEW-core side: the vocabulary's builders + generic
//! handlers realized against `LegacyBridge<FullRecorder>` — the recorder
//! implements `runtime_core::Backend`, the bridge supplies `Host` + the
//! capability traits, and `register_builtins` installs the 13 handlers.
//! Compared against the SAME `goldens_full/` files the old walker owns,
//! through the established normalization, plus the closed
//! [`FULL_NEWCORE_OVERRIDES`] set for the one sanctioned full-op
//! divergence:
//!
//! - **Teardown release order (LIFO vs creation order).** On a subtree
//!   swap-out the old core fires scope-level cleanups LIFO (`Scope::drop`
//!   reverses `cleanups`), then drops effects in creation order; the new
//!   kernel frees collected effects in creation order, running each's
//!   cleanups as it goes. Same release SET (every `release_text_id`,
//!   every `on_node_unstyled`), different interleaving — an ordering
//!   artifact of the two teardown mechanisms, never a missing or extra
//!   backend effect. Pinned by the `full_release_on_swap` override
//!   goldens (both modes).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use runtime_scene::{realize, Element, Realized, Registry};
use runtime_vocabulary::LegacyBridge;
use runtime_world::World;

use crate::full::{full_golden_path, full_newcore_golden_path, FullRecorder};
use crate::new_core::normalize;
use crate::scenarios_full_new::full_new_scenarios;
use crate::{serialize_steps, Mode, PNode, Recorder, Step};

/// The backend the new side realizes against.
pub type Bridged = LegacyBridge<FullRecorder>;

/// A new-core full-op scenario (same names/modes/labels/mutations as
/// `scenarios_full`, asserted by the registry-mirror test).
pub struct FullNewScenario {
    pub name: &'static str,
    pub modes: &'static [Mode],
    pub run: fn(&mut FullNewCx),
}

/// New-core full-op driver: mount realizes through the vocabulary
/// registry; `step` flushes the world after the mutation.
pub struct FullNewCx {
    rec: Recorder,
    backend: Rc<RefCell<Bridged>>,
    registry: Rc<Registry<Bridged>>,
    world: World,
    realized: Option<Realized<PNode>>,
    steps: Vec<Step>,
}

impl FullNewCx {
    pub fn recorder(&self) -> Recorder {
        self.rec.clone()
    }

    /// The `nth` captured interaction-state setter — mirror of
    /// `FullCx::state_setter` (P3c overlay-flip scenarios).
    pub fn state_setter(&self, nth: usize) -> Rc<dyn Fn(runtime_core::StateBits, bool)> {
        self.backend.borrow().0.state_setter(nth)
    }

    pub fn mount(&mut self, root: Element) {
        assert!(self.realized.is_none(), "scenario mounted twice");
        let realized = realize(&self.backend, &self.registry, root);
        self.realized = Some(realized);
        self.snap("mount");
    }

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

/// Run `scenario` in `mode` against the new core; returns the
/// header-less step body.
pub fn run_full_scenario_new(scenario: &FullNewScenario, mode: Mode) -> String {
    let rec = Recorder::default();
    let backend = Rc::new(RefCell::new(LegacyBridge(FullRecorder::new(
        rec.clone(),
        mode,
    ))));
    let mut registry: Registry<Bridged> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let world = World::new();
    let mut cx = FullNewCx {
        rec,
        backend,
        registry: Rc::new(registry),
        world: world.clone(),
        realized: None,
        steps: Vec::new(),
    };
    world.enter(|| (scenario.run)(&mut cx));
    let steps = std::mem::take(&mut cx.steps);
    // Unmount (Realized then World) after the last snapshot — teardown
    // is outside the pinned sequence (sanctioned divergence #4).
    drop(cx);
    drop(world);
    serialize_steps(&steps)
}

/// The closed override set for the full-op suite — sanctioned
/// divergence: teardown release ordering (see module docs). Adding a
/// pair requires a matching README entry.
pub const FULL_NEWCORE_OVERRIDES: &[(&str, Mode)] = &[
    ("full_release_on_swap", Mode::Anchored),
    ("full_release_on_swap", Mode::Spliced),
];

fn override_path(name: &str, mode: Mode) -> PathBuf {
    full_newcore_golden_path(name, mode)
}

/// New-core full-op test entry point: normalized comparison against the
/// shared golden, or the explicit override for sanctioned pairs.
/// `UPDATE_NEWCORE_GOLDENS=1` (re)writes override files only.
pub fn check_full_new(name: &str, mode: Mode) {
    let all = full_new_scenarios();
    let scenario = all
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no new-core full-op scenario named `{name}`"));
    assert!(
        scenario.modes.contains(&mode),
        "full-op scenario `{name}` is not registered for mode {mode:?}",
    );

    let raw = run_full_scenario_new(scenario, mode);
    let actual = normalize(&raw);

    let is_override = FULL_NEWCORE_OVERRIDES.contains(&(name, mode));
    if std::env::var_os("UPDATE_NEWCORE_GOLDENS").is_some() && is_override {
        let path = override_path(name, mode);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create goldens_full_newcore dir");
        let header = format!(
            "# scene-parity FULL-OP NEW-CORE override — scenario `{name}`, mode `{mode}`\n\
             #\n\
             # Sanctioned divergence (README): teardown release ordering. The old\n\
             # core fires scope-level cleanups LIFO then drops effects in creation\n\
             # order; the new kernel frees collected effects in creation order.\n\
             # Same release SET (every release_text_id / on_node_unstyled), only\n\
             # the interleaving differs, only in teardown windows.\n\
             # Regenerate: UPDATE_NEWCORE_GOLDENS=1 cargo test -p scene-parity\n",
            mode = mode.suffix()
        );
        std::fs::write(&path, format!("{header}{raw}")).expect("write newcore golden");
        return;
    }

    let (path, expected_raw) = if is_override {
        let path = override_path(name, mode);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "missing full-op new-core override golden {}\nGenerate it with: \
                 UPDATE_NEWCORE_GOLDENS=1 cargo test -p scene-parity",
                path.display()
            )
        });
        (path, text)
    } else {
        let path = full_golden_path(name, mode);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing full-op golden {}", path.display()));
        (path, text)
    };
    let expected = normalize(&expected_raw);

    if expected != actual {
        panic!(
            "FULL-OP NEW-CORE mismatch for scenario `{name}` (mode {mode:?}).\n\
             The vocabulary handlers diverged from the walker's backend-call\n\
             stream — fix the handler, not the golden (only README-sanctioned\n\
             divergences may differ, via normalization or an explicit\n\
             goldens_full_newcore/ override).\n\
             \n--- expected ({path}) ---\n{expected}\n--- actual (new core) ---\n{actual}",
            path = path.display(),
        );
    }
}
