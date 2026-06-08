//! Programmatic introspection and control of a running UI.
//!
//! The `robot` module is the framework's automation surface. It gives
//! external controllers — E2E test harnesses, MCP servers, AI agents —
//! a single API to:
//!
//! - **Query** the mounted UI tree (find elements by test ID, label,
//!   primitive kind).
//! - **Control** the app (click buttons, fill text inputs, toggle
//!   switches, navigate, read/write signals).
//! - **Introspect** reactive state (read signal values, check ref
//!   liveness, dump arena stats).
//!
//! # Architecture
//!
//! When the `robot` feature is enabled, the framework's render walker
//! registers every mounted primitive into a thread-local **Registry**.
//! Each entry records:
//! - the primitive kind,
//! - an optional `test_id` (author-assigned stable identifier),
//! - an optional label (for buttons/text),
//! - an opaque handle for control actions.
//!
//! The [`Robot`] struct is the user-facing API. It queries the registry
//! and dispatches control commands through the stored handles.
//!
//! # Usage
//!
//! ```ignore
//! use runtime_core::robot::{Robot, Query};
//!
//! let robot = Robot::new();
//!
//! // Find and click a button
//! let btn = robot.find(Query::test_id("submit-btn")).unwrap();
//! robot.click(&btn);
//!
//! // Read a signal's value
//! let count: i32 = robot.read_signal(count_signal);
//!
//! // Type into a text input
//! let input = robot.find(Query::test_id("email-input")).unwrap();
//! robot.type_text(&input, "user@example.com");
//!
//! // Dump the tree
//! let snapshot = robot.snapshot();
//! ```
//!
//! # Feature gate
//!
//! This module is only compiled when the `robot` Cargo feature is
//! enabled. Production builds should leave it off — the registry adds
//! per-node overhead and retains references that would otherwise be
//! freed by scope drops.

pub mod bridge;
pub mod components;
pub mod logs;
pub mod screenshot;
pub mod watch;

pub use components::{
    component_for_element, invoke_method, list_components, register_component, ComponentInstanceId,
    ComponentRegistration, ComponentSnapshot, Method,
};
// Walk-time linkage helpers — crate-internal (the walker arms/consumes them).
pub(crate) use components::{
    link_component_element, set_pending_component_link, take_pending_component_link,
};
pub use watch::watch_signal;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::reactive::{Signal, Ref};

// =============================================================================
// Element kinds (mirrors Element variants at a coarser level)
// =============================================================================

/// Identifies what kind of primitive an element was built from.
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
    TabNavigator,
    DrawerNavigator,
    Link,
    Overlay,
    Presence,
}

// =============================================================================
// ElementId — stable handle into the registry
// =============================================================================

/// Opaque identifier for a mounted element in the registry. Cheap to
/// copy and compare. Invalid after the element's scope drops (the
/// registry entry is removed).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ElementId(pub u32);

// =============================================================================
// Control handles — type-erased actions the robot can perform
// =============================================================================

/// Type-erased action dispatch table for a mounted element. Each
/// variant is populated at registration time based on the primitive
/// kind and what handles/callbacks the walker has available.
#[derive(Clone)]
pub(crate) struct ElementActions {
    /// Trigger a click/press action (buttons, pressables, links).
    pub click: Option<Rc<dyn Fn()>>,
    /// Set text value (text inputs). The closure calls `on_change`
    /// with the new value, simulating user input.
    pub set_text: Option<Rc<dyn Fn(String)>>,
    /// Set toggle value.
    pub set_toggle: Option<Rc<dyn Fn(bool)>>,
    /// Set slider value.
    pub set_slider: Option<Rc<dyn Fn(f32)>>,
    /// Focus the element (text inputs).
    pub focus: Option<Rc<dyn Fn()>>,
    /// Blur the element.
    pub blur: Option<Rc<dyn Fn()>>,
    /// Read the element's rect in **parent** coordinates. Wraps
    /// `Backend::frame`. Returns `None` if the node isn't mounted in
    /// a layout yet.
    pub frame: Option<Rc<dyn Fn() -> Option<crate::primitives::portal::ViewportRect>>>,
    /// Read the element's rect in **viewport/window** coordinates.
    /// Wraps `Backend::absolute_frame`.
    pub absolute_frame: Option<Rc<dyn Fn() -> Option<crate::primitives::portal::ViewportRect>>>,
    /// Read the element's rect in **physical device-screen pixels** for
    /// OS-level input injection. Wraps `Backend::device_frame`.
    pub device_frame: Option<Rc<dyn Fn() -> Option<crate::primitives::portal::ViewportRect>>>,
}

