//! Component-method registry — companion to the element registry.
//!
//! When a `#[component]` body declares a `methods! { ... }` block, the
//! macro generates an imperative `Handle` (e.g. `CounterHandle { reset,
//! bump_by }`) plus a registration call that lands a JSON-callable
//! wrapper for each method here. The robot bridge exposes the registry
//! over its TCP protocol so external automation (MCP server, E2E
//! harness) can drive components by name.
//!
//! Identity: each registration receives a fresh `ComponentInstanceId`.
//! IDs never recycle, so callers caching them can detect re-mounts.
//!
//! Lifecycle: `register_component` hands back a `ComponentRegistration`
//! whose `Drop` removes the entry. The macro keeps the guard alive for
//! the component's mounted lifetime by capturing it in a reactive
//! `Effect` attached to the surrounding scope — same trick the walker
//! uses for navigator scope keepalives.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Opaque per-instance ID. Stable while the component is mounted; never
/// reused after unmount.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ComponentInstanceId(pub u32);

/// One method exposed by a component. Built by the `#[component]`
/// macro; consumed by `register_component`.
pub struct Method {
    /// Method name as written in `methods! { fn NAME(...) }`.
    pub name: &'static str,
    /// Arguments in declaration order: `(name, rust_type_string)`.
    /// The type string is the source-form of the declared type
    /// (e.g. `"i32"`, `"String"`, `"MyStruct"`) — enough for an
    /// LLM-driven client to construct a valid JSON value, and a
    /// fallback when there's no formal schema.
    pub args: &'static [(&'static str, &'static str)],
    /// JSON-callable adapter. Receives the args object verbatim,
    /// deserializes each parameter, invokes the closure. Returns
    /// `Err` if deserialization fails.
    pub invoke: Rc<dyn Fn(&serde_json::Value) -> Result<(), String>>,
}

pub(crate) struct ComponentEntry {
    pub name: &'static str,
    pub methods: Vec<Method>,
}

thread_local! {
    static COMPONENTS: RefCell<HashMap<ComponentInstanceId, ComponentEntry>> =
        RefCell::new(HashMap::new());
    static NEXT_ID: RefCell<u32> = const { RefCell::new(1) };

    /// `ComponentInstanceId → ElementId`: the robot element a component
    /// instance renders as (its root primitive). Populated during the walk:
    /// the `#[component]` macro wraps a methods-bearing component's root in
    /// `Element::Component { instance, .. }`, the walker unwraps it and arms
    /// [`set_pending_component_link`], and the next element registered (the
    /// root primitive) consumes it via [`take_pending_component_link`] +
    /// [`link_component_element`]. Lets the inspector map a selected element
    /// to the component whose methods it can invoke.
    static ELEMENT_LINKS: RefCell<HashMap<ComponentInstanceId, super::ElementId>> =
        RefCell::new(HashMap::new());

    /// Set by the walker when it unwraps an `Element::Component`; consumed by
    /// the very next robot registration (the component's root primitive).
    static PENDING_LINK: std::cell::Cell<Option<ComponentInstanceId>> =
        const { std::cell::Cell::new(None) };
}

/// Arm the link: the next robot-registered element is `instance`'s root.
pub(crate) fn set_pending_component_link(instance: ComponentInstanceId) {
    PENDING_LINK.with(|p| p.set(Some(instance)));
}

/// Take (and clear) the pending component link, if any. Cleared so only the
/// first registration after arming — the root primitive — links.
pub(crate) fn take_pending_component_link() -> Option<ComponentInstanceId> {
    PENDING_LINK.with(|p| p.take())
}

/// Record that component `instance` renders as element `element`.
pub(crate) fn link_component_element(instance: ComponentInstanceId, element: super::ElementId) {
    ELEMENT_LINKS.with(|m| {
        m.borrow_mut().insert(instance, element);
    });
}

/// The component instance rendered as `element`, if any (reverse lookup).
/// Used by the bridge so the inspector can resolve a selected element to its
/// invokable component.
pub fn component_for_element(element: super::ElementId) -> Option<ComponentInstanceId> {
    ELEMENT_LINKS.with(|m| {
        m.borrow()
            .iter()
            .find(|(_, el)| **el == element)
            .map(|(id, _)| *id)
    })
}

