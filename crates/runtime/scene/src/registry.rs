//! The TypeId-keyed handler [`Registry`] — the scene's ONLY mount path.
//!
//! Generalizes runtime-core's `ExternalRegistry` (external.rs) into the
//! universal primitive contract: every payload type — framework-shipped or
//! third-party — registers one handler per backend, type erasure is paid
//! at exactly one line (inside [`Registry::register`]), and user-facing
//! constructors stay fully typed. "External" stops being a concept.

use std::any::{Any, TypeId};
use std::rc::Rc;

use rustc_hash::FxHashMap;

use crate::element::Element;
use crate::host::Host;
use crate::realize::MountCx;

/// A type-erased mount handler: receives the mount context (backend access
/// + child realization), the item's payload, and its children; returns the
/// real node. Handlers own the full mount sequence for their primitive
/// (create → bind props → realize children → attach handlers).
pub type Handler<H> =
    Rc<dyn Fn(&mut MountCx<'_, H>, Rc<dyn Any>, Vec<Element>) -> <H as Host>::Node>;

/// Per-backend registry of primitive handlers keyed by the payload type's
/// [`TypeId`]. TypeId keying is collision-free by construction: two
/// unrelated crates' `MapViewProps` types have distinct TypeIds, where a
/// string-keyed registry would conflict.
pub struct Registry<H: Host> {
    handlers: FxHashMap<TypeId, Handler<H>>,
}

impl<H: Host> Registry<H> {
    pub fn new() -> Self {
        Registry {
            handlers: FxHashMap::default(),
        }
    }

    /// Register `handler` for payload type `T`. Returns the previously
    /// registered handler if `T` was already registered (typically `None`;
    /// `Some` means the same primitive registered twice).
    pub fn register<T, F>(&mut self, handler: F) -> Option<Handler<H>>
    where
        T: 'static,
        F: Fn(&mut MountCx<'_, H>, &Rc<T>, Vec<Element>) -> H::Node + 'static,
    {
        // Type erasure happens here, at exactly one line per registration.
        // The stored closure downcasts the payload back on each mount; a
        // mismatch is impossible unless the registry map itself is
        // corrupted (the lookup key IS the payload's TypeId), so it panics
        // loudly as a kernel bug rather than degrading.
        let erased: Handler<H> = Rc::new(move |cx, any, children| {
            let typed: Rc<T> = any.downcast::<T>().unwrap_or_else(|_| {
                panic!(
                    "runtime-scene: item payload downcast mismatch for the handler registered \
                     under `{}` — the registry key and the stored closure disagree (scene bug)",
                    std::any::type_name::<T>()
                )
            });
            handler(cx, &typed, children)
        });
        self.handlers.insert(TypeId::of::<T>(), erased)
    }

    /// Look up the handler for `type_id`. Returns a cloned `Rc` so the
    /// caller can release the registry borrow before invoking the handler.
    pub fn get(&self, type_id: TypeId) -> Option<Handler<H>> {
        self.handlers.get(&type_id).cloned()
    }

    /// `true` if `T` has a registered handler.
    pub fn has<T: 'static>(&self) -> bool {
        self.handlers.contains_key(&TypeId::of::<T>())
    }

    /// `true` if any payload with this `type_id` has a registered handler.
    pub fn has_id(&self, type_id: TypeId) -> bool {
        self.handlers.contains_key(&type_id)
    }
}

impl<H: Host> Default for Registry<H> {
    fn default() -> Self {
        Self::new()
    }
}
