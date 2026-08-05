//! The abstract scene tree: [`Element`], its constructors, and [`Key`].
//!
//! An `Element` is a single-use blueprint — [`realize`](crate::realize)
//! consumes it. Only [`Element::Item`] carries a payload a platform ever
//! sees; `Fragment`/`Dyn`/`Keyed`/`Owned` are pure structure the scene
//! manages itself.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_world::Owned;

/// The blueprint a component returns, generic over any primitive
/// vocabulary. The scene never interprets payloads: constructors stay
/// typed, and the [`Registry`](crate::Registry) handler downcasts once at
/// the platform boundary.
pub enum Element {
    /// A primitive from some vocabulary, plus children. The only variant
    /// that becomes a real [`Host`](crate::Host) node.
    Item {
        data: Box<dyn Any>,
        children: Vec<Element>,
    },
    /// Siblings with no node of their own. Spliced flat into the enclosing
    /// children list; the threaded `inserted` index counts THROUGH it so a
    /// reactive region after a fragment captures the correct absolute
    /// base index (pinned by `fragment_base_index.spliced.golden`).
    Fragment(Vec<Element>),
    /// A structural hole: a subtree that re-realizes when the dependencies
    /// of its [`DynSpec`] change. The old subtree unrealizes (effects
    /// drop, cleanups fire) and the new one is realized fresh. Built via
    /// [`dyn_element`] (rebuild on every fire) or [`dyn_keyed`] (guarded).
    Dyn(DynSpec),
    /// A keyed reactive list: `items` re-runs reactively; each row's
    /// subtree is kept, created, or dropped by [`Key`] identity. Erased
    /// like `Item` — build through [`keyed`], which owns the typing.
    Keyed {
        items: Box<dyn Fn() -> Vec<(Key, Box<dyn Any>)>>,
        render: Box<dyn Fn(Box<dyn Any>) -> Element>,
    },
    /// A component boundary: a subtree plus the [`Owned`] scope its
    /// component body created (signals AND effects, per the kernel's
    /// collector). The scope is folded into the enclosing
    /// [`Realized`](crate::Realized) at realize time and lives exactly as
    /// long as the subtree stays realized.
    Owned { element: Box<Element>, owned: Owned },
    /// A **multi-node** primitive: one payload that mounts N sibling
    /// nodes directly into the enclosing parent through a
    /// [`register_many`](crate::Registry::register_many) handler. This is
    /// the scene seam for the old core's `Element::Repeat` (the static
    /// `for i in 0..n` lowering): the handler gets parent access, so a
    /// batching backend can collapse the whole expansion into one FFI
    /// round-trip instead of N per-node mounts.
    ///
    /// Children-list ONLY, exactly like the old `Repeat`: it stands for
    /// N sibling nodes, never a single subtree root, so realizing one as
    /// a detached root (navigator screen, keyed row root, spliced hole
    /// root) panics — the same contract the old walker enforced.
    Many { data: Box<dyn Any> },
}

/// A primitive item: typed payload + children. The payload's `TypeId` is
/// the [`Registry`](crate::Registry) dispatch key.
pub fn item(data: impl Any, children: Vec<Element>) -> Element {
    Element::Item {
        data: Box::new(data),
        children,
    }
}

/// Siblings with no node of their own.
pub fn fragment(children: Vec<Element>) -> Element {
    Element::Fragment(children)
}

/// A multi-node primitive item (see [`Element::Many`]): the payload's
/// `TypeId` dispatches to the handler registered via
/// [`register_many`](crate::Registry::register_many).
pub fn many(data: impl Any) -> Element {
    Element::Many {
        data: Box::new(data),
    }
}

/// Attach a component body's collected scope to its returned element —
/// the manual form of [`component_scope`].
pub fn owned(element: Element, owned: Owned) -> Element {
    Element::Owned {
        element: Box::new(element),
        owned,
    }
}