impl ElementActions {
    pub(crate) fn empty() -> Self {
        Self {
            click: None,
            set_text: None,
            set_toggle: None,
            set_slider: None,
            focus: None,
            blur: None,
            frame: None,
            absolute_frame: None,
            device_frame: None,
        }
    }
}

// =============================================================================
// Registry entry
// =============================================================================

/// A single mounted element tracked by the robot registry.
#[allow(dead_code)]
pub(crate) struct RegistryEntry {
    pub kind: ElementKind,
    /// Author-assigned stable identifier (via `.test_id("...")`).
    pub test_id: Option<&'static str>,
    /// Human-readable label. For buttons this is the initial label
    /// text; for text nodes the content; for others `None`. For
    /// reactive text/labels this holds the *mount-time* value and is
    /// only a fallback — `label_fn` recomputes the live value.
    pub label: Option<String>,
    /// Recompute the current label on demand. `Some` only for reactive
    /// text/labels (`TextSource::Bound` / `JsBinding`); `None` for
    /// static labels, where `label` is authoritative.
    ///
    /// Why this exists: the registry entry is built once at walker
    /// registration and is NOT re-registered when a bound signal
    /// changes (the reactive Effect updates the backend's view, not the
    /// robot registry). Caching the string at registration made
    /// `get_snapshot` / `find(Label)` report stale text after the UI
    /// updated — see `regression_robot_snapshot_reflects_reactive_text`.
    /// Resolving lazily at query time keeps the introspection surface
    /// honest without hooking the reactive update path.
    pub label_fn: Option<Rc<dyn Fn() -> Option<String>>>,
    /// Actions available on this element.
    pub actions: ElementActions,
    /// Parent element, if any.
    pub parent: Option<ElementId>,
    /// Children elements.
    pub children: Vec<ElementId>,
}

impl RegistryEntry {
    /// The label to report right now: the live value from `label_fn`
    /// when this is a reactive label, else the cached static `label`.
    pub(crate) fn current_label(&self) -> Option<String> {
        match &self.label_fn {
            Some(f) => f(),
            None => self.label.clone(),
        }
    }
}

// =============================================================================
// Registry (thread-local)
// =============================================================================

pub(crate) struct Registry {
    entries: Vec<Option<RegistryEntry>>,
    free: Vec<u32>,
    /// Index: test_id → element id.
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

    pub(crate) fn insert(&mut self, entry: RegistryEntry) -> ElementId {
        let id = if let Some(idx) = self.free.pop() {
            self.entries[idx as usize] = Some(entry);
            ElementId(idx)
        } else {
            let idx = self.entries.len() as u32;
            self.entries.push(Some(entry));
            ElementId(idx)
        };

        // Update secondary index.
        if let Some(Some(entry)) = self.entries.get(id.0 as usize) {
            if let Some(test_id) = entry.test_id {
                self.by_test_id.insert(test_id, id);
            }
        }

        id
    }

    pub(crate) fn remove(&mut self, id: ElementId) {
        if let Some(entry) = self.entries.get_mut(id.0 as usize).and_then(|s| s.take()) {
            if let Some(test_id) = entry.test_id {
                self.by_test_id.remove(test_id);
            }
            // Unlink from the parent's children list so a still-live
            // parent (e.g. a `View` wrapping a reactive `when` region)
            // doesn't keep a dangling id that a recycled slot could later
            // re-point at a different element. Without this, `snapshot()`
            // / `children_of` would walk a stale child id whose freed slot
            // was reused by an unrelated registration.
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

    fn find_by_test_id(&self, test_id: &str) -> Option<ElementId> {
        self.by_test_id.get(test_id).copied()
    }

    fn find_by_label(&self, label: &str) -> Option<ElementId> {
        self.entries.iter().flatten().enumerate().find_map(|(i, e)| {
            if e.current_label().as_deref() == Some(label) {
                Some(ElementId(i as u32))
            } else {
                None
            }
        })
    }

    fn find_all_by_kind(&self, kind: ElementKind) -> Vec<ElementId> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                slot.as_ref()
                    .filter(|e| e.kind == kind)
                    .map(|_| ElementId(i as u32))
            })
            .collect()
    }

    fn all_elements(&self) -> Vec<ElementId> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|_| ElementId(i as u32)))
            .collect()
    }
}

