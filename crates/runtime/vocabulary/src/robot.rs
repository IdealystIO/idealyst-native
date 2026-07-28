//! The NEW core's element identity / robot registry (P5 seam, brought
//! forward for the conformance-app gate).
//!
//! # Design: a vocabulary-owned registry, not a re-export of the old one
//!
//! The old core's registry (`runtime_core::robot`) is walker-internal:
//! its registration surface (`register`, `deregister`, the parent
//! stack, the `attach_*` action wiring) is `pub(crate)` and driven from
//! `walker.rs::build_inner` — there is no public write path a foreign
//! crate could feed, runtime-core is frozen during the migration, and
//! the whole module is deleted with the walker at P7. So the new core
//! gets its own registry HERE — but the *model* is a faithful mirror of
//! the old one (same entry shape, same query semantics, same
//! duplicate-id policy, same register-at-mount / deregister-at-teardown
//! lifecycle), so the robot bridge's verbs (`find_element`,
//! `get_snapshot`, `tap`, `type_text`, `set_toggle`, `set_slider`,
//! `focus`/`blur`, `set_scroll`, `get_frame`/`get_absolute_frame`,
//! `introspect_native`, …) adapt 1:1.
//!
//! # The backend seam (conformance-wave contract)
//!
//! No new Backend/caps methods are required. Every node-resolving
//! action is a closure captured at mount over capabilities the traits
//! already expose:
//!
//! - geometry + native introspection ride
//!   [`IntrospectionOps`](crate::caps::IntrospectionOps)
//!   (`frame`/`absolute_frame`/`device_frame`/`introspect_native`,
//!   plus `note_introspection_root` at registration);
//! - `focus`/`blur` go through `make_text_input_handle(..).focus()`,
//!   `set_scroll` through `make_scroll_view_handle(..).scroll_to(..)`
//!   — the handle routes, exactly like the old walker (see the
//!   borrow-safety note on the scroll action below);
//! - `click`/`set_text`/`set_toggle`/`set_slider` are the author
//!   callbacks the mount handler already holds.
//!
//! [`LegacyBridge`](crate::bridge::LegacyBridge) implements all of the
//! above by delegation, so every existing backend participates with
//! ZERO backend edits. The transport half (conformance wave) is now in
//! place: [`bridge::invoke_command`] dispatches the wire verbs against
//! THIS registry, [`current_revision`] drives the live-update push, and
//! the [`install_driver_env`] seam lets the host supply `World::enter`
//! (label-resolving queries need the ambient world) plus a synchronous
//! settle (actions commit staged writes before returning). The web
//! relay client routes to all three when the new-core boot installed
//! the env (`backend_web::newcore` ↔ `robot_transport`); the in-process
//! `robot-e2e` harness rides the same seam via its `new-core` feature.
//!
//! # Lifecycle
//!
//! [`register_mount`] is called by every mount handler right after the
//! backend node exists and BEFORE children realize (children peek the
//! parent stack to link parenting; the returned guard pops on drop).
//! Deregistration is an [`on_teardown`](crate::style_attach::on_teardown)
//! probe registered at mount: when the realized subtree's `Owned`
//! drops (branch swap, keyed row removal, unmount), the entry is
//! removed. A stale entry after teardown is the classic bug this
//! prevents — see `regression_unmount_deregisters_entries` in the
//! vocabulary test suite.
//!
//! # Duplicate `test_id` policy (mirrors the old core exactly)
//!
//! The `by_test_id` index is LAST-WINS: `find(Query::TestId)` resolves
//! the most recently registered holder. `find_all(Query::TestId)` scans
//! every entry, returning ALL holders — sibling list rows legitimately
//! share one affordance test_id (Playwright's
//! `getByTestId(...).toHaveCount(n)`).
//!
//! # Feature gate
//!
//! The whole module is compiled only under the vocabulary's `robot`
//! Cargo feature (mirroring runtime-core's `robot` gate): the identity
//! SLOTS (`test_id` prim fields + builder/glue setters) are always
//! present so `ui! { view(test_id = …) }` compiles in every build, but
//! the per-mount registration overhead exists only in robot builds.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::primitives::portal::ViewportRect;

use crate::caps::IntrospectionOps;

// =============================================================================
// Element kinds (mirrors `runtime_core::robot::ElementKind`)
// =============================================================================

/// Identifies what kind of primitive an element was built from. Same
/// variant set as the old core's, so bridge verb payloads (`kind`
/// strings) stay wire-identical.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ElementKind {
    View,
    Text,
    Button,
    Pressable,
    Image,
    Icon,
    TextInput,
    Toggle,
    ScrollView,
    Slider,
    ActivityIndicator,
    Virtualizer,
    Graphics,
    Navigator,
    Link,
    Overlay,
    Presence,
}

/// Opaque identifier for a mounted element in the registry. Invalid
/// after the element's subtree tears down (the entry is removed).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ElementId(pub u32);

// =============================================================================
// Control handles
// =============================================================================

/// Type-erased action dispatch table for a mounted element (the old
/// registry's shape, verbatim).
#[derive(Clone, Default)]
pub(crate) struct ElementActions {
    pub click: Option<Rc<dyn Fn()>>,
    pub set_text: Option<Rc<dyn Fn(String)>>,
    pub set_toggle: Option<Rc<dyn Fn(bool)>>,
    pub set_slider: Option<Rc<dyn Fn(f32)>>,
    pub focus: Option<Rc<dyn Fn()>>,
    pub blur: Option<Rc<dyn Fn()>>,
    pub set_scroll: Option<Rc<dyn Fn(f32, f32)>>,
    pub frame: Option<Rc<dyn Fn() -> Option<ViewportRect>>>,
    pub absolute_frame: Option<Rc<dyn Fn() -> Option<ViewportRect>>>,
    pub device_frame: Option<Rc<dyn Fn() -> Option<ViewportRect>>>,
    pub introspect: Option<Rc<dyn Fn() -> Option<runtime_core::introspect::NativeNode>>>,
}