/// Run a component body — untracked, with every signal/effect it creates
/// collected — and attach the collected scope to the returned element.
/// This is what `#[component]` will wrap function bodies in (P6). Port of
/// idea-lite's `component_scope`, on the kernel's
/// [`component_scope`](runtime_world::component_scope) collector.
pub fn component_scope(f: impl FnOnce() -> Element) -> Element {
    let (element, owned) = runtime_world::component_scope(f);
    if owned.is_empty() {
        element
    } else {
        Element::Owned {
            element: Box::new(element),
            owned,
        }
    }
}

// ============================================================================
// Dyn — the structural hole's spec.
// ============================================================================

/// A retire hook: receives the swapped-out subtree as a boxed
/// [`Retired<N>`](crate::Retired) (type-erased — `Element` is
/// node-agnostic; downcast to `Retired<YourNode>`) INSTEAD of the driver
/// tearing it down. The hook owns BOTH halves of the teardown:
///
/// - **drop timing** — the hook holds the `Realized` while an exit
///   animation plays and drops it in the `after_ms` task;
/// - **structural detachment** — the driver does NOT `remove_child` /
///   `clear_children` the outgoing nodes; the hook removes them (the
///   `Retired` carries `parent` + `nodes`) when the animation completes.
///   Detaching eagerly would defeat the hook's whole purpose: an exit
///   animation on an already-detached node is invisible (removed from
///   the DOM / view tree before a single frame renders).
///
/// Because the outgoing nodes stay attached while the incoming branch
/// builds, a retire hole should be the sole child of a dedicated
/// container (the presence placeholder pattern) so the transient
/// old+new coexistence can't skew sibling indices.
pub type RetireHook = Box<dyn FnMut(Box<dyn Any>)>;

/// How a Dyn hole decides and builds. The build closure is invoked by the
/// driver INSIDE the fresh subtree's collector (`collect_owned`) — element
/// construction runs in the new scope, exactly like the old walker's
/// `with_scope(|| ...builder()...)`, so effects a builder creates (probe
/// cleanups, component effects) die with the subtree they belong to.
pub(crate) enum DynKind {
    /// Rebuild on EVERY fire; the closure runs TRACKED (inside the fresh
    /// scope) — its eager signal reads are the rebuild dependencies
    /// (`dynamic.rs` semantics).
    Plain(Box<dyn Fn() -> Element>),
    /// Guarded: `changed` runs TRACKED, BEFORE any teardown, and reports
    /// whether the guard value moved; on `false` the driver does nothing
    /// structurally. `build` then runs UNTRACKED inside the fresh scope
    /// (`when_switch.rs` semantics: only the predicate's reads are deps).
    Guarded {
        changed: Box<dyn Fn() -> bool>,
        build: Box<dyn Fn() -> Element>,
    },
}

/// The spec a Dyn driver executes.
pub struct DynSpec {
    pub(crate) kind: DynKind,
    pub(crate) retire: Option<RetireHook>,
}

/// A structural hole that rebuilds on EVERY dependency change. The closure
/// runs tracked (its eager signal reads are the rebuild deps) — there is no
/// output dedup, matching the old `Element::Dynamic` ("the closure IS the
/// dependency source"; pinned by `dynamic_swap.anchored.golden`).
///
/// Deliberately explicit: swapping a subtree is heavier than rebinding a
/// value prop, so it should be visible at the call site.
pub fn dyn_element(f: impl Fn() -> Element + 'static) -> Element {
    Element::Dyn(DynSpec {
        kind: DynKind::Plain(Box::new(f)),
        retire: None,
    })
}

