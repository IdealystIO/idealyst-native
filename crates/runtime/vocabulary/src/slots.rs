//! Slots — publishing content into a region an ancestor owns.
//!
//! A component often needs to contribute UI to a region it does not
//! render: a page adding buttons to the app shell's header, a screen
//! adding breadcrumbs to a toolbar, a form adding a "Save" action to a
//! sticky footer. Props can't carry that when the region's owner is
//! several levels up and the intermediate components know nothing about
//! the content.
//!
//! Slots close that gap without a portal. This is pure scene-level
//! composition — a signal, a context entry, and a keyed list. No new
//! primitive, no backend method, no window-level mount. (A `portal`
//! escapes to the *host root*; a slot renders inside the ordinary layout
//! tree, wherever the outlet sits.)
//!
//! # Why this is in the framework, not a component library
//!
//! A slot is an **interop protocol** between components that don't know
//! each other. If it shipped inside one component library, a page built
//! on library A could not fill a region rendered by library B, and an app
//! using neither would have to invent a third incompatible copy. A
//! protocol belongs below the libraries that speak it.
//!
//! It earns the placement without growing the parts of the framework that
//! CLAUDE.md §3 protects: no primitive, no payload type, no `Host` method,
//! no capability-trait method, no backend work. It sits in `glue` next to
//! `reducer` / `watch` / `when` / `each_keyed` — authoring composition over
//! the kernel — and, being generic over the slot type, codegens nothing at
//! all for an app that never names one.
//!
//! # Usage
//!
//! A slot is identified by a marker **type**, not a string — the same
//! rule `provide`/`inject` documents for disambiguation. Two crates can
//! both ship a `HeaderActions` slot without colliding, and a typo is a
//! compile error rather than a silently-empty region.
//!
//! ```ignore
//! use runtime_core::{fill_slot, slot_outlet, Slot};
//!
//! // 1. Declare the slot (one line, anywhere both sides can name it).
//! pub struct HeaderActions;
//! impl Slot for HeaderActions {}
//!
//! // 2. The owner renders the outlet.
//! #[component]
//! fn AppShell(children: Vec<Element>) -> Element {
//!     ui! {
//!         view(style = shell) {
//!             view(style = header) {
//!                 Typography(content = "My App".to_string())
//!                 slot_outlet::<HeaderActions>()
//!             }
//!             view(style = body) { children }
//!         }
//!     }
//! }
//!
//! // 3. Any descendant fills it, from its component body.
//! #[component]
//! fn SettingsPage() -> Element {
//!     let dirty = signal(false);
//!     fill_slot::<HeaderActions>(move || ui! {
//!         Button(label = "Save", disabled = !dirty.get(), on_click = save)
//!     });
//!     ui! { view() { /* …page body… */ } }
//! }
//! ```
//!
//! The fill is withdrawn automatically when `SettingsPage` unmounts —
//! navigate away and the header button disappears with the page.
//!
//! # What a fill actually stores
//!
//! A **builder**, `Rc<dyn Fn() -> Element>` — never an `Element`. An
//! `Element` is a single-use blueprint that `realize` consumes; it isn't
//! `Clone` and can't be parked in a signal and re-rendered. Every
//! render-elsewhere mechanism in this repo carries a builder for that
//! reason (see `ToastEntry::render`).
//!
//! Because the builder is a closure, reactivity works the way it does
//! everywhere else: capture the filler's signals and the slot content
//! re-renders on its own. Conditional content goes *inside* the builder
//! (`ui!`'s `if` / `when`), not around the `fill_slot` call — the call
//! runs once, at build.
//!
//! # Ordering, and why the feed is world-lifetime
//!
//! Publish and render are order-independent: the feed for a slot type is
//! created lazily at world root by whichever side touches it first, so it
//! does not matter whether the filler or the outlet runs first. Scoping
//! the feed to the outlet's component instead would have made
//! `ui! { AppShell { SettingsPage() } }` fail outright: a `ui!` children
//! block builds bottom-up, so `SettingsPage`'s body runs *before*
//! `AppShell`'s and would find no provider to inject.
//!
//! A fill is an ordinary staged write, so content appears on the next
//! flush — including on the very first mount. That is not a visible
//! flash: an app's boot path realizes the tree and flushes before the
//! first paint (`backend_web::start_in` does exactly this), so the slot
//! is populated by the time anything renders. In a test harness it means
//! `mount` then `flush` before asserting.
//!
//! World-lifetime is the same call the toast queue makes, for the same
//! reason, and it carries the same consequence: **one outlet per slot
//! type per world.** Mount two and both render the same content. Two
//! regions that should hold different content are two slot types — which
//! costs one line each. [`slot_outlet`] logs a warning if it catches a
//! second live outlet.
//!
//! The feed holds one arena slot per slot type for the life of the
//! world. Entries themselves are reclaimed on withdrawal.
//!
//! # Scope inversion — the one sharp edge
//!
//! The content subtree is realized inside the **outlet's** scope, while
//! the builder captures signals owned by the **filler**. The filler is
//! usually the shorter-lived of the two, so withdrawal has to be exact:
//! if a dead filler's entry stayed in the feed, a later rebuild would
//! read through a freed handle and abort with `stale-signal-handle`.
//!
//! [`fill_slot`] anchors withdrawal with `on_scope_drop`, so the entry
//! leaves the feed in the same flush that tears the filler down and the
//! content unrealizes with it. That is why filling is a call in a
//! component *body* — the body's scope is the lifetime being tracked.
//! Calling it from a detached position (outside any world) is inert.
//!
//! # When NOT to use a slot
//!
//! If the region's owner is reachable through props, pass an element or a
//! builder prop instead — `Alert`'s `action`, `List`'s `leading`, or
//! `Autocomplete`'s `header`/`footer` builders. Those are direct, need no
//! shared key, and place no constraint on how many hosts exist. Slots are
//! for the case where props genuinely cannot reach.

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