/// The handler-supplied half of [`ElementActions`] — what the mount
/// handler has in hand (author callbacks + handle routes). The
/// geometry/introspection closures are derived inside
/// [`register_mount`] from [`IntrospectionOps`].
#[derive(Default)]
pub(crate) struct MountActions {
    pub click: Option<Rc<dyn Fn()>>,
    pub set_text: Option<Rc<dyn Fn(String)>>,
    pub set_toggle: Option<Rc<dyn Fn(bool)>>,
    pub set_slider: Option<Rc<dyn Fn(f32)>>,
    pub focus: Option<Rc<dyn Fn()>>,
    pub blur: Option<Rc<dyn Fn()>>,
    pub set_scroll: Option<Rc<dyn Fn(f32, f32)>>,
}

// =============================================================================
// Registry
// =============================================================================

/// A single mounted element tracked by the registry.
pub(crate) struct RegistryEntry {
    pub kind: ElementKind,
    /// Author-assigned stable identifier (via `test_id = …`).
    pub test_id: Option<&'static str>,
    /// Mount-time label snapshot (button/text content). Authoritative
    /// for static sources; a fallback for reactive ones.
    pub label: Option<String>,
    /// Live-label recompute for reactive text/labels. The registry
    /// entry is built once at mount and never re-registered when a
    /// bound signal changes, so a cached string would go stale — the
    /// old core's exact rationale. The closure reads UNTRACKED so a
    /// robot query never subscribes anything.
    pub label_fn: Option<Rc<dyn Fn() -> Option<String>>>,
    pub actions: ElementActions,
    pub parent: Option<ElementId>,
    pub children: Vec<ElementId>,
}

impl RegistryEntry {
    fn current_label(&self) -> Option<String> {
        match &self.label_fn {
            Some(f) => f(),
            None => self.label.clone(),
        }
    }
}

pub(crate) struct Registry {
    entries: Vec<Option<RegistryEntry>>,
    free: Vec<u32>,
    /// test_id → element id. LAST-WINS on duplicates (old-core policy);
    /// `find_all` scans entries instead of reading this 1:1 index.
    by_test_id: HashMap<&'static str, ElementId>,
}

impl Registry {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            free: Vec::new(),
            by_test_id: HashMap::new(),
        }
    }

    fn insert(&mut self, entry: RegistryEntry) -> ElementId {
        let id = if let Some(idx) = self.free.pop() {
            self.entries[idx as usize] = Some(entry);
            ElementId(idx)
        } else {
            let idx = self.entries.len() as u32;
            self.entries.push(Some(entry));
            ElementId(idx)
        };
        if let Some(Some(entry)) = self.entries.get(id.0 as usize) {
            if let Some(test_id) = entry.test_id {
                self.by_test_id.insert(test_id, id);
            }
        }
        id
    }

    fn remove(&mut self, id: ElementId) {
        if let Some(entry) = self.entries.get_mut(id.0 as usize).and_then(|s| s.take()) {
            // Only drop the index mapping if it still points at US — a
            // later duplicate registration (last-wins) may own it now,
            // and removing the older entry must not orphan the live one.
            if let Some(test_id) = entry.test_id {
                if self.by_test_id.get(test_id) == Some(&id) {
                    self.by_test_id.remove(test_id);
                }
            }
            // Unlink from a still-live parent so a recycled slot can't
            // later be walked as this parent's child (the old registry's
            // stale-child-id guard).
            if let Some(pid) = entry.parent {
                if let Some(Some(parent)) = self.entries.get_mut(pid.0 as usize) {
                    parent.children.retain(|c| *c != id);
                }
            }
            self.free.push(id.0);
        }
    }

    fn get(&self, id: ElementId) -> Option<&RegistryEntry> {
        self.entries.get(id.0 as usize).and_then(|s| s.as_ref())
    }
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::new());
    /// Parent stack: [`register_mount`] pushes before the handler
    /// realizes children; the guard pops on drop. Children peek the top
    /// to set `parent`.
    static PARENT_STACK: RefCell<Vec<ElementId>> = const { RefCell::new(Vec::new()) };
}

