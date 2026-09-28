//! Component registry for the NEW core — every mounted `#[component]`
//! instance, its props, its `#[method]`s, and the element it renders as.
//! This is what the inspector's component tree and Props table read.
//!
//! # Emission surface (why this module always compiles)
//!
//! `#[component]` emits, for EVERY component body returning `Element`:
//!
//! ```text
//! let __idealyst_inspect = __inspect_component(NAME, file!(), line!(), || props…);
//! __idealyst_inspect.finish(component_scope(move || { body }))
//! ```
//!
//! and, for a component with `#[method]`s, a
//! [`__attach_component_methods`] call inside the body. The names must
//! exist in every build: with the vocabulary `robot` feature OFF,
//! [`__inspect_component`] never calls its props closure (so the probes
//! are never instantiated), `finish` is identity, and attaching methods
//! drops them.
//!
//! # Model
//!
//! - Fresh, never-recycled [`ComponentInstanceId`] per registration.
//! - The registration lives exactly as long as the component's subtree:
//!   `finish` moves the guard into a scope that rides the returned
//!   element and is absorbed into the enclosing realized tree.
//! - **Element link.** `finish` brackets the subtree with a scene realize
//!   hook ([`runtime_scene::with_realize_hook`]). Entering arms a pending
//!   link; the first element the robot registry registers inside the
//!   bracket consumes every armed link — so a component whose root is
//!   another component's element links both to the same element, outer
//!   first. Leaving disarms a link nothing consumed (a component that
//!   mounted no registered node must not claim the next sibling's).
//!   On a component rooted in a reactive region the hook re-brackets
//!   each branch, so the link follows the swap.
//! - **Parent component.** Not stored: the bridge derives it from the
//!   element tree (the nearest ancestor element carrying a link belongs
//!   to the parent). That gives the RENDERED hierarchy — `Card { Button }`
//!   written inside `Screen` reads `Screen › Card › Button` — which is
//!   what a build-time owner stack could not give.
//!
//! Known approximation: a component whose root is a FRAGMENT links to the
//! fragment's first registered node only; its other top-level nodes read
//! as belonging to its parent.
//!
//! # Why a hook and not a wrapper node
//!
//! The previous link wrapped each `#[method]` component in a `Dyn` hole,
//! which added an anchor node on anchored hosts and an effect per
//! component, and hid the component's root from the navigator's
//! screen-style fold. Applied to every component in every dev build that
//! would have made dev layout differ from release. The realize hook
//! creates no node and no effect.

use std::rc::Rc;

use runtime_shared::__serde_json as serde_json;
use runtime_scene::Element;

pub use crate::robot_props::{PropEntry, PropMode};

/// Opaque per-instance ID. Stable while the component is mounted; never
/// reused after unmount. (Stub-compatible shape when `robot` is off.)
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComponentInstanceId(pub u32);

/// One method exposed by a component. Built by the `#[component]`
/// macro; consumed by [`register_component`] /
/// [`__attach_component_methods`].
pub struct Method {
    /// Method name as written on the `#[method] fn NAME(...)`.
    pub name: &'static str,
    /// Arguments in declaration order: `(name, rust_type_string)`.
    pub args: &'static [(&'static str, &'static str)],
    /// JSON-callable adapter: deserializes each parameter by name,
    /// invokes the handle's closure. `Err` on deserialization failure.
    pub invoke: Rc<dyn Fn(&serde_json::Value) -> Result<(), String>>,
}

// ===========================================================================
// Real registry (feature `robot`)
// ===========================================================================

#[cfg(feature = "robot")]
mod real {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    pub(super) struct ComponentEntry {
        pub name: &'static str,
        /// Source location of the component fn (`file!()` / `line!()` at
        /// the definition). Empty / 0 for a hand-registered entry.
        pub file: &'static str,
        pub line: u32,
        pub methods: Vec<Method>,
        pub props: Vec<PropEntry>,
    }