thread_local! {
    pub(crate) static REGISTRY: RefCell<Registry> = RefCell::new(Registry::new());
    /// Stack of parent element IDs. The walker pushes before building
    /// children and pops after. Children peek the top to set their
    /// `parent` field.
    static PARENT_STACK: RefCell<Vec<ElementId>> = RefCell::new(Vec::new());
}

// =============================================================================
// Public API: Element (read-only view of a registry entry)
// =============================================================================

/// A snapshot of a mounted element's properties. Returned by queries
/// on [`Robot`]. The element may have been unmounted by the time you
/// use it — control methods return `Result` to handle this case.
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

// =============================================================================
// Query
// =============================================================================

/// Criteria for finding elements in the mounted tree.
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

// =============================================================================
// RobotError
// =============================================================================

/// Errors returned by robot control operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RobotError {
    /// The element has been unmounted since it was queried.
    ElementGone,
    /// The requested action is not available on this element kind
    /// (e.g. clicking a text node).
    ActionNotAvailable(&'static str),
    /// No element matched the query.
    NotFound,
}

impl std::fmt::Display for RobotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ElementGone => write!(f, "element has been unmounted"),
            Self::ActionNotAvailable(action) => {
                write!(f, "action '{}' not available on this element", action)
            }
            Self::NotFound => write!(f, "no element matched the query"),
        }
    }
}

impl std::error::Error for RobotError {}

// =============================================================================
// TreeNode — recursive snapshot of the UI tree
// =============================================================================

/// A recursive snapshot of the mounted UI tree, useful for debugging
/// and assertions in tests.
#[derive(Clone, Debug)]
pub struct TreeNode {
    pub id: ElementId,
    pub kind: ElementKind,
    pub test_id: Option<&'static str>,
    pub label: Option<String>,
    pub children: Vec<TreeNode>,
}

// =============================================================================
// Robot — the main API
// =============================================================================

/// The programmatic controller for a running UI. Thread-local — must
/// be used on the same thread as the render call.
///
/// `Robot` is cheap to construct (it's a ZST that indexes into the
/// thread-local registry). Multiple `Robot` instances are fine.
pub struct Robot;

impl Robot {
    /// Create a new robot controller.
    pub fn new() -> Self {
        Self
    }

    // -------------------------------------------------------------------------
    // Queries
    // -------------------------------------------------------------------------

