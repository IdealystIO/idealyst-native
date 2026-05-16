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
}

pub fn list_components() -> Vec<ComponentSnapshot> {
    COMPONENTS.with(|c| {
        c.borrow()
            .iter()
            .map(|(id, entry)| ComponentSnapshot {
                id: *id,
                name: entry.name,
                methods: entry
                    .methods
                    .iter()
                    .map(|m| (m.name, m.args))
                    .collect(),
            })
            .collect()
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
