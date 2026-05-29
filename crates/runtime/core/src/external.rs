//! Third-party primitive extension infrastructure.
//!
//! Framework-core defines `Element::External` as a single escape
//! hatch for primitives the framework itself doesn't ship. Third-party
//! crates construct an External primitive whose payload is type-erased
//! props; backends consult their own [`ExternalRegistry`] to dispatch
//! the kind to a concrete builder closure.
//!
//! # Layering
//!
//! - **Framework-core** owns this module + the `Element::External`
//!   variant + the `Backend::create_external` trait method. It knows
//!   nothing about specific backends or specific external primitive
//!   types.
//! - **Each backend** holds an `ExternalRegistry<Self>` as a field and
//!   exposes inherent `register_external` / `has_external` methods,
//!   plus implements `Backend::create_external` to consult the
//!   registry (falling through to a platform-native "not supported"
//!   placeholder on a miss).
//! - **Third-party primitive crates** ship a facade (constructor + a
//!   props struct) plus N per-backend leaf crates whose `register`
//!   function calls `backend.register_external::<TheirProps>(...)`.
//! - **User apps** call the third-party umbrella's `register(...)` once
//!   per third-party SDK, regardless of platform.
//!
//! The contract: runtime-core stays platform-agnostic, the closed
//! `Element` enum stays closed, type erasure is paid at exactly one
//! line per backend (inside `register_external`), and user-facing code
//! is fully typed.

use crate::backend::Backend;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::rc::Rc;

/// A type-erased handler closure: takes the External primitive's
/// payload + a mutable borrow of the backend and produces the
/// backend's native node.
pub type ErasedHandler<B> = Rc<dyn Fn(&Rc<dyn Any>, &mut B) -> <B as Backend>::Node>;

/// Per-backend registry of third-party primitive handlers keyed by the
/// payload type's [`TypeId`]. Backends embed one of these as a field
/// and consult it from their `Backend::create_external` impl.
///
/// `TypeId` keying is collision-free by construction — two unrelated
/// crates registering an `idealyst-maps:map-view` kind would conflict
/// on a string-keyed registry, but their `MapViewProps` types have
/// distinct TypeIds (Rust's type system guarantees uniqueness).
pub struct ExternalRegistry<B: Backend + 'static> {
    handlers: HashMap<TypeId, ErasedHandler<B>>,
}

impl<B: Backend + 'static> ExternalRegistry<B> {
    pub fn new() -> Self {
        Self { handlers: HashMap::new() }
    }

    /// Register `handler` for payload type `T`. Returns the previously
    /// registered handler if `T` was already registered (typically
    /// `None`; non-`None` means the same SDK registered twice).
    pub fn register<T, F>(&mut self, handler: F) -> Option<ErasedHandler<B>>
    where
        T: 'static,
        F: Fn(&Rc<T>, &mut B) -> B::Node + 'static,
    {
        // Type-erasure happens here, at exactly one line per backend.
        // User-supplied `handler` stays fully typed; the closure we
        // store downcasts the payload back to `Rc<T>` on each
        // invocation. Downcast panics if the framework ever delivers
        // the wrong type — which it shouldn't, because the TypeId in
        // the Element::External matches the registered TypeId.
        let erased: ErasedHandler<B> = Rc::new(move |any, backend| {
            let typed: Rc<T> = any
                .clone()
                .downcast::<T>()
                .expect("external primitive payload type mismatch");
            handler(&typed, backend)
        });
        self.handlers.insert(TypeId::of::<T>(), erased)
    }

    /// Look up the handler for `type_id`. Returns a cloned `Rc` so the
    /// caller can release the registry borrow before invoking the
    /// handler (which itself needs `&mut B`).
    pub fn get(&self, type_id: TypeId) -> Option<ErasedHandler<B>> {
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

impl<B: Backend + 'static> Default for ExternalRegistry<B> {
    fn default() -> Self {
        Self::new()
    }
}

/// Backend-neutral registration seam for third-party `Element::External`
/// handlers — the external analogue of
/// [`RegisterNavigator`](crate::primitives::navigator::RegisterNavigator).
/// Lets an SDK write one `register<B: RegisterExternal>(b)` that works on
/// any backend (web, SSR, …) without naming the concrete backend type or
/// depending on a backend crate. Each backend that owns an
/// [`ExternalRegistry`] implements this by forwarding to it.
pub trait RegisterExternal: Backend + Sized + 'static {
    fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&Rc<T>, &mut Self) -> Self::Node + 'static;
}