    /// Find the first element matching `query`. Returns `None` if no
    /// match is found.
    pub fn find(&self, query: Query) -> Option<Element> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            match query {
                Query::TestId(ref id) => {
                    let eid = reg.find_by_test_id(id)?;
                    reg.get(eid).map(|e| Element::from_registry(eid, e))
                }
                Query::Label(ref text) => {
                    let eid = reg.find_by_label(text)?;
                    reg.get(eid).map(|e| Element::from_registry(eid, e))
                }
                Query::LabelContains(ref text) => {
                    reg.entries.iter().enumerate().find_map(|(i, slot)| {
                        let entry = slot.as_ref()?;
                        if entry.current_label().map_or(false, |l| l.contains(text.as_str())) {
                            Some(Element::from_registry(ElementId(i as u32), entry))
                        } else {
                            None
                        }
                    })
                }
                Query::Kind(kind) => {
                    let ids = reg.find_all_by_kind(kind);
                    ids.first()
                        .and_then(|&id| reg.get(id).map(|e| Element::from_registry(id, e)))
                }
                Query::All => {
                    reg.entries.iter().enumerate().find_map(|(i, slot)| {
                        slot.as_ref()
                            .map(|e| Element::from_registry(ElementId(i as u32), e))
                    })
                }
            }
        })
    }

    /// Find all elements matching `query`.
    pub fn find_all(&self, query: Query) -> Vec<Element> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            match query {
                Query::TestId(ref id) => {
                    // Scan ALL entries (not the `by_test_id` 1:1 index) so a
                    // `test_id` shared by sibling list items returns every
                    // match — `find_all`/`to_have_count` over a repeated row
                    // affordance is a core E2E pattern (Playwright's
                    // `getByTestId(...).toHaveCount(n)`). The `by_test_id` map
                    // keeps last-wins and still backs the single `find`.
                    reg.entries
                        .iter()
                        .enumerate()
                        .filter_map(|(i, slot)| {
                            let entry = slot.as_ref()?;
                            if entry.test_id == Some(id.as_str()) {
                                Some(Element::from_registry(ElementId(i as u32), entry))
                            } else {
                                None
                            }
                        })
                        .collect()
                }
                Query::Label(ref text) => {
                    reg.entries
                        .iter()
                        .enumerate()
                        .filter_map(|(i, slot)| {
                            let entry = slot.as_ref()?;
                            if entry.current_label().as_deref() == Some(text.as_str()) {
                                Some(Element::from_registry(ElementId(i as u32), entry))
                            } else {
                                None
                            }
                        })
                        .collect()
                }
                Query::LabelContains(ref text) => {
                    reg.entries
                        .iter()
                        .enumerate()
                        .filter_map(|(i, slot)| {
                            let entry = slot.as_ref()?;
                            if entry.current_label().map_or(false, |l| l.contains(text.as_str())) {
                                Some(Element::from_registry(ElementId(i as u32), entry))
                            } else {
                                None
                            }
                        })
                        .collect()
                }
                Query::Kind(kind) => {
                    reg.find_all_by_kind(kind)
                        .into_iter()
                        .filter_map(|id| reg.get(id).map(|e| Element::from_registry(id, e)))
                        .collect()
                }
                Query::All => {
                    reg.all_elements()
                        .into_iter()
                        .filter_map(|id| reg.get(id).map(|e| Element::from_registry(id, e)))
                        .collect()
                }
            }
        })
    }

    /// Returns a count of all mounted elements, optionally filtered
    /// by kind.
    pub fn count(&self, kind: Option<ElementKind>) -> usize {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            match kind {
                Some(k) => reg.find_all_by_kind(k).len(),
                None => reg.all_elements().len(),
            }
        })
    }

    // -------------------------------------------------------------------------
    // Control actions
    // -------------------------------------------------------------------------

    /// Simulate a click/press on an element.
    pub fn click(&self, element: &Element) -> Result<(), RobotError> {
        self.with_actions(element.id, |actions| {
            let click = actions.click.clone().ok_or(RobotError::ActionNotAvailable("click"))?;
            click();
            Ok(())
        })
    }

    /// Type text into an input element. This replaces the current
    /// value entirely (like a paste). For character-by-character
    /// simulation, call `type_text` per character.
    pub fn type_text(&self, element: &Element, text: &str) -> Result<(), RobotError> {
        self.with_actions(element.id, |actions| {
            let set = actions
                .set_text
                .clone()
                .ok_or(RobotError::ActionNotAvailable("type_text"))?;
            set(text.to_string());
            Ok(())
        })
    }

    /// Set a toggle's value.
    pub fn set_toggle(&self, element: &Element, value: bool) -> Result<(), RobotError> {
        self.with_actions(element.id, |actions| {
            let set = actions
                .set_toggle
                .clone()
                .ok_or(RobotError::ActionNotAvailable("set_toggle"))?;
            set(value);
            Ok(())
        })
    }

    /// Set a slider's value.
    pub fn set_slider(&self, element: &Element, value: f32) -> Result<(), RobotError> {
        self.with_actions(element.id, |actions| {
            let set = actions
                .set_slider
                .clone()
                .ok_or(RobotError::ActionNotAvailable("set_slider"))?;
            set(value);
            Ok(())
        })
    }

    /// Focus an element (text inputs).
    pub fn focus(&self, element: &Element) -> Result<(), RobotError> {
        self.with_actions(element.id, |actions| {
            let f = actions.focus.clone().ok_or(RobotError::ActionNotAvailable("focus"))?;
            f();
            Ok(())
        })
    }

    /// Blur (unfocus) an element.
    pub fn blur(&self, element: &Element) -> Result<(), RobotError> {
        self.with_actions(element.id, |actions| {
            let f = actions.blur.clone().ok_or(RobotError::ActionNotAvailable("blur"))?;
            f();
            Ok(())
        })
    }

    /// Read the element's rect in its **parent's** coordinate system.
    /// `Ok(None)` means the element exists but isn't laid out yet;
    /// `Err` means the element has been unmounted.
    pub fn frame(
        &self,
        element: &Element,
    ) -> Result<Option<crate::primitives::portal::ViewportRect>, RobotError> {
        let cb = REGISTRY.with(|r| {
            r.borrow()
                .get(element.id)
                .map(|e| e.actions.frame.clone())
                .ok_or(RobotError::ElementGone)
        })?;
        match cb {
            Some(cb) => Ok(cb()),
            None => Err(RobotError::ActionNotAvailable("frame")),
        }
    }

    /// Read the element's rect in **viewport/window** coordinates.
    pub fn absolute_frame(
        &self,
        element: &Element,
    ) -> Result<Option<crate::primitives::portal::ViewportRect>, RobotError> {
        let cb = REGISTRY.with(|r| {
            r.borrow()
                .get(element.id)
                .map(|e| e.actions.absolute_frame.clone())
                .ok_or(RobotError::ElementGone)
        })?;
        match cb {
            Some(cb) => Ok(cb()),
            None => Err(RobotError::ActionNotAvailable("absolute_frame")),
        }
    }

    /// Read the element's rect in **physical device-screen pixels** —
    /// the coordinate space an OS-level input injector (Android
    /// `adb input tap`, etc.) operates in. `Ok(None)` means the element
    /// exists but isn't laid out yet; `Err` means it's unmounted or the
    /// backend doesn't implement `device_frame`.
    pub fn device_frame(
        &self,
        element: &Element,
    ) -> Result<Option<crate::primitives::portal::ViewportRect>, RobotError> {
        let cb = REGISTRY.with(|r| {
            r.borrow()
                .get(element.id)
                .map(|e| e.actions.device_frame.clone())
                .ok_or(RobotError::ElementGone)
        })?;
        match cb {
            Some(cb) => Ok(cb()),
            None => Err(RobotError::ActionNotAvailable("device_frame")),
        }
    }

    // -------------------------------------------------------------------------
    // Signal introspection
    // -------------------------------------------------------------------------

    /// Read the current value of a signal. This read is untracked —
    /// it does not subscribe any effect.
    pub fn read_signal<T: Clone + 'static>(&self, signal: Signal<T>) -> T {
        crate::reactive::untrack(|| signal.get())
    }

    /// Write a new value to a signal, triggering its subscribers.
    pub fn write_signal<T: Clone + 'static>(&self, signal: Signal<T>, value: T) {
        signal.set(value);
    }

    /// Update a signal's value in place via a closure.
    pub fn update_signal<T: Clone + 'static>(&self, signal: Signal<T>, f: impl FnOnce(&mut T)) {
        signal.update(f);
    }

    /// Check whether a ref is currently mounted (filled).
    pub fn is_mounted<H: 'static>(&self, r: Ref<H>) -> bool {
        r.is_mounted()
    }

    // -------------------------------------------------------------------------
    // Tree snapshot
    // -------------------------------------------------------------------------

    /// Produce a flat list of all mounted elements (non-recursive,
    /// cheaper than `snapshot`).
    pub fn elements(&self) -> Vec<Element> {
        self.find_all(Query::All)
    }

    /// Produce a recursive snapshot of the component hierarchy. Returns
    /// the root elements with their full subtree.
    ///
    /// A "root" is any entry that has **no reachable parent**: either
    /// `parent == None`, or `parent == Some(pid)` where `pid` is no longer
    /// a live registry entry (a freed/recycled slot). The second case —
    /// an *orphan* — must surface, not silently disappear: navigation-time
    /// screen mounts can register content against a parent id that is dead
    /// by snapshot time (e.g. a navigator's robot id captured on
    /// `current_parent()` whose slot was reused, or any timing where the
    /// build's `PARENT_STACK` top no longer exists). Before this, such an
    /// orphan was unreachable from every `parent == None` root, so the
    /// current screen's elements vanished from `snapshot()` and `find`
    /// could still see them but the tree showed only a bare `Navigator` —
    /// see `stack-navigator/tests/robot_nav_screen_tree`. Treating a dead
    /// parent as "no parent" keeps mounted content always visible in the
    /// tree, independent of which backend/handler mounted it.
    pub fn snapshot(&self) -> Vec<TreeNode> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            // Find root elements: no parent, or a parent that no longer
            // resolves to a live entry (orphaned by a recycled/freed slot).
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
                    if is_root {
                        Some(ElementId(i as u32))
                    } else {
                        None
                    }
                })
                .collect();
            roots.iter().filter_map(|&id| self.build_tree_node(&reg, id)).collect()
        })
    }

    /// Get the subtree rooted at a specific element.
    pub fn subtree(&self, element: &Element) -> Option<TreeNode> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            self.build_tree_node(&reg, element.id)
        })
    }

    /// Get the direct children of an element.
    pub fn children_of(&self, element: &Element) -> Vec<Element> {
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

    /// Get the parent of an element.
    pub fn parent_of(&self, element: &Element) -> Option<Element> {
        REGISTRY.with(|r| {
            let reg = r.borrow();
            let entry = reg.get(element.id)?;
            let pid = entry.parent?;
            reg.get(pid).map(|e| Element::from_registry(pid, e))
        })
    }

    fn build_tree_node(&self, reg: &Registry, id: ElementId) -> Option<TreeNode> {
        let entry = reg.get(id)?;
        let children = entry
            .children
            .iter()
            .filter_map(|&cid| self.build_tree_node(reg, cid))
            .collect();
        Some(TreeNode {
            id,
            kind: entry.kind,
            test_id: entry.test_id,
            label: entry.current_label(),
            children,
        })
    }

    /// Get arena statistics (signal/effect/ref counts).
    pub fn arena_stats(&self) -> crate::ArenaStats {
        crate::arena_stats()
    }

    // -------------------------------------------------------------------------
    // Registry management (for testing the robot itself)
    // -------------------------------------------------------------------------

    /// Clear the registry. Useful between test runs.
    pub fn reset(&self) {
        REGISTRY.with(|r| {
            *r.borrow_mut() = Registry::new();
        });
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    /// Take a snapshot of the actions for an element, then drop the
    /// registry borrow before invoking `f`. This is critical because
    /// the action closure (e.g. an `on_click`) may navigate or rebuild
    /// the UI, which re-enters the registry to register new elements.
    /// Holding the registry borrow across that call panics with
    /// "RefCell already borrowed".
    fn with_actions(
        &self,
        id: ElementId,
        f: impl FnOnce(&ElementActions) -> Result<(), RobotError>,
    ) -> Result<(), RobotError> {
        let actions = REGISTRY.with(|r| {
            let reg = r.borrow();
            reg.get(id).map(|e| e.actions.clone()).ok_or(RobotError::ElementGone)
        })?;
        f(&actions)
    }
}

// =============================================================================
// Registration helpers (called by the walker)
// =============================================================================

/// Register a newly-mounted element. Called by the walker when
/// building primitives. Returns the element ID so the walker can
/// track parent/child relationships and deregister on scope drop.
pub(crate) fn register(entry: RegistryEntry) -> ElementId {
    REGISTRY.with(|r| r.borrow_mut().insert(entry))
}

/// Attach frame-reading closures to an already-registered element.
/// The walker registers the element from the `Element` (which
/// happens *before* the backend has produced a node), then calls
/// this with closures that capture the built node so positions can
/// be read on demand.
pub(crate) fn attach_frame_actions(
    id: ElementId,
    frame: Rc<dyn Fn() -> Option<crate::primitives::portal::ViewportRect>>,
    absolute_frame: Rc<dyn Fn() -> Option<crate::primitives::portal::ViewportRect>>,
    device_frame: Rc<dyn Fn() -> Option<crate::primitives::portal::ViewportRect>>,
) {
    REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        if let Some(Some(entry)) = reg.entries.get_mut(id.0 as usize) {
            entry.actions.frame = Some(frame);
            entry.actions.absolute_frame = Some(absolute_frame);
            entry.actions.device_frame = Some(device_frame);
        }
    });
}

