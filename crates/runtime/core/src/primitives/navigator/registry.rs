//! Per-backend registry of navigator handlers, keyed by presentation
//! type. Parallel to [`crate::external::ExternalRegistry`] but with a
//! richer payload — navigators aren't opaque, so the registered value
//! is a *handler factory* (producing a [`NavigatorHandler`] trait
//! object) instead of a one-shot closure.
//!
//! # Layering
//!
//! - Framework-core owns this struct and the [`NavigatorHandler`] trait
//!   it dispatches to ([`super::host::NavigatorHandler`]).
//! - Each backend embeds a `NavigatorRegistry<Self>` as a field, exposes
//!   `register_navigator::<P, _>(factory)` and `has_navigator::<P>()`,
//!   and implements `Backend::create_navigator` to consult
//!   the registry (falling through to a "not supported" placeholder on
//!   a miss).
//! - Each navigator-kind SDK ships per-backend modules whose `register`
//!   function calls the backend's inherent `register_navigator`. App
//!   bootstrap calls `stack_navigator::register(&mut backend)` once
//!   per kind.
//!
//! # TypeId keying
//!
//! `TypeId` is collision-free across crates — two SDKs both shipping a
//! `Presentation` struct have distinct `TypeId`s because the structs
//! live in different crates' type namespaces. The user-facing builder
//! captures its own `Presentation`'s `TypeId` at construction, so
//! dispatch is unambiguous even when many SDKs are registered.

use super::host::NavigatorHandler;
use crate::backend::Backend;
use std::any::TypeId;
use std::collections::HashMap;
use std::rc::Rc;

/// Factory closure that produces a fresh handler per navigator
/// instance. `Rc` because dispatch borrows the registry, then drops
/// the borrow before calling the factory (which itself needs `&mut B`
/// via `init`).
pub type NavigatorHandlerFactory<B> = Rc<dyn Fn() -> Box<dyn NavigatorHandler<B>>>;

/// Per-backend registry of navigator handler factories keyed by
/// presentation `TypeId`. Backends own one as a field; SDKs register
/// into it during app bootstrap.
pub struct NavigatorRegistry<B: Backend + 'static> {
    factories: HashMap<TypeId, NavigatorHandlerFactory<B>>,
}

impl<B: Backend + 'static> NavigatorRegistry<B> {
    pub fn new() -> Self {
        Self { factories: HashMap::new() }
    }

    /// Register a navigator kind. `P` is the SDK's presentation
    /// payload type; `factory` produces a fresh handler per
    /// `Element::Navigator { type_id: TypeId::of::<P>(), .. }`
    /// mounted in the tree. Returns the previously-registered factory
    /// if `P` was already registered (typically `None`; non-`None`
    /// means the same SDK registered twice).
    pub fn register<P, F>(&mut self, factory: F) -> Option<NavigatorHandlerFactory<B>>
    where
        P: 'static,
        F: Fn() -> Box<dyn NavigatorHandler<B>> + 'static,
    {
        self.factories.insert(TypeId::of::<P>(), Rc::new(factory))
    }

    /// Look up a factory by presentation `TypeId`. Returns a cloned
    /// `Rc` so the backend can drop the registry borrow before calling
    /// the factory (which itself needs `&mut B` through `init`).
    pub fn get(&self, type_id: TypeId) -> Option<NavigatorHandlerFactory<B>> {
        self.factories.get(&type_id).cloned()
    }

    /// `true` if a handler factory is registered for `P`.
    pub fn has<P: 'static>(&self) -> bool {
        self.factories.contains_key(&TypeId::of::<P>())
    }

    /// `true` if a handler factory is registered for `type_id`.
    pub fn has_id(&self, type_id: TypeId) -> bool {
        self.factories.contains_key(&type_id)
    }
}

impl<B: Backend + 'static> Default for NavigatorRegistry<B> {
    fn default() -> Self {
        Self::new()
    }
}