/// Monotonic change-revision for a live-update push channel — same
/// contract as `runtime_core::robot::current_revision` (the bridge
/// coalesces bursts; register/deregister bump it).
static ROBOT_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn bump_revision() {
    ROBOT_REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The current change-revision (read by a bridge/relay transport).
pub fn current_revision() -> u64 {
    ROBOT_REVISION.load(std::sync::atomic::Ordering::Relaxed)
}

// =============================================================================
// Mount-time registration (the shared helper every handler calls)
// =============================================================================

/// RAII guard returned by [`register_mount`]: keeps the element on the
/// parent stack while the mount handler realizes children, pops on
/// drop (end of the handler body — children are realized by then).
pub(crate) struct RegisteredPrim {
    id: ElementId,
}

impl RegisteredPrim {
    #[cfg(test)]
    pub(crate) fn id(&self) -> ElementId {
        self.id
    }
}

impl Drop for RegisteredPrim {
    fn drop(&mut self) {
        PARENT_STACK.with(|s| {
            let mut stack = s.borrow_mut();
            debug_assert_eq!(
                stack.last(),
                Some(&self.id),
                "robot parent stack out of order — guards must drop LIFO"
            );
            stack.pop();
        });
    }
}

/// Register a just-created primitive node. Call AFTER the backend node
/// exists and BEFORE `realize_children_into` (children link to the top
/// of the parent stack). Wires:
///
/// - the registry entry (kind / test_id / label / actions),
/// - geometry + native-introspection closures over [`IntrospectionOps`]
///   (evaluated lazily at query time — registration itself makes no
///   geometry calls, so scene-parity op goldens are unaffected),
/// - `note_introspection_root` (primitive-boundary marker for native
///   introspection walks — a data-only default no-op),
/// - teardown deregistration via an `on_teardown` probe (dies with the
///   realized subtree),
/// - the parent-stack push (popped by the returned guard's drop).
pub(crate) fn register_mount<H: IntrospectionOps>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    kind: ElementKind,
    test_id: Option<&'static str>,
    label: Option<String>,
    label_fn: Option<Rc<dyn Fn() -> Option<String>>>,
    actions: MountActions,
) -> RegisteredPrim {
    let MountActions {
        click,
        set_text,
        set_toggle,
        set_slider,
        focus,
        blur,
        set_scroll,
    } = actions;
    // Geometry / introspection closures: borrow the backend at QUERY
    // time (never held across the query's own callback), mirroring the
    // old walker's post-dispatch attach closures.
    let frame = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().frame(&n)) as Rc<dyn Fn() -> Option<ViewportRect>>
    };
    let absolute_frame = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().absolute_frame(&n)) as Rc<dyn Fn() -> Option<ViewportRect>>
    };
    let device_frame = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().device_frame(&n)) as Rc<dyn Fn() -> Option<ViewportRect>>
    };
    let introspect = {
        let b = backend.clone();
        let n = node.clone();
        Rc::new(move || b.borrow().introspect_native(&n))
            as Rc<dyn Fn() -> Option<runtime_core::introspect::NativeNode>>
    };
    backend.borrow().note_introspection_root(node);

    let parent = PARENT_STACK.with(|s| s.borrow().last().copied());
    let id = REGISTRY.with(|r| {
        r.borrow_mut().insert(RegistryEntry {
            kind,
            test_id,
            label,
            label_fn,
            actions: ElementActions {
                click,
                set_text,
                set_toggle,
                set_slider,
                focus,
                blur,
                set_scroll,
                frame: Some(frame),
                absolute_frame: Some(absolute_frame),
                device_frame: Some(device_frame),
                introspect: Some(introspect),
            },
            parent,
            children: Vec::new(),
        })
    });
    if let Some(pid) = parent {
        REGISTRY.with(|r| {
            if let Some(Some(entry)) = r.borrow_mut().entries.get_mut(pid.0 as usize) {
                entry.children.push(id);
            }
        });
    }
    bump_revision();

    // Deregister when the realized subtree tears down (branch swap,
    // keyed removal, unmount). Without this, every torn-down branch
    // leaks a phantom entry — the classic stale-registry bug.
    crate::style_attach::on_teardown(move || {
        REGISTRY.with(|r| r.borrow_mut().remove(id));
        bump_revision();
    });

    PARENT_STACK.with(|s| s.borrow_mut().push(id));
    RegisteredPrim { id }
}

// =============================================================================
// Public query/control API (mirrors `runtime_core::robot::Robot`)
// =============================================================================

/// A snapshot of a mounted element's properties.
#[derive(Clone, Debug)]
pub struct Element {
    pub id: ElementId,
    pub kind: ElementKind,
    pub test_id: Option<&'static str>,
    pub label: Option<String>,
}

impl Element {
    fn from_registry(id: ElementId, entry: &RegistryEntry) -> Self {
        Self {
            id,
            kind: entry.kind,
            test_id: entry.test_id,
            label: entry.current_label(),
        }
    }
}

/// Criteria for finding elements (old-core mirror).
#[derive(Clone, Debug)]
pub enum Query {
    /// Match by author-assigned test ID (exact).
    TestId(String),
    /// Match by label text (exact).
    Label(String),
    /// Match by label text (contains substring).
    LabelContains(String),
    /// Match by primitive kind.
    Kind(ElementKind),
    /// Match all elements.
    All,
}

impl Query {
    pub fn test_id(id: impl Into<String>) -> Self {
        Self::TestId(id.into())
    }

    pub fn label(text: impl Into<String>) -> Self {
        Self::Label(text.into())
    }

    pub fn label_contains(text: impl Into<String>) -> Self {
        Self::LabelContains(text.into())
    }

    pub fn kind(kind: ElementKind) -> Self {
        Self::Kind(kind)
    }
}

/// Errors returned by robot control operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RobotError {
    /// The element has been unmounted since it was queried.
    ElementGone,
    /// The requested action is not available on this element kind.
    ActionNotAvailable(&'static str),
    /// No element matched the query.
    NotFound,
}

impl std::fmt::Display for RobotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ElementGone => write!(f, "element has been unmounted"),
            Self::ActionNotAvailable(action) => {
                write!(f, "action '{action}' not available on this element")
            }
            Self::NotFound => write!(f, "no element matched the query"),
        }
    }
}

impl std::error::Error for RobotError {}

/// A recursive snapshot of the mounted tree.
#[derive(Clone, Debug)]
pub struct TreeNode {
    pub id: ElementId,
    pub kind: ElementKind,
    pub test_id: Option<&'static str>,
    pub label: Option<String>,
    pub children: Vec<TreeNode>,
}

/// The programmatic controller for a running new-core UI. Thread-local
/// (same thread as the mount); a ZST indexing the thread-local
/// registry, cheap to construct.
///
/// API mirrors `runtime_core::robot::Robot`'s element surface so the
/// bridge's verb handlers adapt mechanically.
#[derive(Default)]
pub struct Robot;