/// Remove a previously-registered element. Wired by the walker as an
/// `on_cleanup` against the element's owning reactive scope, so a
/// `when`/`switch`/`each` branch swap (or any scope teardown) drops the
/// old subtree's registry entries instead of leaking them as phantom
/// live roots in `snapshot()` — see
/// `regression_when_branch_swap_disposes_old_branch_from_robot_registry`.
pub(crate) fn deregister(id: ElementId) {
    REGISTRY.with(|r| r.borrow_mut().remove(id));
}

/// Peek the current parent from the stack.
pub(crate) fn current_parent() -> Option<ElementId> {
    PARENT_STACK.with(|s| s.borrow().last().copied())
}

/// Push a new parent onto the stack (called before building children).
pub(crate) fn push_parent(id: ElementId) {
    PARENT_STACK.with(|s| s.borrow_mut().push(id));
}

/// Pop the current parent from the stack (called after children built).
pub(crate) fn pop_parent() {
    PARENT_STACK.with(|s| s.borrow_mut().pop());
}

/// Add a child to a parent's children list.
pub(crate) fn add_child(parent: ElementId, child: ElementId) {
    REGISTRY.with(|r| {
        if let Some(Some(entry)) = r.borrow_mut().entries.get_mut(parent.0 as usize) {
            entry.children.push(child);
        }
    });
}

