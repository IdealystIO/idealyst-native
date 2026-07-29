//! Component-method registry for the NEW core — the vocabulary port of
//! `runtime_shared::robot::components` (P5 `#[method]` seam).
//!
//! # Emission surface (why this module always compiles)
//!
//! The `#[component]` macro emits `register_component(...)` + a
//! keepalive effect + a `__component_root(...)` wrap for every
//! `#[method]`-bearing component UNCONDITIONALLY — under `new-core` the
//! retarget maps those `::runtime_shared::…` paths to
//! `::runtime_vocabulary::glue::…`, which re-exports from HERE. Exactly
//! like the old core's stub `robot` module, the names must exist in
//! every build: with the vocabulary `robot` feature OFF these are
//! zero-work stubs (`register_component` builds nothing, hands back an
//! inert guard; `__component_root` is identity), so non-robot builds
//! pay only the `Method` vec construction the optimizer strips.
//!
//! # Model (mirrors the old registry 1:1)
//!
//! - Fresh, never-recycled [`ComponentInstanceId`] per registration —
//!   callers caching one can detect re-mounts.
//! - [`ComponentRegistration`] guard: `Drop` removes the entry AND its
//!   element link in lockstep, so a recycled element id can never
//!   resolve to a dead instance. The macro ties the guard's lifetime to
//!   the mounted lifetime by capturing it in a keepalive effect created
//!   inside the component body (collected into the component's `Owned`
//!   by `component_scope` — the new-core analogue of the old
//!   scope-owned `Effect`).
//! - Element↔component link via a one-shot pending cell: armed just
//!   before the component's root subtree realizes, consumed by the next
//!   [`register_mount`](crate::robot::register_mount) (the root
//!   primitive). See [`__component_root`] for how the new core arms it
//!   at REALIZE time (the old walker armed it while unwrapping
//!   `Element::Component`, which was build==mount time — the new core
//!   builds eagerly and realizes later, so the arm must ride the
//!   realize walk).
//!
//! # `__component_root`: the realize-time arm (robot builds only)
//!
//! The scene [`Element`] carries no metadata slot and `runtime-scene`
//! is frozen for this wave, so the wrap is expressed as a `Dyn` hole
//! whose (dependency-free, single-fire) build closure arms the pending
//! link and yields the already-built subtree. Costs, robot builds only:
//! on splice-capable hosts (web) the hole contributes no extra node; on
//! anchored hosts / detached roots (navigator screens, keyed row roots)
//! it contributes one anchor view. Two knock-on effects, both accepted
//! and documented here: a `#[method]` component used as a STACK screen
//! root is skipped by the screen-overlay style fold (its root is a
//! `Dyn`, not an `Item` — same skip class as `when`-rooted screens),
//! and the hole adds one driver effect per methods-bearing component.
//! Non-robot builds get the identity wrap — zero structural change.

use std::rc::Rc;

use runtime_shared::__serde_json as serde_json;
use runtime_scene::Element;

/// Opaque per-instance ID. Stable while the component is mounted; never
/// reused after unmount. (Stub-compatible shape when `robot` is off.)
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComponentInstanceId(pub u32);

/// One method exposed by a component. Built by the `#[component]`
/// macro; consumed by [`register_component`]. Field shape frozen to the
/// old core's `robot::Method` (the macro's struct literal must
/// type-check identically on both cores).
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
        pub methods: Vec<Method>,
    }

    thread_local! {
        pub(super) static COMPONENTS: RefCell<HashMap<u32, ComponentEntry>> =
            RefCell::new(HashMap::new());
        pub(super) static NEXT_ID: Cell<u32> = const { Cell::new(1) };
        /// `ComponentInstanceId → ElementId` (raw u32): the robot element a
        /// component instance renders as (its root primitive).
        pub(super) static ELEMENT_LINKS: RefCell<HashMap<u32, u32>> =
            RefCell::new(HashMap::new());
        /// Armed by `__component_root`'s realize-time closure; consumed by
        /// the very next `register_mount` (the component's root primitive).
        pub(super) static PENDING_LINK: Cell<Option<ComponentInstanceId>> =
            const { Cell::new(None) };
    }
}

#[cfg(feature = "robot")]
use real::*;

/// Arm the link: the next robot-registered element is `instance`'s root.
#[cfg(feature = "robot")]
pub(crate) fn set_pending_component_link(instance: ComponentInstanceId) {
    PENDING_LINK.with(|p| p.set(Some(instance)));
}

/// Take (and clear) the pending component link. One-shot so only the
/// first registration after arming — the root primitive — links.
#[cfg(feature = "robot")]
pub(crate) fn take_pending_component_link() -> Option<ComponentInstanceId> {
    PENDING_LINK.with(|p| p.take())
}