    thread_local! {
        pub(super) static COMPONENTS: RefCell<HashMap<u32, ComponentEntry>> =
            RefCell::new(HashMap::new());
        pub(super) static NEXT_ID: Cell<u32> = const { Cell::new(1) };
        /// `ComponentInstanceId → (ElementId, link order)`: the robot
        /// element a component instance renders as, and a global sequence
        /// number so instances sharing one element order outer → inner.
        pub(super) static ELEMENT_LINKS: RefCell<HashMap<u32, (u32, u64)>> =
            RefCell::new(HashMap::new());
        pub(super) static LINK_SEQ: Cell<u64> = const { Cell::new(0) };
        /// Links armed by entered realize hooks and not yet consumed,
        /// outermost first.
        pub(super) static PENDING_LINKS: RefCell<Vec<ComponentInstanceId>> =
            const { RefCell::new(Vec::new()) };
        /// Component bodies currently executing, innermost last — where
        /// [`__attach_component_methods`] finds its instance.
        pub(super) static BUILDING: RefCell<Vec<ComponentInstanceId>> =
            const { RefCell::new(Vec::new()) };
    }

    pub(super) fn next_id() -> ComponentInstanceId {
        NEXT_ID.with(|c| {
            let id = c.get();
            c.set(id.checked_add(1).unwrap_or(1));
            ComponentInstanceId(id)
        })
    }
}

#[cfg(feature = "robot")]
use real::*;

/// Arm a link: the next robot-registered element is `instance`'s root.
#[cfg(feature = "robot")]
pub(crate) fn arm_component_link(instance: ComponentInstanceId) {
    PENDING_LINKS.with(|p| p.borrow_mut().push(instance));
}

/// Disarm `instance`'s link if nothing consumed it.
#[cfg(feature = "robot")]
pub(crate) fn disarm_component_link(instance: ComponentInstanceId) {
    PENDING_LINKS.with(|p| p.borrow_mut().retain(|i| *i != instance));
}