#[cfg(test)]
mod find_all_tests {
    use super::*;

    fn entry(test_id: &'static str) -> RegistryEntry {
        RegistryEntry {
            kind: ElementKind::Text,
            test_id: Some(test_id),
            label: None,
            label_fn: None,
            actions: ElementActions::empty(),
            parent: None,
            children: Vec::new(),
        }
    }

    /// Regression: `find_all` by `test_id` must return EVERY element sharing
    /// that id — sibling list rows legitimately share one affordance test_id
    /// (Playwright's `getByTestId(...).toHaveCount(n)`). Previously it read the
    /// `by_test_id` 1:1 index and returned at most one, so a list-affordance
    /// count assertion (e.g. the conformance per-row del-marker) always saw 1.
    #[test]
    fn find_all_returns_every_duplicate_test_id() {
        let robot = Robot::new();
        robot.reset();
        register(entry("row-del"));
        register(entry("row-del"));
        register(entry("row-del"));
        register(entry("other"));

        assert_eq!(robot.find_all(Query::test_id("row-del")).len(), 3);
        assert_eq!(robot.find_all(Query::test_id("other")).len(), 1);
        // The single `find` still resolves (last-wins via `by_test_id`).
        assert!(robot.find(Query::test_id("row-del")).is_some());
        robot.reset();
    }

