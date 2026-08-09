//! A complete third-party backend, built from the public surface only.
//!
//! The framework's claim is that a backend is a *replaceable* part: the
//! in-house set (web, UIKit, AppKit, Android, wgpu, terminal, …) has no
//! privileged access, and anyone can render an idealyst tree onto a
//! toolkit we've never heard of. That claim is easy to believe and easy
//! to break — an in-tree backend can reach a `pub(crate)` helper, a
//! `#[doc(hidden)]` installer, or a sibling module without anyone
//! noticing that an out-of-tree crate could not have done the same.
//!
//! `RecordingBackend` below is the executable form of the claim. It is a
//! real backend — `Host` plus all 30 capability traits, a `BuiltinSet`-
//! generic boot entry, a flush driver — and it is written under the same
//! constraints a third-party crate has:
//!
//! - **`runtime_vocabulary::backend` is the only framework import.** No
//!   `runtime-core` (that root is the *author* surface; its `glue`
//!   re-export deliberately shadows substrate names with authoring
//!   wrappers), no `#[doc(hidden)]` path, no crate-internal helper.
//! - **Nothing is reached through a sibling backend.** No copied
//!   `pub(crate)` fn, no `crate::` path that only resolves in-tree.
//!
//! `public_surface_is_sufficient_for_an_out_of_tree_backend` pins the
//! first constraint by scanning this file's own source, so the proof
//! cannot rot into "it compiles because it's in-tree."
//!
//! If a future refactor moves something a backend author needs behind a
//! private door, this file stops compiling — which is the point.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_vocabulary::backend::{
    self as fw, caps, install_env_services, realize, register_builtins_with, AllBuiltins,
    BuiltinSet, Element, Host, Registry, World,
};
// Value types in the capability signatures. Reached through the prelude's
// `runtime_shared` re-export, so this backend needs exactly one framework
// dependency in its Cargo.toml.
use fw::runtime_shared::accessibility::AccessibilityProps;
use fw::runtime_shared::primitives::icon::IconData;
use fw::runtime_shared::{Action, StyleRules};

// ---------------------------------------------------------------------------
// The backend
// ---------------------------------------------------------------------------

/// The node type. A real backend hands back a handle to a platform view;
/// `Clone` is the only constraint the framework imposes, because
/// structural regions retain node handles across effect fires.
#[derive(Clone, Debug, PartialEq)]
struct NodeId(usize);

#[derive(Default)]
struct Tree {
    /// `parent -> children`, in insertion order.
    kids: Vec<(usize, Vec<usize>)>,
    /// What each node is, for assertions.
    kind: Vec<String>,
}

impl Tree {
    fn slot(&mut self, id: usize) -> &mut Vec<usize> {
        if let Some(i) = self.kids.iter().position(|(p, _)| *p == id) {
            return &mut self.kids[i].1;
        }
        self.kids.push((id, Vec::new()));
        let last = self.kids.len() - 1;
        &mut self.kids[last].1
    }

    fn children(&self, id: usize) -> &[usize] {
        self.kids
            .iter()
            .find(|(p, _)| *p == id)
            .map(|(_, c)| c.as_slice())
            .unwrap_or(&[])
    }
}

#[derive(Default)]
struct RecordingBackend {
    tree: Tree,
    next: usize,
    /// Every capability call, in order — the flush-driver and mount
    /// contract assertions read this.
    log: Vec<String>,
}

impl RecordingBackend {
    fn mint(&mut self, kind: &str) -> NodeId {
        let id = self.next;
        self.next += 1;
        self.kind.push(kind.to_string());
        self.log.push(format!("create {kind} #{id}"));
        NodeId(id)
    }
}

// `kind` lives on the Tree; this keeps `mint` readable.
impl std::ops::Deref for RecordingBackend {
    type Target = Tree;
    fn deref(&self) -> &Tree {
        &self.tree
    }
}
impl std::ops::DerefMut for RecordingBackend {
    fn deref_mut(&mut self) -> &mut Tree {
        &mut self.tree
    }
}

// --- 1. Host: the seven structural ops -------------------------------------

impl Host for RecordingBackend {
    type Node = NodeId;

    fn insert(&mut self, parent: &mut NodeId, child: NodeId) {
        self.log.push(format!("insert #{} -> #{}", child.0, parent.0));
        let p = parent.0;
        self.slot(p).push(child.0);
    }

    fn insert_at(&mut self, parent: &mut NodeId, child: NodeId, index: usize) {
        self.log
            .push(format!("insert_at #{} -> #{} @{index}", child.0, parent.0));
        let p = parent.0;
        let slot = self.slot(p);
        let at = index.min(slot.len());
        slot.insert(at, child.0);
    }

    fn remove_child(&mut self, parent: &NodeId, child: &NodeId) {
        self.log
            .push(format!("remove #{} from #{}", child.0, parent.0));
        let (p, c) = (parent.0, child.0);
        self.slot(p).retain(|k| *k != c);
    }

    fn clear_children(&mut self, node: &NodeId) {
        self.log.push(format!("clear #{}", node.0));
        let n = node.0;
        self.slot(n).clear();
    }