/// Record that component `instance` renders as element `element_id`.
#[cfg(feature = "robot")]
pub(crate) fn link_component_element(instance: ComponentInstanceId, element_id: u32) {
    ELEMENT_LINKS.with(|m| {
        m.borrow_mut().insert(instance.0, element_id);
    });
}

/// The component instance rendered as `element_id`, if any (reverse
/// lookup — the inspector's "select an element → call its methods").
#[cfg(feature = "robot")]
pub fn component_for_element(element_id: u32) -> Option<ComponentInstanceId> {
    ELEMENT_LINKS.with(|m| {
        m.borrow()
            .iter()
            .find(|(_, el)| **el == element_id)
            .map(|(id, _)| ComponentInstanceId(*id))
    })
}

/// RAII guard returned by [`register_component`]. Dropping it removes
/// the entry (and its element link) from the registry.
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
    }
}

/// Register a freshly-mounted component instance and its methods.
/// Returns a guard that unregisters on drop.
#[cfg(feature = "robot")]
pub fn register_component(name: &'static str, methods: Vec<Method>) -> ComponentRegistration {
    let id = NEXT_ID.with(|c| {
        let id = c.get();
        c.set(id.checked_add(1).unwrap_or(1));
        ComponentInstanceId(id)
    });
    COMPONENTS.with(|c| {
        c.borrow_mut().insert(id.0, ComponentEntry { name, methods });
    });
    ComponentRegistration { id }
}

/// No-op when the vocabulary `robot` feature is off (stub mirror of the
/// old core's non-robot `register_component`).
#[cfg(not(feature = "robot"))]
pub fn register_component(_name: &'static str, _methods: Vec<Method>) -> ComponentRegistration {
    ComponentRegistration {
        id: ComponentInstanceId(0),
    }
}

/// Snapshot of one entry, returned by [`list_components`]. Same shape as
/// the old core's `ComponentSnapshot` (the conformance methods suite and
/// the bridge JSON renderer consume it identically on both cores).
#[cfg(feature = "robot")]
pub struct ComponentSnapshot {
    pub id: ComponentInstanceId,
    pub name: &'static str,
    pub methods: Vec<(&'static str, &'static [(&'static str, &'static str)])>,
    /// The robot element this component renders as (its root
    /// primitive), if the realize-time link was established.
    pub element_id: Option<crate::robot::ElementId>,
}

#[cfg(feature = "robot")]
pub fn list_components() -> Vec<ComponentSnapshot> {
    COMPONENTS.with(|c| {
        ELEMENT_LINKS.with(|links| {
            let links = links.borrow();
            c.borrow()
                .iter()
                .map(|(id, entry)| ComponentSnapshot {
                    id: ComponentInstanceId(*id),
                    name: entry.name,
                    methods: entry.methods.iter().map(|m| (m.name, m.args)).collect(),
                    element_id: links.get(id).copied().map(crate::robot::ElementId),
                })
                .collect()
        })
    })
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
    PENDING_LINK.with(|p| p.set(None));
}

// ===========================================================================
// __component_root — the element↔component link wrap
// ===========================================================================

/// Tag a `#[component]`'s root subtree with its component instance so
/// the next mount registration links element↔component. Robot builds
/// wrap in a dependency-free `Dyn` hole that arms the pending link at
/// realize time (module docs — the new-core substitute for the old
/// walker-unwrapped `Element::Component`); non-robot builds are
/// identity.
#[cfg(feature = "robot")]
#[doc(hidden)]
pub fn __component_root(child: Element, instance: ComponentInstanceId) -> Element {
    use std::cell::RefCell;
    let slot: RefCell<Option<Element>> = RefCell::new(Some(child));
    runtime_scene::dyn_element(move || {
        match slot.borrow_mut().take() {
            Some(el) => {
                // Armed HERE — realize consumes the returned element
                // synchronously next, so the first `register_mount` in
                // this subtree (its root primitive) takes the link.
                set_pending_component_link(instance);
                el
            }
            // Unreachable in practice: the closure reads no signals, so
            // the driver effect never re-fires. Defensive empty subtree
            // instead of a panic (an Element is single-use).
            None => runtime_scene::fragment(Vec::new()),
        }
    })
}

#[cfg(not(feature = "robot"))]
#[doc(hidden)]
#[inline(always)]
pub fn __component_root(child: Element, _instance: ComponentInstanceId) -> Element {
    child
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

    /// The element↔component link: arm (realize-time closure) → next
    /// registration consumes → both lookups resolve → drop removes the
    /// link in lockstep. Mirror of the old
    /// `element_component_link_round_trips`.
    #[test]
    fn element_component_link_round_trips() {
        reset();
        let reg = register_component("Counter", Vec::new());
        let id = reg.id();

        set_pending_component_link(id);
        assert_eq!(take_pending_component_link(), Some(id));
        assert_eq!(
            take_pending_component_link(),
            None,
            "pending link is one-shot — descendants must not re-link"
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
}