/// Take every armed link, outermost first. Called by the robot registry
/// for each element it registers; only the first one inside a bracket
/// finds anything.
#[cfg(feature = "robot")]
pub(crate) fn take_pending_component_links() -> Vec<ComponentInstanceId> {
    PENDING_LINKS.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// Record that component `instance` renders as element `element_id`.
/// A later link for the same instance (its region swapped) replaces the
/// earlier one.
#[cfg(feature = "robot")]
pub(crate) fn link_component_element(instance: ComponentInstanceId, element_id: u32) {
    if !COMPONENTS.with(|c| c.borrow().contains_key(&instance.0)) {
        return; // the registration already dropped
    }
    let seq = LINK_SEQ.with(|s| {
        let n = s.get() + 1;
        s.set(n);
        n
    });
    ELEMENT_LINKS.with(|m| {
        m.borrow_mut().insert(instance.0, (element_id, seq));
    });
}

/// Every component instance rendered as `element_id`, outermost first
/// (a component returning another component's element shares it).
#[cfg(feature = "robot")]
pub fn components_for_element(element_id: u32) -> Vec<ComponentInstanceId> {
    ELEMENT_LINKS.with(|m| {
        let mut hits: Vec<(u64, u32)> = m
            .borrow()
            .iter()
            .filter(|(_, (el, _))| *el == element_id)
            .map(|(id, (_, seq))| (*seq, *id))
            .collect();
        hits.sort();
        hits.into_iter().map(|(_, id)| ComponentInstanceId(id)).collect()
    })
}

/// The innermost component instance rendered as `element_id`, if any.
#[cfg(feature = "robot")]
pub fn component_for_element(element_id: u32) -> Option<ComponentInstanceId> {
    components_for_element(element_id).pop()
}

/// Element → its linked instances (outermost first), for every linked
/// element — one pass for the bridge's tree render.
#[cfg(feature = "robot")]
pub(crate) fn element_component_index() -> std::collections::HashMap<u32, Vec<(ComponentInstanceId, &'static str)>> {
    let mut by_element: std::collections::HashMap<u32, Vec<(u64, ComponentInstanceId, &'static str)>> =
        std::collections::HashMap::new();
    COMPONENTS.with(|c| {
        let c = c.borrow();
        ELEMENT_LINKS.with(|m| {
            for (id, (el, seq)) in m.borrow().iter() {
                if let Some(entry) = c.get(id) {
                    by_element
                        .entry(*el)
                        .or_default()
                        .push((*seq, ComponentInstanceId(*id), entry.name));
                }
            }
        });
    });
    by_element
        .into_iter()
        .map(|(el, mut v)| {
            v.sort_by_key(|(seq, _, _)| *seq);
            (el, v.into_iter().map(|(_, id, name)| (id, name)).collect())
        })
        .collect()
}

/// RAII guard: dropping it removes the entry (and its element link) from
/// the registry.
pub struct ComponentRegistration {
    id: ComponentInstanceId,
}

impl ComponentRegistration {
    pub fn id(&self) -> ComponentInstanceId {
        self.id
    }
}

#[cfg(feature = "robot")]
impl Drop for ComponentRegistration {
    fn drop(&mut self) {
        COMPONENTS.with(|c| {
            c.borrow_mut().remove(&self.id.0);
        });
        // Drop the element link in lockstep so a recycled element id
        // can't resolve to a dead component instance.
        ELEMENT_LINKS.with(|m| {
            m.borrow_mut().remove(&self.id.0);
        });
        disarm_component_link(self.id);
        crate::robot::bump_component_revision();
    }
}

#[cfg(feature = "robot")]
fn insert_entry(
    name: &'static str,
    file: &'static str,
    line: u32,
    methods: Vec<Method>,
    props: Vec<PropEntry>,
) -> ComponentRegistration {
    let id = next_id();
    COMPONENTS.with(|c| {
        c.borrow_mut().insert(id.0, ComponentEntry { name, file, line, methods, props });
    });
    crate::robot::bump_component_revision();
    ComponentRegistration { id }
}

/// Register a component instance by hand (no props, no source location).
/// The `#[component]` macro uses [`__inspect_component`] instead; this
/// stays for hosts and tests that build a methods-bearing entry directly.
#[cfg(feature = "robot")]
pub fn register_component(name: &'static str, methods: Vec<Method>) -> ComponentRegistration {
    insert_entry(name, "", 0, methods, Vec::new())
}

/// No-op when the vocabulary `robot` feature is off.
#[cfg(not(feature = "robot"))]
pub fn register_component(_name: &'static str, _methods: Vec<Method>) -> ComponentRegistration {
    ComponentRegistration { id: ComponentInstanceId(0) }
}

/// A component body's registration in progress: returned by
/// [`__inspect_component`] at the top of the body, consumed by
/// [`finish`](ComponentInspect::finish) with the body's element.
#[must_use = "pass the component's element through `finish`"]
pub struct ComponentInspect {
    #[cfg(feature = "robot")]
    reg: Option<ComponentRegistration>,
}

#[cfg(feature = "robot")]
impl Drop for ComponentInspect {
    fn drop(&mut self) {
        // Pops the build frame on every exit path — `finish`, an early
        // return, or a panic in the body.
        if let Some(reg) = &self.reg {
            let id = reg.id();
            BUILDING.with(|b| {
                let mut b = b.borrow_mut();
                if b.last() == Some(&id) {
                    b.pop();
                } else {
                    b.retain(|i| *i != id);
                }
            });
        }
    }
}

impl ComponentInspect {
    /// Tie the registration to `element`'s mounted lifetime and bracket
    /// its realization so the robot registry links the instance to the
    /// first element it mounts (module docs). Identity without `robot`.
    #[cfg(feature = "robot")]
    pub fn finish(mut self, element: Element) -> Element {
        let Some(reg) = self.reg.take() else { return element };
        let id = reg.id();
        // Pop the build frame now (the body has run); Drop sees `None`.
        BUILDING.with(|b| b.borrow_mut().retain(|i| *i != id));
        // The guard rides a scope that rides the element: absorbed into
        // the enclosing realized tree at mount, dropped with it at
        // unmount — or dropped right away if the element is never
        // realized. Outside a world `on_owned_drop` is inert and the
        // guard drops here, which is the right answer: nothing mounted.
        let (_, scope) = runtime_world::component_scope(move || {
            runtime_world::on_owned_drop(move || drop(reg));
        });
        let element = runtime_scene::owned(element, scope);
        runtime_scene::with_realize_hook(
            element,
            Rc::new(move || {
                arm_component_link(id);
                Box::new(move || disarm_component_link(id)) as Box<dyn FnOnce()>
            }),
        )
    }

    #[cfg(not(feature = "robot"))]
    #[inline(always)]
    pub fn finish(self, element: Element) -> Element {
        element
    }
}

/// Register the component whose body is starting. `props` runs only in
/// robot builds (module docs). Macro emission target.
#[doc(hidden)]
#[cfg(feature = "robot")]
pub fn __inspect_component(
    name: &'static str,
    file: &'static str,
    line: u32,
    props: impl FnOnce() -> Vec<PropEntry>,
) -> ComponentInspect {
    let reg = insert_entry(name, file, line, Vec::new(), props());
    BUILDING.with(|b| b.borrow_mut().push(reg.id()));
    ComponentInspect { reg: Some(reg) }
}

#[doc(hidden)]
#[cfg(not(feature = "robot"))]
#[inline(always)]
pub fn __inspect_component(
    _name: &'static str,
    _file: &'static str,
    _line: u32,
    _props: impl FnOnce() -> Vec<PropEntry>,
) -> ComponentInspect {
    ComponentInspect {}
}

/// Attach `#[method]`s to the component whose body is executing. Macro
/// emission target (inside the body, after the handle is built).
#[doc(hidden)]
pub fn __attach_component_methods(methods: Vec<Method>) {
    #[cfg(feature = "robot")]
    {
        let Some(id) = BUILDING.with(|b| b.borrow().last().copied()) else { return };
        COMPONENTS.with(|c| {
            if let Some(entry) = c.borrow_mut().get_mut(&id.0) {
                entry.methods = methods;
            }
        });
    }
    #[cfg(not(feature = "robot"))]
    drop(methods);
}

/// Snapshot of one entry, returned by [`list_components`].
#[cfg(feature = "robot")]
pub struct ComponentSnapshot {
    pub id: ComponentInstanceId,
    pub name: &'static str,
    pub file: &'static str,
    pub line: u32,
    pub methods: Vec<(&'static str, &'static [(&'static str, &'static str)])>,
    /// The robot element this component renders as (its root
    /// primitive), if the realize-time link was established.
    pub element_id: Option<crate::robot::ElementId>,
}

#[cfg(feature = "robot")]
pub fn list_components() -> Vec<ComponentSnapshot> {
    let mut out: Vec<ComponentSnapshot> = COMPONENTS.with(|c| {
        ELEMENT_LINKS.with(|links| {
            let links = links.borrow();
            c.borrow()
                .iter()
                .map(|(id, entry)| ComponentSnapshot {
                    id: ComponentInstanceId(*id),
                    name: entry.name,
                    file: entry.file,
                    line: entry.line,
                    methods: entry.methods.iter().map(|m| (m.name, m.args)).collect(),
                    element_id: links.get(id).map(|(el, _)| crate::robot::ElementId(*el)),
                })
                .collect()
        })
    });
    out.sort_by_key(|s| s.id.0);
    out
}

/// One prop's rendered state, from [`component_props`].
#[cfg(feature = "robot")]
pub struct PropSnapshot {
    pub name: &'static str,
    pub ty: &'static str,
    pub mode: PropMode,
    /// The `Debug` rendering of the CURRENT value, truncated to
    /// [`PROP_VALUE_MAX_CHARS`]; `None` when it can't be rendered.
    pub value: Option<String>,
}

/// Cap on a rendered prop value — a `Debug` of a large collection would
/// otherwise dominate the bridge reply.
pub const PROP_VALUE_MAX_CHARS: usize = 400;

/// Render `instance`'s props now. `None` if the instance is gone.
///
/// The readers are cloned out under a short borrow and run after it
/// drops: a `Live` prop's closure is AUTHOR code and may touch anything.
#[cfg(feature = "robot")]
pub fn component_props(instance: ComponentInstanceId) -> Option<Vec<PropSnapshot>> {
    let props = COMPONENTS.with(|c| c.borrow().get(&instance.0).map(|e| e.props.clone()))?;
    Some(
        props
            .into_iter()
            .map(|p| PropSnapshot {
                name: p.name,
                ty: p.ty,
                mode: p.mode,
                value: p.read.map(|read| truncate_chars(read(), PROP_VALUE_MAX_CHARS)),
            })
            .collect(),
    )
}

#[cfg(feature = "robot")]
fn truncate_chars(s: String, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &s[..cut]),
        None => s,
    }
}