    fn create_anchor(&mut self) -> NodeId {
        self.mint("anchor")
    }

    /// `false` is always correct and costs one anchor node per reactive
    /// region. A backend that can splice children directly into a real
    /// parent returns `true` and lets style-less regions go anchorless.
    fn supports_splice(&self) -> bool {
        false
    }
}

// --- 2. The capabilities this backend actually implements ------------------

impl caps::ViewOps for RecordingBackend {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> NodeId {
        self.mint("view")
    }
}

impl caps::TextOps for RecordingBackend {
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> NodeId {
        let n = self.mint("text");
        self.log.push(format!("text #{} = {content:?}", n.0));
        n
    }

    fn update_text(&mut self, node: &NodeId, content: &str) {
        self.log.push(format!("update_text #{} = {content:?}", node.0));
    }
}

impl caps::ButtonOps for RecordingBackend {
    /// The one capability with a required method that has no sensible
    /// default: `ButtonOps: TextOps`, and the label/action pair has no
    /// generic lowering. Every backend implements it explicitly.
    fn create_button(
        &mut self,
        label: &str,
        _on_click: &Action,
        _leading_icon: Option<&IconData>,
        _trailing_icon: Option<&IconData>,
        _a11y: &AccessibilityProps,
    ) -> NodeId {
        let n = self.mint("button");
        self.log.push(format!("button #{} = {label:?}", n.0));
        n
    }
}

impl caps::StyleOps for RecordingBackend {
    fn apply_style(&mut self, node: &NodeId, _style: &Rc<StyleRules>) {
        // Each call is a fresh authoritative application — overwrite,
        // never accumulate.
        self.log.push(format!("apply_style #{}", node.0));
    }
}

impl caps::LifecycleOps for RecordingBackend {
    fn finish(&mut self, root: NodeId) {
        self.log.push(format!("finish #{}", root.0));
    }
}

impl caps::AppEnvOps for RecordingBackend {
    fn platform(&self) -> fw::Platform {
        fw::Platform::Custom("Recording")
    }
}

// --- 3. Everything else: defaults ------------------------------------------
//
// Every remaining cap trait has a default for all of its methods, so a
// backend opts in per family with an empty impl. Each one skipped
// degrades that primitive to a placeholder or a plain container rather
// than failing the build — which is what makes a partial backend viable.

impl caps::InputOps for RecordingBackend {}
impl caps::PressableOps for RecordingBackend {}
impl caps::AssetOps for RecordingBackend {}
impl caps::ExternalOps for RecordingBackend {}
impl caps::DocumentOps for RecordingBackend {}
impl caps::ImageOps for RecordingBackend {}
impl caps::IconOps for RecordingBackend {}
impl caps::LinkOps for RecordingBackend {}
impl caps::TextInputOps for RecordingBackend {}
impl caps::ToggleOps for RecordingBackend {}
impl caps::SliderOps for RecordingBackend {}
impl caps::ActivityIndicatorOps for RecordingBackend {}
impl caps::ScrollOps for RecordingBackend {}
impl caps::SafeAreaOps for RecordingBackend {}
impl caps::VirtualizerOps for RecordingBackend {}
impl caps::GridOps for RecordingBackend {}
impl caps::PortalOps for RecordingBackend {}
impl caps::PresenceOps for RecordingBackend {}
impl caps::NavigatorOps for RecordingBackend {}
impl caps::GraphicsOps for RecordingBackend {}
impl caps::A11yOps for RecordingBackend {}
impl caps::AnimationOps for RecordingBackend {}
impl caps::IntrospectionOps for RecordingBackend {}
impl caps::BatchOps for RecordingBackend {}
impl caps::WireBindingOps for RecordingBackend {}

// ---------------------------------------------------------------------------
// The boot entry
// ---------------------------------------------------------------------------

/// What a boot entry retains. Field order is drop order: the `Realized`
/// tree must unmount before the `World` that owns the slots its effects
/// read, so `realized` is declared first.
struct App {
    #[allow(dead_code)]
    realized: fw::Realized<NodeId>,
    _world: World,
    backend: Rc<RefCell<RecordingBackend>>,
}

/// The `BuiltinSet`-generic entry. Generic over `S` rather than pinning
/// `AllBuiltins`, so an app can drop unused primitive families and the
/// linker can drop their handlers with them — the same lever every
/// in-house backend exposes as `start_with` / `start_in_with`.
fn start_with<S: BuiltinSet>(
    backend: Rc<RefCell<RecordingBackend>>,
    register: impl FnOnce(&mut Registry<RecordingBackend>),
    build: impl FnOnce() -> Element,
) -> App {
    // 1. Ambient environment services, BEFORE the build: a component body
    //    may read `platform()` while constructing.
    install_env_services(&backend);

    // 2. Registry: builtins for the selected set, then the app's own
    //    payload handlers on the same registry.
    let mut registry: Registry<RecordingBackend> = Registry::new();
    register_builtins_with::<RecordingBackend, S>(&mut registry);
    register(&mut registry);
    let registry = Rc::new(registry);

    // 3. Realize inside `world.enter` so free signal()/effect() calls in
    //    the root build resolve against this world.
    let world = World::new();
    let realized = world.enter(|| realize(&backend, &registry, build()));

    // 4. Hand the single root to `finish`.
    let mut roots = realized.collect_nodes();
    assert_eq!(roots.len(), 1, "single-root mount contract");
    let root = roots.pop().expect("len checked");
    caps::LifecycleOps::finish(&mut *backend.borrow_mut(), root);

    App { realized, _world: world, backend }
}