impl Robot {
    pub fn new() -> Self {
        Self
    }

    /// Find the first element matching `query`. For `TestId`, resolves
    /// via the last-wins index (duplicate policy — see module docs).
    /// Runs entered (label queries resolve `label_fn` — driver env).
    pub fn find(&self, query: Query) -> Option<Element> {
        entered(|| self.find_inner(query))
    }

    fn find_inner(&self, query: Query) -> Option<Element> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            match query {
                Query::TestId(ref id) => {
                    let eid = reg.by_test_id.get(id.as_str()).copied()?;
                    reg.get(eid).map(|e| Element::from_registry(eid, e))
                }
                Query::Label(ref text) => {
                    reg.entries.iter().enumerate().find_map(|(i, slot)| {
                        let entry = slot.as_ref()?;
                        (entry.current_label().as_deref() == Some(text.as_str()))
                            .then(|| Element::from_registry(ElementId(i as u32), entry))
                    })
                }
                Query::LabelContains(ref text) => {
                    reg.entries.iter().enumerate().find_map(|(i, slot)| {
                        let entry = slot.as_ref()?;
                        entry
                            .current_label()
                            .is_some_and(|l| l.contains(text.as_str()))
                            .then(|| Element::from_registry(ElementId(i as u32), entry))
                    })
                }
                Query::Kind(kind) => {
                    reg.entries.iter().enumerate().find_map(|(i, slot)| {
                        let entry = slot.as_ref()?;
                        (entry.kind == kind)
                            .then(|| Element::from_registry(ElementId(i as u32), entry))
                    })
                }
                Query::All => reg.entries.iter().enumerate().find_map(|(i, slot)| {
                    slot.as_ref()
                        .map(|e| Element::from_registry(ElementId(i as u32), e))
                }),
            }
        })
    }

    /// Find all elements matching `query`. For `TestId`, scans ALL
    /// entries (not the 1:1 index) so duplicates are all returned.
    /// Runs entered (label queries resolve `label_fn` — driver env).
    pub fn find_all(&self, query: Query) -> Vec<Element> {
        entered(|| self.find_all_inner(query))
    }

    fn find_all_inner(&self, query: Query) -> Vec<Element> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            reg.entries
                .iter()
                .enumerate()
                .filter_map(|(i, slot)| {
                    let entry = slot.as_ref()?;
                    let matched = match &query {
                        Query::TestId(id) => entry.test_id == Some(id.as_str()),
                        Query::Label(text) => {
                            entry.current_label().as_deref() == Some(text.as_str())
                        }
                        Query::LabelContains(text) => entry
                            .current_label()
                            .is_some_and(|l| l.contains(text.as_str())),
                        Query::Kind(kind) => entry.kind == *kind,
                        Query::All => true,
                    };
                    matched.then(|| Element::from_registry(ElementId(i as u32), entry))
                })
                .collect()
        })
    }

    /// Count mounted elements, optionally filtered by kind.
    pub fn count(&self, kind: Option<ElementKind>) -> usize {
        match kind {
            Some(k) => self.find_all(Query::Kind(k)).len(),
            None => self.find_all(Query::All).len(),
        }
    }

    /// Simulate a click/press.
    pub fn click(&self, element: &Element) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a.click.clone().ok_or(RobotError::ActionNotAvailable("click"))?;
            f();
            Ok(())
        })
    }

    /// Replace a text input's value (paste-like).
    pub fn type_text(&self, element: &Element, text: &str) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a
                .set_text
                .clone()
                .ok_or(RobotError::ActionNotAvailable("type_text"))?;
            f(text.to_string());
            Ok(())
        })
    }

    /// Set a toggle's value.
    pub fn set_toggle(&self, element: &Element, value: bool) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a
                .set_toggle
                .clone()
                .ok_or(RobotError::ActionNotAvailable("set_toggle"))?;
            f(value);
            Ok(())
        })
    }

    /// Set a slider's value.
    pub fn set_slider(&self, element: &Element, value: f32) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a
                .set_slider
                .clone()
                .ok_or(RobotError::ActionNotAvailable("set_slider"))?;
            f(value);
            Ok(())
        })
    }

    /// Focus an element (text inputs).
    pub fn focus(&self, element: &Element) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a.focus.clone().ok_or(RobotError::ActionNotAvailable("focus"))?;
            f();
            Ok(())
        })
    }

    /// Blur (unfocus) an element.
    pub fn blur(&self, element: &Element) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a.blur.clone().ok_or(RobotError::ActionNotAvailable("blur"))?;
            f();
            Ok(())
        })
    }

    /// Set a scroll view's offset (logical px/points).
    pub fn set_scroll(&self, element: &Element, x: f32, y: f32) -> Result<(), RobotError> {
        self.with_actions(element.id, |a| {
            let f = a
                .set_scroll
                .clone()
                .ok_or(RobotError::ActionNotAvailable("set_scroll"))?;
            f(x, y);
            Ok(())
        })
    }

    /// The element's rect in its parent's coordinate system.
    pub fn frame(&self, element: &Element) -> Result<Option<ViewportRect>, RobotError> {
        self.geometry(element, |a| a.frame.clone(), "frame")
    }

    /// The element's rect in viewport/window coordinates.
    pub fn absolute_frame(
        &self,
        element: &Element,
    ) -> Result<Option<ViewportRect>, RobotError> {
        self.geometry(element, |a| a.absolute_frame.clone(), "absolute_frame")
    }

    /// The element's rect in physical device-screen pixels.
    pub fn device_frame(
        &self,
        element: &Element,
    ) -> Result<Option<ViewportRect>, RobotError> {
        self.geometry(element, |a| a.device_frame.clone(), "device_frame")
    }

    /// The element's platform-native render tree (parity surface).
    pub fn introspect_native(
        &self,
        element: &Element,
    ) -> Result<Option<runtime_core::introspect::NativeNode>, RobotError> {
        let cb = REGISTRY.with(|r| {
            r.borrow()
                .get(element.id)
                .map(|e| e.actions.introspect.clone())
                .ok_or(RobotError::ElementGone)
        })?;
        match cb {
            Some(cb) => Ok(cb()),
            None => Err(RobotError::ActionNotAvailable("introspect_native")),
        }
    }

    /// Flat list of all mounted elements.
    pub fn elements(&self) -> Vec<Element> {
        self.find_all(Query::All)
    }

    /// Recursive snapshot. An entry whose parent is no longer live (a
    /// dead/recycled slot) surfaces as a ROOT rather than vanishing —
    /// the old registry's orphan rule (content mounted late inside
    /// driver effects registers with a dead or absent parent and must
    /// stay reachable). Runs entered (labels resolve — driver env).
    pub fn snapshot(&self) -> Vec<TreeNode> {
        entered(|| self.snapshot_inner())
    }

    fn snapshot_inner(&self) -> Vec<TreeNode> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            let roots: Vec<ElementId> = reg
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, slot)| {
                    let entry = slot.as_ref()?;
                    let is_root = match entry.parent {
                        None => true,
                        Some(pid) => reg.get(pid).is_none(),
                    };
                    is_root.then_some(ElementId(i as u32))
                })
                .collect();
            roots
                .iter()
                .filter_map(|&id| Self::build_tree_node(&reg, id))
                .collect()
        })
    }

    /// Direct children of an element. Runs entered (labels resolve).
    pub fn children_of(&self, element: &Element) -> Vec<Element> {
        entered(|| self.children_of_inner(element))
    }

    fn children_of_inner(&self, element: &Element) -> Vec<Element> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            reg.get(element.id)
                .map(|entry| {
                    entry
                        .children
                        .iter()
                        .filter_map(|&cid| reg.get(cid).map(|e| Element::from_registry(cid, e)))
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// Parent of an element. Runs entered (labels resolve).
    pub fn parent_of(&self, element: &Element) -> Option<Element> {
        entered(|| self.parent_of_inner(element))
    }

    fn parent_of_inner(&self, element: &Element) -> Option<Element> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            let pid = reg.get(element.id)?.parent?;
            reg.get(pid).map(|e| Element::from_registry(pid, e))
        })
    }

    /// Clear the registry (test isolation; registries are thread-local,
    /// so parallel `#[test]` threads never share entries).
    pub fn reset(&self) {
        REGISTRY.with(|r| *r.borrow_mut() = Registry::new());
        PARENT_STACK.with(|s| s.borrow_mut().clear());
    }

    fn build_tree_node(reg: &Registry, id: ElementId) -> Option<TreeNode> {
        let entry = reg.get(id)?;
        let children = entry
            .children
            .iter()
            .filter_map(|&cid| Self::build_tree_node(reg, cid))
            .collect();
        Some(TreeNode {
            id,
            kind: entry.kind,
            test_id: entry.test_id,
            label: entry.current_label(),
            children,
        })
    }

    fn geometry(
        &self,
        element: &Element,
        pick: impl FnOnce(&ElementActions) -> Option<Rc<dyn Fn() -> Option<ViewportRect>>>,
        name: &'static str,
    ) -> Result<Option<ViewportRect>, RobotError> {
        let cb = REGISTRY.with(|r| {
            r.borrow()
                .get(element.id)
                .map(|e| pick(&e.actions))
                .ok_or(RobotError::ElementGone)
        })?;
        match cb {
            Some(cb) => Ok(cb()),
            None => Err(RobotError::ActionNotAvailable(name)),
        }
    }

    /// Clone the actions out, DROP the registry borrow, then invoke —
    /// an action (e.g. a click handler) may mount/unmount subtrees,
    /// which re-enters the registry; a held borrow would abort with
    /// "RefCell already borrowed" (the old registry's exact guard).
    ///
    /// The author callback runs OUTSIDE the world (its handle-routed
    /// writes stage, exactly like a real platform event), then
    /// [`settle`] commits the staged work synchronously so a query on
    /// the next line observes the post-action tree — the driver-env
    /// contract (see `DriverEnv`).
    fn with_actions(
        &self,
        id: ElementId,
        f: impl FnOnce(&ElementActions) -> Result<(), RobotError>,
    ) -> Result<(), RobotError> {
        let actions = REGISTRY.with(|r| {
            r.borrow()
                .get(id)
                .map(|e| e.actions.clone())
                .ok_or(RobotError::ElementGone)
        })?;
        let out = f(&actions);
        settle();
        out
    }
}

