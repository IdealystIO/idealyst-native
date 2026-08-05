//! The TypeId-keyed handler [`Registry`] — the scene's ONLY mount path.
//!
//! Generalizes runtime-core's `ExternalRegistry` (external.rs) into the
//! universal primitive contract: every payload type — framework-shipped or
//! third-party — registers one handler per backend, type erasure is paid
//! at exactly one line (inside [`Registry::register`]), and user-facing
//! constructors stay fully typed. "External" stops being a concept.
//!
//! # Two registration windows
//!
//! - **Boot** ([`register`](Registry::register) /
//!   [`register_many`](Registry::register_many)) takes `&mut self`, so it
//!   is only reachable before the registry is wrapped in an `Rc` and
//!   handed to [`realize`](crate::realize). Everything registered here is
//!   statically reachable from the boot entry, i.e. resident in
//!   `main.wasm` on web.
//! - **Late** ([`register_deferred`](Registry::register_deferred)) takes
//!   `&Rc<Self>` and works at any time, including from inside a lazily
//!   loaded wasm chunk. It exists for exactly one reason: bundle size. A
//!   handler that main never names is a handler wasm-split can place
//!   outside `main.wasm` (see the crate docs and
//!   `tests/lazy-payload-split`).
//!
//! Late registration is **opt-in per payload kind**: the app declares the
//! kind at boot with [`defer`](Registry::defer), which is what licenses
//! [`realize`](crate::realize) to *park* an item of that kind instead of
//! panicking. A payload with neither a handler nor a declaration still
//! panics — panicking on a genuine mistake is the feature, and the
//! declaration is what distinguishes "arriving later" from "you forgot".

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::element::Element;
use crate::host::Host;
use crate::realize::{DeferredSlot, MountCx};