use runtime_scene::Element;
use runtime_shared::log_warn;
use runtime_world::{inject, on_scope_drop, provide, signal, unscoped, Signal};

use crate::glue::{each_keyed, EachKey, EachRowBuild};

// =============================================================================
// Slot identity
// =============================================================================

/// Marks a type as a slot key.
///
/// Implement it on a unit struct; the type is the slot's identity, so
/// nothing else is needed:
///
/// ```ignore
/// pub struct HeaderActions;
/// impl Slot for HeaderActions {}
/// ```
///
/// The trait is an explicit opt-in rather than a blanket `T: 'static`
/// bound so a slot key can't be confused with an unrelated context type,
/// and so `fill_slot::<String>(…)` doesn't compile.
pub trait Slot: 'static {}

// =============================================================================
// Feed
// =============================================================================

/// One published contribution to a slot.
#[derive(Clone)]
struct SlotEntry {
    /// Process-unique, monotonic. Doubles as the keyed list's row key and
    /// as the tie-break that keeps equal-`order` fills in publish order.
    seq: u64,
    order: i32,
    build: Rc<dyn Fn() -> Element>,
}

/// The feed's signal carries `Vec<SlotEntry>`, and the kernel's guarded
/// `set` needs `PartialEq` on the payload. `seq` alone identifies an
/// entry (it is monotonic and never reused), but `order` and closure
/// identity are compared too so a same-`seq` rewrite can't be mistaken
/// for a no-op write. Closure identity is pointer equality — the only
/// honest comparison available for an `Rc<dyn Fn>`. Same shape as
/// `ToastEntry`'s impl, for the same reason.
impl PartialEq for SlotEntry {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq
            && self.order == other.order
            && Rc::ptr_eq(&self.build, &other.build)
    }
}

/// The per-slot-type context entry: the published entries plus a live
/// count of mounted outlets (for the duplicate-outlet diagnostic).
struct SlotFeed<S: Slot> {
    entries: Signal<Vec<SlotEntry>>,
    outlets: Rc<Cell<usize>>,
    /// `fn() -> S` rather than `S` so the marker imposes no variance or
    /// auto-trait obligations on the key type.
    _slot: PhantomData<fn() -> S>,
}

// Hand-written: `#[derive(Clone)]` on a generic struct would demand
// `S: Clone`, which a marker type has no reason to be.
impl<S: Slot> Clone for SlotFeed<S> {
    fn clone(&self) -> Self {
        SlotFeed {
            entries: self.entries,
            outlets: Rc::clone(&self.outlets),
            _slot: PhantomData,
        }
    }
}

thread_local! {
    static NEXT_SEQ: Cell<u64> = const { Cell::new(0) };
}

fn next_seq() -> u64 {
    NEXT_SEQ.with(|n| {
        let v = n.get();
        n.set(v.wrapping_add(1));
        v
    })
}

/// The feed for slot `S` in the ambient world, created on first touch.
///
/// World-rooted via `unscope` — see the module docs. A scope-owned
/// provision would die with whichever side happened to touch the slot
/// first, and the other side would then build a second, unrendered feed.
///
/// Requires an ambient world (`inject` and `signal` both do). Every
/// public entry point here is documented as body/render-position only.
fn feed<S: Slot>() -> SlotFeed<S> {
    if let Some(existing) = inject::<SlotFeed<S>>() {
        return existing;
    }
    unscoped(|| {
        let created = SlotFeed::<S> {
            entries: signal(Vec::new()),
            outlets: Rc::new(Cell::new(0)),
            _slot: PhantomData,
        };
        provide(created.clone());
        created
    })
}

// =============================================================================
// Filling
// =============================================================================

/// Publish `content` into slot `S`, withdrawing it when the calling
/// scope drops.
///
/// Call from a component body. The closure is the *content builder* — it
/// runs where the outlet renders, not here, and re-runs only if the
/// contribution is remounted, so reactive content should read its
/// signals inside the closure (or inside the `ui!` tree it returns).
///
/// Several components may fill the same slot; all of them render. Use
/// [`fill_slot_at`] to control their relative order.
///
/// ```ignore
/// fill_slot::<HeaderActions>(move || ui! {
///     Button(label = "Save", on_click = save.clone())
/// });
/// ```
pub fn fill_slot<S: Slot>(content: impl Fn() -> Element + 'static) {
    fill_slot_at::<S>(0, content);
}