// =============================================================================
// Driver environment — the transport/harness seam (conformance wave)
// =============================================================================

/// The per-boot environment a NEW-core host installs so foreign drivers
/// (the bridge/relay verb loop, the in-process `robot-e2e` harness) can
/// use [`Robot`] without knowing the backend:
///
/// - `enter`: run a closure with the app's [`runtime_world::World`]
///   entered. Label-resolving queries ([`RegistryEntry::label_fn`]) read
///   world signals, and new-core signal reads need the ambient world —
///   drivers run outside it (relay `onmessage`, scheduled harness
///   steps). `World::enter` nests, so wrapping an already-entered caller
///   is harmless.
/// - `settle`: synchronously commit staged reactive work (the web
///   host's `flush_sync`). Robot actions invoke the RAW author callback
///   (pre dispatch-site flush glue), so without an explicit settle the
///   staged writes would only commit on the next flush microtask —
///   breaking the harness's synchronous `act → assert` contract that
///   the old core satisfied by applying writes synchronously.
///
/// [`Robot`] consults the installed env itself (queries run entered,
/// actions settle after dispatch), so callers stay core-agnostic.
/// Without an installed env both hooks are identity — unit tests that
/// drive the registry directly need no world.
struct DriverEnv {
    enter: Rc<dyn Fn(&mut dyn FnMut())>,
    settle: Rc<dyn Fn()>,
}