// --- SCAN-REGION-END -------------------------------------------------------
//
// Everything ABOVE this marker is the backend proper, and is what
// `public_surface_is_sufficient_for_an_out_of_tree_backend` scans. Test
// scaffolding below is exempt — it is allowed to reach for author-surface
// conveniences a real backend would not need.

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn boot(build: impl FnOnce() -> Element) -> App {
    start_with::<AllBuiltins>(
        Rc::new(RefCell::new(RecordingBackend::default())),
        |_| {},
        build,
    )
}

/// The whole point: a backend written only against the public surface
/// mounts a real tree.
#[test]
fn an_out_of_tree_backend_boots_and_realizes_a_tree() {
    let app = boot(|| {
        runtime_vocabulary::builders::view()
            .child(runtime_vocabulary::builders::text().content("hello").build())
            .child(runtime_vocabulary::builders::text().content("world").build())
            .build()
    });

    let b = app.backend.borrow();
    assert!(
        b.log.iter().any(|l| l.starts_with("create view")),
        "root view must be created through ViewOps::create_view; log: {:#?}",
        b.log
    );
    assert_eq!(
        b.log.iter().filter(|l| l.starts_with("create text")).count(),
        2,
        "both text leaves must reach TextOps::create_text; log: {:#?}",
        b.log
    );
    assert!(
        b.log.iter().any(|l| l.starts_with("finish")),
        "LifecycleOps::finish must receive the single root; log: {:#?}",
        b.log
    );

    // Children are constructed before their parent's insert call.
    let create_view = b.log.iter().position(|l| l.starts_with("create view")).unwrap();
    let first_insert = b.log.iter().position(|l| l.starts_with("insert ")).unwrap();
    assert!(
        create_view < first_insert,
        "insert must follow both nodes existing; log: {:#?}",
        b.log
    );

    // …and the parent really ends up owning them, in author order. This
    // reads the backend's own tree rather than the call log, so it fails
    // if `insert` was called but the structural op was a no-op.
    let parented: Vec<&[usize]> = (0..b.next)
        .map(|id| b.children(id))
        .filter(|kids| !kids.is_empty())
        .collect();
    assert_eq!(
        parented.len(),
        1,
        "exactly one node should have children (the root view); tree: {:#?}",
        b.tree.kids
    );
    assert_eq!(
        parented[0].len(),
        2,
        "the root view must own both text leaves; tree: {:#?}",
        b.tree.kids
    );

    drop(b);
    // Teardown must not panic: `Realized` unmounts before the `World`.
    drop(app);
}

/// The boot entry's environment install reaches the ambient author reads
/// — proving `install_env_services` is usable from outside the tree, not
/// just from the in-house boots that call it today.
#[test]
fn an_out_of_tree_backend_seeds_the_ambient_environment() {
    assert_eq!(
        runtime_shared::platform(),
        fw::Platform::Custom(""),
        "precondition: fresh thread, nothing installed"
    );

    let app = boot(|| runtime_vocabulary::builders::text().content("x").build());

    assert_eq!(
        runtime_shared::platform(),
        fw::Platform::Custom("Recording"),
        "the third-party backend's AppEnvOps::platform must reach platform()"
    );
    drop(app);
}

/// Source-scan: this fixture must keep proving *out-of-tree* viability.
///
/// The failure this prevents is silent. Someone debugging a test reaches
/// for `runtime_core::…` or a `#[doc(hidden)]` installer, the file still
/// compiles because it lives in-tree, and the proof quietly degrades into
/// "an in-tree backend works" — which was never in doubt.
#[test]
fn public_surface_is_sufficient_for_an_out_of_tree_backend() {
    let src = include_str!("third_party_backend.rs");

    // Scan the backend + boot entry only. The region ends at the marker
    // below, because this test's own forbidden-name list would otherwise
    // match itself.
    let end = src
        .find("SCAN-REGION-END")
        .expect("scan-region marker must exist");
    let region = &src[..end];

    let mut bad: Vec<String> = Vec::new();
    for (i, line) in region.lines().enumerate() {
        // Skip prose: this file documents the rule as well as obeying it.
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        for forbidden in ["runtime_core", "glue::", "__scene", "glue_lazy", "robot_methods"] {
            if line.contains(forbidden) {
                bad.push(format!(
                    "line {}: reaches `{forbidden}`, which an out-of-tree \
                     backend must not need: {}",
                    i + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        bad.is_empty(),
        "the third-party backend fixture escaped the public surface:\n  {}",
        bad.join("\n  "),
    );
}