/// A type-erased mount handler: receives the mount context (backend access
/// + child realization), the item's payload, and its children; returns the
/// real node. Handlers own the full mount sequence for their primitive
/// (create → bind props → realize children → attach handlers).
pub type Handler<H> =
    Rc<dyn Fn(&mut MountCx<'_, H>, Rc<dyn Any>, Vec<Element>) -> <H as Host>::Node>;

/// A type-erased MULTI-NODE mount handler (see [`Element::Many`]): receives
/// the mount context, the payload, and the REAL parent node, mounts N
/// sibling nodes into it (batched or per-node — the handler's call), and
/// returns the resulting live subtree plus the number of top-level nodes it
/// contributed to `parent` (the child-splice loop's `inserted` accounting;
/// a following reactive region's base index depends on it).
pub type ManyHandler<H> = Rc<
    dyn Fn(
        &mut MountCx<'_, H>,
        Rc<dyn Any>,
        &mut <H as Host>::Node,
    ) -> (crate::realize::LiveNode<<H as Host>::Node>, usize),
>;

/// One item parked awaiting its late handler. The Registry holds a WEAK
/// handle to the slot: the strong one lives in the enclosing tree's
/// [`LiveNode`](crate::LiveNode), so a subtree that unmounts before its
/// chunk loads takes its parked items with it and the drain simply skips
/// them (upgrade fails) instead of realizing into a detached parent.
pub(crate) struct Parked<H: Host> {
    pub(crate) backend: Rc<RefCell<H>>,
    pub(crate) slot: Weak<RefCell<DeferredSlot<H::Node>>>,
}

/// Per-backend registry of primitive handlers keyed by the payload type's
/// [`TypeId`]. TypeId keying is collision-free by construction: two
/// unrelated crates' `MapViewProps` types have distinct TypeIds, where a
/// string-keyed registry would conflict.
pub struct Registry<H: Host> {
    /// Boot handlers. Deliberately NOT behind a `RefCell`: `&mut self`
    /// registration is what makes "frozen after boot" checkable, and it
    /// keeps the hot mount lookup a plain map probe. Late handlers live
    /// in their own map so this one keeps meaning "resident in the boot
    /// module" (see [`handler_count`](Self::handler_count)).
    handlers: FxHashMap<TypeId, Handler<H>>,
    many_handlers: FxHashMap<TypeId, ManyHandler<H>>,
    /// Payload kinds the app declared late-bound at boot ([`defer`]).
    ///
    /// [`defer`]: Self::defer
    deferred: FxHashSet<TypeId>,
    /// Handlers installed after boot ([`register_deferred`]). Consulted
    /// only when `handlers` misses, so an app that never defers pays
    /// nothing on the mount path.
    ///
    /// [`register_deferred`]: Self::register_deferred
    late: RefCell<FxHashMap<TypeId, Handler<H>>>,
    /// Items realize met before their handler arrived, in DOCUMENT ORDER
    /// per kind (the realize walk is a document-order walk, and the drain
    /// replays that order).
    parked: RefCell<FxHashMap<TypeId, Vec<Parked<H>>>>,
}

/// The one line where type erasure is paid. Shared by the boot and late
/// registration paths so both store byte-identical closures — that
/// identity is what makes a parked-then-drained mount indistinguishable
/// from an eager one.
fn erase<H, T, F>(handler: F) -> Handler<H>
where
    H: Host,
    T: 'static,
    F: Fn(&mut MountCx<'_, H>, &Rc<T>, Vec<Element>) -> H::Node + 'static,
{
    Rc::new(move |cx, any, children| {
        // The stored closure downcasts the payload back on each mount; a
        // mismatch is impossible unless the registry map itself is
        // corrupted (the lookup key IS the payload's TypeId), so it panics
        // loudly as a kernel bug rather than degrading.
        let typed: Rc<T> = any.downcast::<T>().unwrap_or_else(|_| {
            panic!(
                "runtime-scene: item payload downcast mismatch for the handler registered \
                 under `{}` — the registry key and the stored closure disagree (scene bug)",
                std::any::type_name::<T>()
            )
        });
        handler(cx, &typed, children)
    })
}

impl<H: Host> Registry<H> {
    pub fn new() -> Self {
        Registry {
            handlers: FxHashMap::default(),
            many_handlers: FxHashMap::default(),
            deferred: FxHashSet::default(),
            late: RefCell::new(FxHashMap::default()),
            parked: RefCell::new(FxHashMap::default()),
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
        self.handlers.insert(TypeId::of::<T>(), erase::<H, T, F>(handler))
    }

    /// Register a MULTI-NODE handler for payload type `T` (dispatched by
    /// [`Element::Many`]). Same type-erasure discipline as
    /// [`register`](Self::register); returns any previously registered
    /// many-handler for `T`.
    ///
    /// Multi-node handlers cannot be deferred — see [`defer`](Self::defer).
    pub fn register_many<T, F>(&mut self, handler: F) -> Option<ManyHandler<H>>
    where
        T: 'static,
        F: Fn(
                &mut MountCx<'_, H>,
                &Rc<T>,
                &mut H::Node,
            ) -> (crate::realize::LiveNode<H::Node>, usize)
            + 'static,
    {
        let erased: ManyHandler<H> = Rc::new(move |cx, any, parent| {
            let typed: Rc<T> = any.downcast::<T>().unwrap_or_else(|_| {
                panic!(
                    "runtime-scene: Many payload downcast mismatch for the handler registered \
                     under `{}` — the registry key and the stored closure disagree (scene bug)",
                    std::any::type_name::<T>()
                )
            });
            handler(cx, &typed, parent)
        });
        self.many_handlers.insert(TypeId::of::<T>(), erased)
    }

    /// Declare payload type `T` **late-bound**: its handler ships outside
    /// the boot module and arrives via
    /// [`register_deferred`](Self::register_deferred) once the code that
    /// carries it loads. Until then, [`realize`](crate::realize) PARKS
    /// items of this kind (a layout-transparent placeholder holds their
    /// position) instead of panicking.
    ///
    /// This is the whole opt-in. A payload kind with no handler and no
    /// declaration still panics at realize — that panic is the framework's
    /// "you forgot to register your SDK" diagnostic and deferral must not
    /// weaken it into a silent blank.
    ///
    /// Boot-only (`&mut self`) on purpose: the set of kinds an app is
    /// willing to wait for is part of its static shape, so it is
    /// auditable from the boot entry alone. Declaring it costs main only
    /// the payload type's `TypeId` — a compile-time constant — which is
    /// precisely why the heavy handler can stay out of the main module.
    ///
    /// # Panics
    ///
    /// - if `T` already has a boot handler (declaring an eagerly
    ///   registered kind deferred is always a mistake);
    /// - if `T` has a MANY handler: a [`Element::Many`] stands for an
    ///   unknown number of sibling nodes, so a parked one cannot reserve
    ///   its slice of the parent's index space. Deferral is single-node
    ///   only. (Declaring a kind that is only ever used as a `Many`
    ///   payload and never registered still fails at realize with the
    ///   ordinary "no MANY handler registered" panic — deferral does not
    ///   apply to the `Many` arm at all.)
    pub fn defer<T: 'static>(&mut self) {
        let tid = TypeId::of::<T>();
        if self.handlers.contains_key(&tid) {
            panic!(
                "runtime-scene: `{}` is already registered as a boot handler — declaring it \
                 deferred as well is contradictory. Register it eagerly OR defer it, not both.",
                std::any::type_name::<T>()
            );
        }
        if self.many_handlers.contains_key(&tid) {
            panic!(
                "runtime-scene: `{}` is a MANY (multi-node) payload — deferral is single-node \
                 only, because a parked `Element::Many` cannot reserve its slice of the \
                 parent's child-index space while its handler is missing.",
                std::any::type_name::<T>()
            );
        }
        self.deferred.insert(tid);
    }

    /// Install a LATE handler for a kind previously declared with
    /// [`defer`](Self::defer), and immediately realize every item of that
    /// kind currently parked — in document order, in place, with no
    /// remount of anything else. Returns how many parked items realized.
    ///
    /// Takes `&Rc<Self>` rather than `&mut self`, which is the entire
    /// point: the registry is already shared and the tree is already
    /// live. Call it from wherever the handler's code actually lands —
    /// typically a `#[component(lazy)]` body, either directly (if the
    /// registry is in hand) or through
    /// [`defer_registration`](crate::defer_registration).
    ///
    /// Must run with the owning [`World`](runtime_world::World) ambient
    /// (inside `enter`, or inside one of its effects) — draining parked
    /// items realizes subtrees, and a realize creates effects. The
    /// [`defer_registration`](crate::defer_registration) mailbox
    /// guarantees this by construction, because realize is what drains
    /// it; a direct call from outside the framework's own callbacks must
    /// arrange it.
    ///
    /// Registering the same kind twice replaces the handler; already
    /// realized items are NOT re-mounted (chunks load once — a second
    /// registration is a re-entry, not a new intent).
    ///
    /// If `T` has a BOOT handler this is a no-op returning 0. That lets
    /// one SDK ship both paths — eager on native, late on web — without
    /// the caller branching: whichever registration the platform's boot
    /// took wins.
    ///
    /// # Panics
    ///
    /// If `T` was never declared with [`defer`](Self::defer). A late
    /// registration for an undeclared kind means realize would already
    /// have panicked on that payload, so accepting it silently would hide
    /// the missing declaration.
    pub fn register_deferred<T, F>(self: &Rc<Self>, handler: F) -> usize
    where
        T: 'static,
        F: Fn(&mut MountCx<'_, H>, &Rc<T>, Vec<Element>) -> H::Node + 'static,
    {
        let tid = TypeId::of::<T>();
        if self.handlers.contains_key(&tid) {
            return 0; // the boot registration already won — see the docs
        }
        if !self.deferred.contains(&tid) {
            panic!(
                "runtime-scene: late registration for `{}`, which was never declared deferred. \
                 Call `registry.defer::<{}>()` at the boot seam (register_scene_extensions / \
                 the boot entry's `register` argument) so realize parks the payload instead of \
                 panicking on it.",
                std::any::type_name::<T>(),
                std::any::type_name::<T>(),
            );
        }
        self.late
            .borrow_mut()
            .insert(tid, erase::<H, T, F>(handler));
        self.drain_parked(tid)
    }

    /// Realize every item parked under `type_id`, in park (= document)
    /// order. Each one re-enters the ordinary [`realize`](crate::realize)
    /// path with the element it was holding, so a drained mount emits the
    /// same handler calls, the same child walk and the same node shape an
    /// eager mount would; only the attach op differs (`insert_at` at the
    /// placeholder's index rather than an append).
    fn drain_parked(self: &Rc<Self>, type_id: TypeId) -> usize {
        let parked = self
            .parked
            .borrow_mut()
            .remove(&type_id)
            .unwrap_or_default();
        let mut realized = 0usize;
        for entry in parked {
            // Dead weak = the enclosing subtree unmounted while the chunk
            // was in flight. Skipping is the correct outcome: there is no
            // live parent to place the node into.
            let Some(slot) = entry.slot.upgrade() else {
                continue;
            };
            crate::realize::realize_parked(&entry.backend, self, &slot);
            realized += 1;
        }
        realized
    }

    /// Record a parked item (realize-internal).
    pub(crate) fn park(
        &self,
        type_id: TypeId,
        backend: Rc<RefCell<H>>,
        slot: &Rc<RefCell<DeferredSlot<H::Node>>>,
    ) {
        let mut map = self.parked.borrow_mut();
        let list = map.entry(type_id).or_default();
        // Prune entries whose subtree unmounted before the chunk arrived,
        // so a long-lived registry with a churning deferred region does
        // not accumulate dead weaks.
        list.retain(|p| p.slot.strong_count() > 0);
        list.push(Parked {
            backend,
            slot: Rc::downgrade(slot),
        });
    }

    /// Look up the handler for `type_id` — boot map first, then late.
    /// Returns a cloned `Rc` so the caller can release the registry borrow
    /// before invoking the handler.
    pub fn get(&self, type_id: TypeId) -> Option<Handler<H>> {
        match self.handlers.get(&type_id) {
            Some(h) => Some(h.clone()),
            // Miss only: an app with no deferred kinds never reaches the
            // RefCell at all.
            None => self.late.borrow().get(&type_id).cloned(),
        }
    }

    /// Look up the MULTI-NODE handler for `type_id`.
    pub fn get_many(&self, type_id: TypeId) -> Option<ManyHandler<H>> {
        self.many_handlers.get(&type_id).cloned()
    }

    /// `true` if `T` has a registered handler (boot or late).
    pub fn has<T: 'static>(&self) -> bool {
        self.has_id(TypeId::of::<T>())
    }

    /// `true` if any payload with this `type_id` has a registered handler
    /// (boot or late).
    pub fn has_id(&self, type_id: TypeId) -> bool {
        self.handlers.contains_key(&type_id) || self.late.borrow().contains_key(&type_id)
    }

    /// `true` if `type_id` was declared late-bound via
    /// [`defer`](Self::defer) — i.e. realize may park it.
    pub fn is_deferred(&self, type_id: TypeId) -> bool {
        self.deferred.contains(&type_id)
    }

    /// How many single-node handlers the BOOT seam installed.
    ///
    /// Boot registration takes `&mut self` and is therefore only reachable
    /// before the registry is shared, which makes this count exactly the
    /// set of handlers — and everything they transitively reach — that is
    /// statically reachable from the boot entry and so resident in
    /// `main.wasm`. That is what the always-resident gate pins
    /// (`runtime-vocabulary/tests/builtin_surface.rs`).
    ///
    /// Handlers installed later through
    /// [`register_deferred`](Self::register_deferred) are deliberately NOT
    /// counted here: not being reachable from boot is their whole purpose,
    /// so folding them in would defeat the measurement. See
    /// [`late_handler_count`](Self::late_handler_count).
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }

    /// How many MULTI-NODE ([`Element::Many`]) handlers are installed.
    /// See [`handler_count`](Self::handler_count) for why this is pinned.
    pub fn many_handler_count(&self) -> usize {
        self.many_handlers.len()
    }

    /// How many handlers arrived after boot via
    /// [`register_deferred`](Self::register_deferred).
    pub fn late_handler_count(&self) -> usize {
        self.late.borrow().len()
    }

    /// How many payload kinds were declared late-bound via
    /// [`defer`](Self::defer).
    pub fn deferred_kind_count(&self) -> usize {
        self.deferred.len()
    }

    /// How many items of type `T` are currently parked awaiting their
    /// handler (diagnostics + tests).
    pub fn parked_count<T: 'static>(&self) -> usize {
        self.parked
            .borrow()
            .get(&TypeId::of::<T>())
            .map(|list| list.iter().filter(|p| p.slot.strong_count() > 0).count())
            .unwrap_or(0)
    }
}

impl<H: Host> Default for Registry<H> {
    fn default() -> Self {
        Self::new()
    }
}