thread_local! {
    static DRIVER_ENV: RefCell<Option<DriverEnv>> = const { RefCell::new(None) };
}

/// Install the driver environment (see [`DriverEnv`] docs above).
/// Called once by the new-core host boot (e.g.
/// `backend_web::newcore::start`). Re-installing replaces the prior env.
pub fn install_driver_env(
    enter: impl Fn(&mut dyn FnMut()) + 'static,
    settle: impl Fn() + 'static,
) {
    DRIVER_ENV.with(|slot| {
        *slot.borrow_mut() = Some(DriverEnv {
            enter: Rc::new(enter),
            settle: Rc::new(settle),
        })
    });
}

/// Remove the installed driver environment (host teardown / tests).
pub fn clear_driver_env() {
    DRIVER_ENV.with(|slot| *slot.borrow_mut() = None);
}

/// Run `f` with the app's world entered when an env is installed
/// (identity otherwise). The env `Rc` is cloned out before the call so
/// `f` may itself touch the env slot.
fn entered<R>(f: impl FnOnce() -> R) -> R {
    let enter = DRIVER_ENV.with(|slot| slot.borrow().as_ref().map(|e| e.enter.clone()));
    match enter {
        Some(enter) => {
            let mut f = Some(f);
            let mut out: Option<R> = None;
            enter(&mut || {
                if let Some(f) = f.take() {
                    out = Some(f());
                }
            });
            out.expect("robot driver env `enter` must invoke the closure it is given")
        }
        None => f(),
    }
}

/// Synchronously commit staged reactive work via the installed env
/// (no-op without one). Robot actions call this after dispatching the
/// author callback; harnesses may call it after writing signals
/// directly.
pub fn settle() {
    let settle = DRIVER_ENV.with(|slot| slot.borrow().as_ref().map(|e| e.settle.clone()));
    if let Some(settle) = settle {
        settle();
    }
}

// =============================================================================
// Bridge verb dispatch — the transport half of the old
// `runtime_core::robot::bridge::dispatch`, adapted 1:1 to THIS registry
// =============================================================================

/// JSON verb dispatch for bridge/relay transports. Mirrors the element
/// surface of `runtime_core::robot::bridge::dispatch` verb-for-verb and
/// byte-for-byte on the wire (same `cmd` names, same argument shapes,
/// same response JSON), so the MCP server / relay / `robot-test` client
/// drive a new-core app unchanged.
///
/// Verbs owned by still-deferred seams (`invoke_method` /
/// `list_components` → `#[method]` P5; `list_navigators` /
/// `get_navigator_state` → robot nav registry P5; watched signals →
/// `watch_signal` P5) return a **named error** rather than an empty
/// success, so a harness failure points at the seam, not at a phantom
/// empty registry. Verbs the registry doesn't own at all (`get_logs`,
/// `screenshot`, custom commands) return `unknown command: …` — the
/// transport falls back to its platform path for those (the web relay
/// client routes them to the old bridge dispatch, whose log/custom
/// machinery is registry-independent).
pub mod bridge {
    use super::*;
    use runtime_core::__serde_json as serde_json;