/// Invoke a method on a registered component. `Err` if the instance is
/// gone, the method is unknown, or arg deserialization fails.
///
/// The invoker is AUTHOR CODE (it writes signals through handle routes):
/// it runs OUTSIDE `World::enter`, its writes stage, and this fn
/// [`settle`](crate::robot::settle)s afterwards so a query on the next
/// line observes the post-invoke tree — the same action contract as
/// `Robot::click` (driver-env docs in `crate::robot`).
#[cfg(feature = "robot")]
pub fn invoke_method(
    instance: ComponentInstanceId,
    method: &str,
    args: &serde_json::Value,
) -> Result<(), String> {
    // Clone the Rc out under a short borrow so the invoker can run
    // without holding the registry borrow — invokers may trigger
    // rebuilds that register new components (old registry's guard).
    let invoker = COMPONENTS.with(|c| {
        let c = c.borrow();
        let entry = c
            .get(&instance.0)
            .ok_or_else(|| format!("component instance {} not found", instance.0))?;
        let m = entry
            .methods
            .iter()
            .find(|m| m.name == method)
            .ok_or_else(|| {
                format!(
                    "component '{}' has no method '{}'; available: [{}]",
                    entry.name,
                    method,
                    entry
                        .methods
                        .iter()
                        .map(|m| m.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        Ok::<_, String>(m.invoke.clone())
    })?;
    let out = invoker(args);
    crate::robot::settle();
    out
}

/// Test isolation: clear the component + link registries (thread-local).
#[cfg(feature = "robot")]
pub(crate) fn reset() {
    COMPONENTS.with(|c| c.borrow_mut().clear());
    ELEMENT_LINKS.with(|m| m.borrow_mut().clear());
    PENDING_LINKS.with(|p| p.borrow_mut().clear());
    BUILDING.with(|b| b.borrow_mut().clear());
}

#[cfg(all(test, feature = "robot"))]
mod tests {
    use super::*;
    use runtime_shared::__serde_json::json;
    use std::cell::Cell;

    /// Register → list → invoke → drop deregisters (guard lifecycle),
    /// mirroring the old `components.rs` contract.
    #[test]
    fn register_invoke_and_unregister_on_drop() {
        reset();
        let hits: Rc<Cell<i32>> = Rc::new(Cell::new(0));
        let hits_in = hits.clone();
        let methods = vec![Method {
            name: "bump_by",
            args: &[("n", "i32")],
            invoke: Rc::new(move |args| {
                let n: i32 = serde_json::from_value(
                    args.get("n").cloned().unwrap_or(serde_json::Value::Null),
                )
                .map_err(|e| format!("arg 'n': {e}"))?;
                hits_in.set(hits_in.get() + n);
                Ok(())
            }),
        }];
        let reg = register_component("Counter", methods);
        let id = reg.id();

        let snap = list_components();
        let entry = snap.iter().find(|s| s.id == id).expect("registered");
        assert_eq!(entry.name, "Counter");
        assert_eq!(entry.methods, vec![("bump_by", &[("n", "i32")][..])]);
        assert_eq!(entry.element_id, None, "no link armed yet");

        invoke_method(id, "bump_by", &json!({ "n": 5 })).expect("invoke");
        assert_eq!(hits.get(), 5, "author closure ran with deserialized arg");

        // Unknown method / bad args surface as errors, not silence.
        let err = invoke_method(id, "nope", &json!({})).unwrap_err();
        assert!(err.contains("has no method 'nope'"), "{err}");
        let err = invoke_method(id, "bump_by", &json!({ "n": "NaN" })).unwrap_err();
        assert!(err.contains("arg 'n'"), "{err}");

        drop(reg);
        assert!(
            invoke_method(id, "bump_by", &json!({ "n": 1 })).is_err(),
            "dropped registration must deregister"
        );
        assert!(list_components().iter().all(|s| s.id != id));
        reset();
    }

    /// The element↔component link: arm → the next registration consumes
    /// every armed link → both lookups resolve → drop removes the link in
    /// lockstep.
    #[test]
    fn element_component_link_round_trips() {
        reset();
        let reg = register_component("Counter", Vec::new());
        let id = reg.id();

        arm_component_link(id);
        assert_eq!(take_pending_component_links(), vec![id]);
        assert!(
            take_pending_component_links().is_empty(),
            "consumed links are gone — descendants must not re-link"
        );
        link_component_element(id, 4242);

        assert_eq!(component_for_element(4242), Some(id), "reverse lookup");
        let snap = list_components();
        let entry = snap.iter().find(|s| s.id == id).expect("registered");
        assert_eq!(entry.element_id, Some(crate::robot::ElementId(4242)));

        drop(reg);
        assert_eq!(
            component_for_element(4242),
            None,
            "link dropped with the component registration"
        );
        reset();
    }

    /// A component whose root is another component's element: both arm
    /// before the element registers, both link to it, outer first.
    #[test]
    fn nested_links_share_an_element_outermost_first() {
        reset();
        let outer = register_component("Outer", Vec::new());
        let inner = register_component("Inner", Vec::new());
        arm_component_link(outer.id());
        arm_component_link(inner.id());
        for instance in take_pending_component_links() {
            link_component_element(instance, 7);
        }
        assert_eq!(components_for_element(7), vec![outer.id(), inner.id()]);
        assert_eq!(component_for_element(7), Some(inner.id()), "innermost");
        let index = element_component_index();
        let names: Vec<&str> = index[&7].iter().map(|(_, n)| *n).collect();
        assert_eq!(names, ["Outer", "Inner"]);
        reset();
    }

    /// Why disarm exists: a component that mounted no registered node
    /// must not hand its link to whatever registers next (a sibling).
    #[test]
    fn regression_unconsumed_link_does_not_leak_to_a_sibling() {
        reset();
        let empty = register_component("Empty", Vec::new());
        arm_component_link(empty.id());
        disarm_component_link(empty.id()); // its bracket closed, nothing mounted
        assert!(take_pending_component_links().is_empty());
        reset();
    }

    /// Methods attach to the component whose body is running, and the
    /// build frame pops even when the body never reaches `finish`.
    #[test]
    fn methods_attach_to_the_building_component() {
        reset();
        let inspect = __inspect_component("Tally", "src/tally.rs", 3, Vec::new);
        __attach_component_methods(vec![Method {
            name: "reset",
            args: &[],
            invoke: Rc::new(|_| Ok(())),
        }]);
        let snap = list_components();
        let tally = snap.iter().find(|s| s.name == "Tally").expect("registered");
        assert_eq!(tally.methods, vec![("reset", &[][..])]);
        assert_eq!((tally.file, tally.line), ("src/tally.rs", 3));
        drop(inspect); // an early return / panic path
        assert!(BUILDING.with(|b| b.borrow().is_empty()), "frame popped");
        reset();
    }

    /// Props render their CURRENT value through the registry, truncated.
    #[test]
    fn component_props_render_current_values() {
        use crate::robot_props::{entry, PropMode};
        reset();
        let long = "x".repeat(PROP_VALUE_MAX_CHARS + 50);
        let long_in = long.clone();
        let inspect = __inspect_component("Card", "", 0, move || {
            vec![
                entry("title", "Reactive<String>", (PropMode::Static, Some(Rc::new(|| "\"Hi\"".to_string())))),
                entry("body", "String", (PropMode::Value, Some(Rc::new(move || long_in.clone())))),
                entry("on_close", "Rc<dyn Fn()>", (PropMode::Handler, None)),
            ]
        });
        let id = list_components().into_iter().find(|s| s.name == "Card").unwrap().id;
        let props = component_props(id).expect("instance is live");
        assert_eq!(props[0].value.as_deref(), Some("\"Hi\""));
        assert_eq!(props[1].value.as_ref().map(|v| v.chars().count()), Some(PROP_VALUE_MAX_CHARS + 1));
        assert_eq!((props[2].mode, props[2].value.as_deref()), (PropMode::Handler, None));
        drop(inspect);
        reset();
    }
}