// =============================================================================
// ExternalHandle<T> — typed handle for `Bound<ExternalHandle<T>>` /
// `Ref<ExternalHandle<T>>`. The `T` parameter is a phantom marker so
// the type system can distinguish refs to different external kinds.
// =============================================================================

/// Handle to a mounted external primitive's backend node. The `T`
/// phantom parameter ties this handle to the payload type the third
/// party defined, so `Ref<ExternalHandle<MapViewProps>>` and
/// `Ref<ExternalHandle<CameraProps>>` are distinct types at the call
/// site.
///
/// The actual backend node (`web_sys::Element` / `UIView` / etc.) is
/// type-erased here so runtime-core stays platform-agnostic. Third
/// parties that want to expose backend-specific node access add
/// `#[cfg]`-gated accessor methods in their facade crate.
pub struct ExternalHandle<T> {
    node: Rc<dyn Any>,
    _marker: std::marker::PhantomData<T>,
}

impl<T> Clone for ExternalHandle<T> {
    fn clone(&self) -> Self {
        Self { node: self.node.clone(), _marker: std::marker::PhantomData }
    }
}

impl<T: 'static> ExternalHandle<T> {
    pub fn new(node: Rc<dyn Any>) -> Self {
        Self { node, _marker: std::marker::PhantomData }
    }

    /// Type-erased access to the backend's node. Third-party facades
    /// `downcast_ref` this to the backend-specific node type they
    /// expect (under `#[cfg]` so user code only sees the per-platform
    /// type for the current target).
    pub fn node(&self) -> &dyn Any {
        &*self.node
    }

    /// Returns the underlying `Rc<dyn Any>` for facades that want to
    /// hold a shared reference to the node.
    pub fn node_rc(&self) -> Rc<dyn Any> {
        self.node.clone()
    }
}

// =============================================================================
// Constructor + builder
// =============================================================================

use crate::builder::Bound;
use crate::handles::RefFill;
use crate::element::Element;
use crate::reactive::Ref;

/// Build a third-party `Element::External` whose payload type is
/// `T`. Returns a `Bound<ExternalHandle<T>>` so `.bind(...)` is type-
/// checked against the call-site `Ref<ExternalHandle<T>>`.
///
/// The framework captures `TypeId::of::<T>()` and
/// `std::any::type_name::<T>()` at construction. Backends dispatch on
/// the `TypeId` via their `ExternalRegistry`; the type name is for
/// debug/error messages.
///
/// ```ignore
/// pub struct MapViewProps { lat: f64, lon: f64, zoom: f32 }
/// let view = external(MapViewProps { lat: 37.7749, lon: -122.4194, zoom: 12.0 });
/// ```
pub fn external<T: 'static>(props: T) -> Bound<ExternalHandle<T>> {
    Bound::new(Element::External {
        type_id: TypeId::of::<T>(),
        type_name: std::any::type_name::<T>(),
        payload: Rc::new(props) as Rc<dyn Any>,
        children: Vec::new(),
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
    })
}

impl<T: 'static> Bound<ExternalHandle<T>> {
    /// Bind a `Ref<ExternalHandle<T>>` for imperative access to the
    /// mounted external primitive's backend node.
    pub fn bind(mut self, r: Ref<ExternalHandle<T>>) -> Self {
        if let Element::External { ref_fill, .. } = self.primitive_mut() {
            *ref_fill = Some(RefFill::External(Box::new(move |node_any| {
                r.fill(ExternalHandle::<T>::new(node_any));
            })));
        }
        self
    }

    /// Supply framework children to be parented into the backend node
    /// this external's handler returns. Leaf widgets (maps, webview)
    /// never call this; container kinds (a web `<form>`) pass the
    /// inputs/buttons that must be real descendants of the returned
    /// node. The handler's returned node is the parent — see
    /// [`crate::walker`]'s external build path.
    pub fn children(mut self, children: Vec<Element>) -> Self {
        if let Element::External { children: slot, .. } = self.primitive_mut() {
            *slot = children;
        }
        self
    }
}