    /// Invoke one bridge verb against the new-core registry. Must run on
    /// the UI thread (registry + driver env are thread-local). Queries
    /// run inside the installed driver env's `enter`; actions settle.
    pub fn invoke_command(cmd: &str, args: &serde_json::Value) -> Result<String, String> {
        let robot = Robot::new();
        match cmd {
            "ping" => Ok("\"pong\"".into()),
            "find_element" => {
                let query = parse_query(args)?;
                match robot.find(query) {
                    Some(el) => Ok(element_json(&el)),
                    None => Ok("null".into()),
                }
            }
            "find_all_elements" => {
                let query = parse_query(args)?;
                let els: Vec<String> = robot.find_all(query).iter().map(element_json).collect();
                Ok(format!("[{}]", els.join(",")))
            }
            "click" => {
                let el = resolve_element(args)?;
                robot.click(&el).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "type_text" => {
                let el = resolve_element(args)?;
                let text = args["text"].as_str().ok_or("missing 'text' argument")?;
                robot.type_text(&el, text).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "set_toggle" => {
                let el = resolve_element(args)?;
                let value = args["value"].as_bool().ok_or("missing 'value' argument")?;
                robot.set_toggle(&el, value).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "set_slider" => {
                let el = resolve_element(args)?;
                let value = args["value"].as_f64().ok_or("missing 'value' argument")? as f32;
                robot.set_slider(&el, value).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "set_scroll" => {
                let el = resolve_element(args)?;
                let x = args["x"].as_f64().unwrap_or(0.0) as f32;
                let y = args["y"].as_f64().unwrap_or(0.0) as f32;
                robot.set_scroll(&el, x, y).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "focus" => {
                let el = resolve_element(args)?;
                robot.focus(&el).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "blur" => {
                let el = resolve_element(args)?;
                robot.blur(&el).map_err(|e| e.to_string())?;
                Ok("\"ok\"".into())
            }
            "get_snapshot" => {
                let tree = robot.snapshot();
                let nodes: Vec<String> = tree.iter().map(tree_node_json).collect();
                Ok(format!("[{}]", nodes.join(",")))
            }
            "get_children" => {
                let el = resolve_element(args)?;
                let children: Vec<String> =
                    robot.children_of(&el).iter().map(element_json).collect();
                Ok(format!("[{}]", children.join(",")))
            }
            "get_parent" => {
                let el = resolve_element(args)?;
                match robot.parent_of(&el) {
                    Some(p) => Ok(element_json(&p)),
                    None => Ok("null".into()),
                }
            }
            "count_elements" => {
                let kind = args["kind"].as_str().and_then(parse_element_kind);
                Ok(robot.count(kind).to_string())
            }
            "get_frame" => {
                let el = resolve_element(args)?;
                rect_json(robot.frame(&el).map_err(|e| e.to_string())?)
            }
            "get_absolute_frame" => {
                let el = resolve_element(args)?;
                rect_json(robot.absolute_frame(&el).map_err(|e| e.to_string())?)
            }
            "get_device_frame" => {
                let el = resolve_element(args)?;
                rect_json(robot.device_frame(&el).map_err(|e| e.to_string())?)
            }
            "introspect_native" => {
                let el = resolve_element(args)?;
                match robot.introspect_native(&el).map_err(|e| e.to_string())? {
                    Some(node) => serde_json::to_string(&node)
                        .map_err(|e| format!("failed to serialize native tree: {e}")),
                    None => Ok("null".into()),
                }
            }
            // Deferred seams: fail with the seam's name (module docs).
            "invoke_method" | "list_components" => Err(format!(
                "{cmd}: `#[method]` components are not yet available on the new core \
                 (idea-lite migration, P5 seam)"
            )),
            "list_navigators" | "get_navigator_state" => Err(format!(
                "{cmd}: the robot navigator registry is not yet available on the new \
                 core (idea-lite migration, P5 seam)"
            )),
            "list_watched_signals" | "read_signal" => Err(format!(
                "{cmd}: `watch_signal` is not yet available on the new core \
                 (idea-lite migration, P5 seam)"
            )),
            _ => Err(format!("unknown command: {cmd}")),
        }
    }

    fn parse_query(args: &serde_json::Value) -> Result<Query, String> {
        if let Some(id) = args["test_id"].as_str() {
            Ok(Query::TestId(id.to_string()))
        } else if let Some(label) = args["label"].as_str() {
            Ok(Query::Label(label.to_string()))
        } else if let Some(sub) = args["label_contains"].as_str() {
            Ok(Query::LabelContains(sub.to_string()))
        } else if let Some(kind_str) = args["kind"].as_str() {
            match parse_element_kind(kind_str) {
                Some(k) => Ok(Query::Kind(k)),
                None => Err(format!("unknown element kind: {kind_str}")),
            }
        } else {
            Ok(Query::All)
        }
    }

    fn parse_element_kind(s: &str) -> Option<ElementKind> {
        match s {
            "View" => Some(ElementKind::View),
            "Text" => Some(ElementKind::Text),
            "Button" => Some(ElementKind::Button),
            "Pressable" => Some(ElementKind::Pressable),
            "Image" => Some(ElementKind::Image),
            "Icon" => Some(ElementKind::Icon),
            "TextInput" => Some(ElementKind::TextInput),
            "Toggle" => Some(ElementKind::Toggle),
            "ScrollView" => Some(ElementKind::ScrollView),
            "Slider" => Some(ElementKind::Slider),
            "ActivityIndicator" => Some(ElementKind::ActivityIndicator),
            "Virtualizer" => Some(ElementKind::Virtualizer),
            "Graphics" => Some(ElementKind::Graphics),
            "Navigator" => Some(ElementKind::Navigator),
            "Link" => Some(ElementKind::Link),
            "Overlay" => Some(ElementKind::Overlay),
            "Presence" => Some(ElementKind::Presence),
            _ => None,
        }
    }

    fn resolve_element(args: &serde_json::Value) -> Result<Element, String> {
        let id = args["element_id"]
            .as_u64()
            .ok_or("missing 'element_id' argument")?;
        Ok(Element {
            id: ElementId(id as u32),
            kind: ElementKind::View,
            test_id: None,
            label: None,
        })
    }

    fn rect_json(rect: Option<ViewportRect>) -> Result<String, String> {
        Ok(match rect {
            Some(r) => format!(
                "{{\"x\":{},\"y\":{},\"width\":{},\"height\":{}}}",
                r.x, r.y, r.width, r.height
            ),
            None => "null".into(),
        })
    }

    fn element_json(el: &Element) -> String {
        format!(
            "{{\"id\":{},\"kind\":\"{:?}\",\"test_id\":{},\"label\":{}}}",
            el.id.0,
            el.kind,
            el.test_id
                .map(|t| serde_json::to_string(t).unwrap_or_else(|_| "null".into()))
                .unwrap_or_else(|| "null".into()),
            el.label
                .as_deref()
                .map(|l| serde_json::to_string(l).unwrap_or_else(|_| "null".into()))
                .unwrap_or_else(|| "null".into()),
        )
    }

    fn tree_node_json(node: &TreeNode) -> String {
        let children: Vec<String> = node.children.iter().map(tree_node_json).collect();
        format!(
            "{{\"id\":{},\"kind\":\"{:?}\",\"test_id\":{},\"label\":{},\"children\":[{}]}}",
            node.id.0,
            node.kind,
            node.test_id
                .map(|t| serde_json::to_string(t).unwrap_or_else(|_| "null".into()))
                .unwrap_or_else(|| "null".into()),
            node.label
                .as_deref()
                .map(|l| serde_json::to_string(l).unwrap_or_else(|_| "null".into()))
                .unwrap_or_else(|| "null".into()),
            children.join(","),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(test_id: &'static str) -> RegistryEntry {
        RegistryEntry {
            kind: ElementKind::Text,
            test_id: Some(test_id),
            label: None,
            label_fn: None,
            actions: ElementActions::default(),
            parent: None,
            children: Vec::new(),
        }
    }

    /// Duplicate policy pin (old-core mirror): `find` is last-wins via
    /// the index; `find_all` returns every holder.
    #[test]
    fn duplicate_test_id_last_wins_for_find_all_for_find_all() {
        let robot = Robot::new();
        robot.reset();
        let first = REGISTRY.with(|r| r.borrow_mut().insert(entry("dup")));
        let second = REGISTRY.with(|r| r.borrow_mut().insert(entry("dup")));
        assert_eq!(robot.find(Query::test_id("dup")).unwrap().id, second);
        assert_eq!(robot.find_all(Query::test_id("dup")).len(), 2);
        // Removing the OLDER duplicate must not orphan the index entry
        // the newer one owns.
        REGISTRY.with(|r| r.borrow_mut().remove(first));
        assert_eq!(robot.find(Query::test_id("dup")).unwrap().id, second);
        robot.reset();
    }

    /// The orphan rule: a dead parent surfaces its child as a root.
    #[test]
    fn snapshot_surfaces_orphan_with_dead_parent() {
        let robot = Robot::new();
        robot.reset();
        let parent = REGISTRY.with(|r| r.borrow_mut().insert(entry("p")));
        let child = REGISTRY.with(|r| {
            r.borrow_mut().insert(RegistryEntry {
                parent: Some(parent),
                ..entry("orphan-child")
            })
        });
        // Kill just the parent slot; the child keeps its stale parent id.
        REGISTRY.with(|r| {
            r.borrow_mut().entries[parent.0 as usize] = None;
        });
        let tree = robot.snapshot();
        assert!(
            tree.iter().any(|n| n.id == child && n.test_id == Some("orphan-child")),
            "orphan must surface as a root: {tree:?}"
        );
        robot.reset();
    }

    /// Bridge-dispatch round trip: the relay verb loop's exact path —
    /// `find_element` (by test_id) → `click` (by the returned
    /// element_id) → the author callback fired. Wire shapes mirror the
    /// old `runtime_core::robot::bridge` responses.
    #[test]
    fn bridge_invoke_command_find_then_click() {
        use runtime_core::__serde_json::json;
        use std::cell::Cell;

        let robot = Robot::new();
        robot.reset();
        let clicks: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let clicks_in = clicks.clone();
        let id = REGISTRY.with(|r| {
            r.borrow_mut().insert(RegistryEntry {
                kind: ElementKind::Button,
                test_id: Some("inc"),
                label: Some("Increment".into()),
                actions: ElementActions {
                    click: Some(Rc::new(move || clicks_in.set(clicks_in.get() + 1))),
                    ..ElementActions::default()
                },
                ..RegistryEntry::default()
            })
        });

        let found = bridge::invoke_command("find_element", &json!({"test_id": "inc"}))
            .expect("find_element");
        assert_eq!(
            found,
            format!(
                "{{\"id\":{},\"kind\":\"Button\",\"test_id\":\"inc\",\"label\":\"Increment\"}}",
                id.0
            )
        );

        let ok = bridge::invoke_command("click", &json!({"element_id": id.0}))
            .expect("click dispatches");
        assert_eq!(ok, "\"ok\"");
        assert_eq!(clicks.get(), 1, "author callback fired through the verb loop");

        // Unknown verbs surface as the fallback marker the transport
        // keys on; deferred seams surface their phase by name.
        let unknown = bridge::invoke_command("get_logs", &json!({})).unwrap_err();
        assert!(unknown.starts_with("unknown command:"), "{unknown}");
        let deferred = bridge::invoke_command("invoke_method", &json!({})).unwrap_err();
        assert!(deferred.contains("P5"), "{deferred}");
        robot.reset();
    }

    /// The driver env wraps queries (`enter`) and actions (`settle`) —
    /// the transport contract. A fake env counts both.
    #[test]
    fn driver_env_enters_queries_and_settles_actions() {
        use std::cell::Cell;

        let robot = Robot::new();
        robot.reset();
        let enters: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let settles: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        {
            let enters = enters.clone();
            let settles = settles.clone();
            install_driver_env(
                move |f| {
                    enters.set(enters.get() + 1);
                    f();
                },
                move || settles.set(settles.get() + 1),
            );
        }
        let id = REGISTRY.with(|r| {
            r.borrow_mut().insert(RegistryEntry {
                kind: ElementKind::Button,
                test_id: Some("env-btn"),
                actions: ElementActions {
                    click: Some(Rc::new(|| {})),
                    ..ElementActions::default()
                },
                ..RegistryEntry::default()
            })
        });
        let el = robot.find(Query::test_id("env-btn")).expect("found");
        assert_eq!(el.id, id);
        assert_eq!(enters.get(), 1, "query ran inside env.enter");
        robot.click(&el).expect("click");
        assert_eq!(settles.get(), 1, "action settled staged work");
        clear_driver_env();
        robot.reset();
    }
}

// Struct-update syntax for the test helper above.
#[cfg(test)]
impl Default for RegistryEntry {
    fn default() -> Self {
        Self {
            kind: ElementKind::View,
            test_id: None,
            label: None,
            label_fn: None,
            actions: ElementActions::default(),
            parent: None,
            children: Vec::new(),
        }
    }
}