/// [`fill_slot`] with an explicit sort key.
///
/// Entries render by ascending `order`; ties break by publish order, so
/// equal-`order` fills keep the sequence they were published in. Order is
/// a plain `i32` so a contribution can deliberately sit before the
/// default-`0` ones (`-10`) without every other call site being renumbered.
pub fn fill_slot_at<S: Slot>(order: i32, content: impl Fn() -> Element + 'static) {
    let entries = feed::<S>().entries;
    let seq = next_seq();
    let entry = SlotEntry {
        seq,
        order,
        build: Rc::new(content),
    };
    // `update`, not `set(get() + …)`: it reads the STAGED value (falling
    // back to the committed one), so two fills published in the same turn
    // compose instead of the second clobbering the first. Two sibling
    // components filling one slot during a single build is the normal
    // case, not an edge case.
    entries.update(move |list| {
        let mut next = list.clone();
        next.push(entry);
        next
    });
    // The withdrawal is the load-bearing half: the builder captures this
    // scope's signals, and they are freed when the scope drops. Removing
    // the entry in the same flush is what keeps the outlet from rebuilding
    // a row over freed handles (`stale-signal-handle`).
    //
    // `on_scope_drop`, not `on_cleanup`: a component body is not an effect
    // run, and `on_cleanup` panics there. Inside an effect, `on_scope_drop`
    // defers to `on_cleanup` on its own, which is also correct — a fill
    // made during an effect run belongs to that run.
    on_scope_drop(move || {
        entries.update(move |list| list.iter().filter(|e| e.seq != seq).cloned().collect());
    });
}

// =============================================================================
// Rendering
// =============================================================================

/// Render everything published into slot `S`, in order.
///
/// Returns an element to drop wherever the content belongs — no wrapper
/// needed, so the entries become children of whatever encloses it:
///
/// ```ignore
/// ui! {
///     view(style = header_bar) {
///         Typography(content = title)
///         slot_outlet::<HeaderActions>()
///     }
/// }
/// ```
///
/// The outlet is a keyed list, which inherits the same host-dependent
/// behavior every `for` in `ui!` has. Where the host splices children —
/// web, iOS, Android, macOS — the entries are spliced directly into the
/// enclosing parent, an empty slot occupies nothing at all, and a fill
/// that stays published keeps its live subtree (and its local state)
/// while a *different* fill arrives or leaves. On a host without splice
/// support (`Host::supports_splice() == false`, e.g. the wgpu scene) the
/// list lives under one anchor node and rebuilds wholesale on every
/// change. Slot content should therefore not assume its subtree survives
/// an unrelated fill on every backend — put state that must persist in
/// the *filler's* scope, where the builder captures it, rather than
/// inside the built content.
///
/// Mount exactly one outlet per slot type; see the module docs on why the
/// feed is world-lifetime. A second live outlet renders duplicate content
/// and logs a warning.
pub fn slot_outlet<S: Slot>() -> Element {
    let feed = feed::<S>();
    let entries = feed.entries;

    // Duplicate-outlet diagnostic. The counter tracks LIVE outlets, so a
    // remount (route swap back to the same shell) doesn't accumulate.
    let outlets = feed.outlets;
    outlets.set(outlets.get() + 1);
    if outlets.get() > 1 {
        log_warn!(
            "slot_outlet::<{}>: {} outlets are mounted for this slot — every one of \
             them renders the same content. A slot's feed is world-lifetime, so two \
             regions that should hold different content need two slot types.",
            std::any::type_name::<S>(),
            outlets.get()
        );
    }
    let release = Rc::clone(&outlets);
    on_scope_drop(move || release.set(release.get().saturating_sub(1)));

    each_keyed(move || {
        let mut list = entries.get();
        // `seq` is monotonic, so it is also the publish order — the tie
        // break that makes equal-`order` fills stable rather than
        // dependent on whatever order the feed happens to hold.
        list.sort_by_key(|e| (e.order, e.seq));
        list.into_iter()
            .map(|entry| {
                let build = entry.build;
                let row: EachRowBuild = Box::new(move || vec![build()]);
                (EachKey::new(entry.seq), row)
            })
            .collect()
    })
}

/// Whether slot `S` currently has any content, as a **tracked** read.
///
/// For chrome that should collapse when nothing is published — a header
/// bar with padding and a border shouldn't render an empty strip:
///
/// ```ignore
/// ui! {
///     view() {
///         if slot_is_filled::<HeaderActions>() {
///             view(style = header_bar) { slot_outlet::<HeaderActions>() }
///         }
///     }
/// }
/// ```
///
/// Because the read is tracked, the branch re-evaluates as fills come and
/// go — no manual subscription.
pub fn slot_is_filled<S: Slot>() -> bool {
    feed::<S>().entries.with(|list| !list.is_empty())
}