    /// Regression: `snapshot()` must surface an *orphaned* entry — one whose
    /// `parent` points to a freed/recycled slot — as a root, not hide it.
    ///
    /// This is the navigation-time screen-content loss: a screen mounted on
    /// navigation can register content against a parent id (the navigator's
    /// robot id captured via `current_parent()`) whose slot is dead by
    /// snapshot time. Before the fix, `snapshot()` collected only
    /// `parent == None` roots, so the orphan was unreachable from any root and
    /// the current screen's elements vanished from the tree (only a bare
    /// `Navigator` showed). Mounted content must always be reachable.
    #[test]
    fn snapshot_surfaces_orphan_with_dead_parent() {
        let robot = Robot::new();
        robot.reset();

        // A live parent that we then free, leaving its child orphaned.
        let dead_parent = register(entry("dead-parent"));
        let child = register(RegistryEntry {
            kind: ElementKind::Text,
            test_id: Some("orphan-child"),
            label: None,
            label_fn: None,
            actions: ElementActions::empty(),
            parent: Some(dead_parent),
            children: Vec::new(),
        });
        // Free the parent slot WITHOUT touching the child (simulating the
        // navigator robot id whose slot was reused / the parent scope that
        // dropped first). `remove` unlinks the child from the parent's list,
        // but the child still holds `parent = Some(dead_parent)`.
        REGISTRY.with(|r| {
            // Manually drop just the parent slot so the child keeps its stale
            // parent id (the real orphan condition).
            r.borrow_mut().entries[dead_parent.0 as usize] = None;
            r.borrow_mut().free.push(dead_parent.0);
        });

        // The child must appear as a snapshot root (its dead parent makes it
        // unreachable any other way).
        let tree = robot.snapshot();
        let surfaced = tree.iter().any(|n| n.id == child && n.test_id == Some("orphan-child"));
        assert!(
            surfaced,
            "orphaned entry (parent points to a freed slot) must surface as a \
             snapshot root; tree = {:?}",
            tree.iter().map(|n| (n.id, n.test_id)).collect::<Vec<_>>()
        );
        robot.reset();
    }
}