fn next_id() -> ComponentInstanceId {
    NEXT_ID.with(|c| {
        let mut c = c.borrow_mut();
        let id = *c;
        *c = c.checked_add(1).unwrap_or(1);
        ComponentInstanceId(id)
    })
}

/// RAII guard returned by `register_component`. Dropping it removes
/// the entry from the registry.
pub struct ComponentRegistration {
    id: ComponentInstanceId,
}

impl ComponentRegistration {
    pub fn id(&self) -> ComponentInstanceId {
        self.id
    }
}

impl Drop for ComponentRegistration {
    fn drop(&mut self) {
        COMPONENTS.with(|c| {
            c.borrow_mut().remove(&self.id);
        });
        // Drop the element link in lockstep so a recycled element id can't
        // resolve to a dead component instance.
        ELEMENT_LINKS.with(|m| {
            m.borrow_mut().remove(&self.id);
        });
    }
}

/// Register a freshly-mounted component instance and its methods.
/// Returns a guard that unregisters on drop.
pub fn register_component(name: &'static str, methods: Vec<Method>) -> ComponentRegistration {
    let id = next_id();
    COMPONENTS.with(|c| {
        c.borrow_mut().insert(id, ComponentEntry { name, methods });
    });
    ComponentRegistration { id }
}

/// Snapshot of one entry, returned by `list_components`.
pub struct ComponentSnapshot {
    pub id: ComponentInstanceId,
    pub name: &'static str,
    pub methods: Vec<(&'static str, &'static [(&'static str, &'static str)])>,
    /// The robot element this component renders as (its root primitive), if
    /// the link was established during the walk. Lets a UI map a selected
    /// element to its invokable component.
    pub element_id: Option<super::ElementId>,
}

pub fn list_components() -> Vec<ComponentSnapshot> {
    COMPONENTS.with(|c| {
        ELEMENT_LINKS.with(|links| {
            let links = links.borrow();
            c.borrow()
                .iter()
                .map(|(id, entry)| ComponentSnapshot {
                    id: *id,
                    name: entry.name,
                    methods: entry.methods.iter().map(|m| (m.name, m.args)).collect(),
                    element_id: links.get(id).copied(),
                })
                .collect()
        })
    })
}

/// Invoke a method on a registered component. Returns `Err` if the
/// instance is gone, the method is unknown, or arg deserialization
/// fails.
pub fn invoke_method(
    instance: ComponentInstanceId,
    method: &str,
    args: &serde_json::Value,
) -> Result<(), String> {
    // Clone the Rc out under a short borrow so the invoker can run
    // without holding the registry lock — invokers may re-enter the
    // walker (e.g. a method that flips a signal triggers rebuilds
    // that register new components).
    let invoker = COMPONENTS.with(|c| {
        let c = c.borrow();
        let entry = c
            .get(&instance)
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
    invoker(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The element↔component link the inspector relies on: arm a pending
    /// link (walker unwrap of `Element::Component`), have the next
    /// registration consume it (the root primitive), then resolve both
    /// directions + surface it via `list_components`. Dropping the
    /// registration must drop the link in lockstep so a recycled element id
    /// can't resolve to a dead instance.
    #[test]
    fn element_component_link_round_trips() {
        let reg = register_component("Counter", Vec::new());
        let id = reg.id();
        let element = super::super::ElementId(4242);

        // Walker sequence: arm, then the root primitive's registration takes
        // it (and clears it, so only the root links — not descendants).
        set_pending_component_link(id);
        assert_eq!(take_pending_component_link(), Some(id));
        assert_eq!(
            take_pending_component_link(),
            None,
            "pending link is one-shot — descendants must not re-link"
        );
        link_component_element(id, element);

        assert_eq!(component_for_element(element), Some(id), "reverse lookup");
        let snap = list_components();
        let entry = snap.iter().find(|s| s.id == id).expect("registered");
        assert_eq!(entry.element_id, Some(element), "surfaced in list_components");

        drop(reg);
        assert_eq!(
            component_for_element(element),
            None,
            "link dropped with the component registration"
        );
    }
}