/// A GUARDED structural hole: `select` runs tracked and produces a guard
/// value; the subtree is torn down and rebuilt (via `build`, run
/// UNTRACKED) only when that value actually changes. A re-fire whose guard
/// value is unchanged does nothing structurally.
///
/// This is the old walker's `last_active` dedup (`when_switch.rs`): a
/// `when` predicate that reads extra signals beyond its boolean (a
/// `version` tick, say) must not recreate the branch's gesture nodes
/// mid-press when only the tick changed. `when(cond, then, else)` lowers
/// to `dyn_keyed(cond, |&b| ...)`; a literal-armed `switch` lowers to
/// `dyn_keyed(discriminant, ...)`.
pub fn dyn_keyed<K, S, B>(select: S, build: B) -> Element
where
    K: PartialEq + 'static,
    S: Fn() -> K + 'static,
    B: Fn(&K) -> Element + 'static,
{
    // Per-hole guard state: an Element is single-use (realize consumes
    // it), so the cell is exactly one driver effect's `last_active`. It is
    // shared between the decision closure (writes the key) and the build
    // closure (reads it) — the driver calls them in that order.
    let last: Rc<RefCell<Option<K>>> = Rc::new(RefCell::new(None));
    let last_build = Rc::clone(&last);
    Element::Dyn(DynSpec {
        kind: DynKind::Guarded {
            changed: Box::new(move || {
                let key = select(); // tracked by the driver effect
                if last.borrow().as_ref() == Some(&key) {
                    return false;
                }
                *last.borrow_mut() = Some(key);
                true
            }),
            build: Box::new(move || {
                let guard = last_build.borrow();
                build(guard.as_ref().expect("guard key is set before build"))
            }),
        },
        retire: None,
    })
}

impl Element {
    /// Install a retire hook on a `Dyn` hole (see [`RetireHook`]). The
    /// swapped-out subtree is handed to the hook (as a boxed
    /// [`Retired<N>`](crate::Retired)) with its nodes STILL ATTACHED;
    /// the hook owns detachment + drop timing. Default (no hook): the
    /// driver detaches and drops immediately.
    ///
    /// Panics on non-`Dyn` elements — retiring is a structural-hole
    /// concept only.
    pub fn with_retire(self, hook: impl FnMut(Box<dyn Any>) + 'static) -> Element {
        match self {
            Element::Dyn(mut spec) => {
                spec.retire = Some(Box::new(hook));
                Element::Dyn(spec)
            }
            _ => panic!("Element::with_retire: retire hooks only apply to Dyn elements"),
        }
    }
}

// ============================================================================
// Keyed — typed constructor + Key.
// ============================================================================

/// A keyed, reactive list. `items` is tracked: whenever a signal it reads
/// changes, the list reconciles. Each item is identified by `key(&item)`:
///
/// - a NEW key realizes `render(item)` fresh (mount),
/// - a key that DISAPPEARS drops its subtree (unmount: effects die,
///   cleanups fire),
/// - a KEPT key reuses its live subtree untouched — `render` is NOT
///   called again, so row-local signals and effects survive edits
///   elsewhere in the list.
///
/// A row body that is a [`fragment`] contributes several flat sibling
/// nodes; the reconciler accounts in per-row node *vectors*
/// (`each_multi_node_rows.spliced.golden`).
///
/// The key function is required on purpose: positional identity silently
/// hands one row's live state to another when items shift.
pub fn keyed<T, I, KF, K, RF>(items: I, key: KF, render: RF) -> Element
where
    T: 'static,
    I: Fn() -> Vec<T> + 'static,
    KF: Fn(&T) -> K + 'static,
    K: Into<Key>,
    RF: Fn(T) -> Element + 'static,
{
    Element::Keyed {
        items: Box::new(move || {
            items()
                .into_iter()
                .map(|item| (key(&item).into(), Box::new(item) as Box<dyn Any>))
                .collect()
        }),
        render: Box::new(move |data| {
            let item = *data.downcast::<T>().expect("keyed list: item type mismatch");
            render(item)
        }),
    }
}

/// A list item's identity. Every keyed item must produce one, and keys
/// must be unique within their list — reconciliation panics on a
/// duplicate.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    Int(i64),
    UInt(u64),
    Str(String),
}

macro_rules! impl_key_from {
    ($variant:ident : $repr:ty => $($t:ty),+) => {$(
        impl From<$t> for Key {
            fn from(value: $t) -> Key {
                Key::$variant(value as $repr)
            }
        }
    )+};
}

impl_key_from!(Int: i64 => i8, i16, i32, i64);
impl_key_from!(UInt: u64 => u8, u16, u32, u64, usize);

impl From<&str> for Key {
    fn from(value: &str) -> Key {
        Key::Str(value.to_string())
    }
}

impl From<String> for Key {
    fn from(value: String) -> Key {
        Key::Str(value)
    }
}
