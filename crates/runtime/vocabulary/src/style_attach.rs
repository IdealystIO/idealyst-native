//! Shared style-attachment service for the built-in handlers (P2b) —
//! the vocabulary-level port of `walker/style.rs::attach_style`.
//!
//! # The two P2 paths
//!
//! [`StyleProp`] models exactly the two paths the P2 phase ports:
//!
//! - **`Static`** — fully resolved [`StyleRules`], applied once at mount
//!   (`StyleOps::apply_style`), with `StyleOps::on_node_unstyled` fired at
//!   subtree teardown. This is the backend-visible contract of the old
//!   static path (`attach_style_static` → `apply_one` → `apply_style`).
//! - **`Dynamic`** — a rules closure re-applied by a binding effect
//!   whenever its dependencies change, plus the interaction-state hookup
//!   the old reactive path installs: a per-node `StateBits` signal, the
//!   `StyleOps::attach_states` setter that flips it (a native
//!   hover/press/focus event re-fires the binding effect and re-applies),
//!   and `on_node_unstyled` at teardown. This is the backend-visible
//!   contract of `attach_style_reactive` on event-driven backends.
//!
//! # Deferred (documented, not silent)
//!
//! The old walker layers several more style services that are **not**
//! ported here — they are later-phase policy, listed in the crate docs'
//! deferred set:
//!
//! - sheet registration + theme-cohort re-application (`ensure_registered_with`,
//!   `register_stylesheet`, token installs) — P3 style-engine policy;
//!   `StyleProp` deliberately carries *resolved* rules, not sheets;
//! - class minting / `SignalClass` / preminted classes — P3 web policy;
//! - state/breakpoint/container **overlay resolution** — the Dynamic path
//!   keeps the state *signal and re-fire contract* (so the seam is in
//!   place) but has no sheet to resolve overlays from until the sheet
//!   model migrates;
//! - the native container-query inline-size feedback loop.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{StateBits, StyleRules};
use runtime_world::effect;

use crate::caps::StyleOps;

/// A primitive's style prop: resolved rules, applied once (`Static`) or
/// re-applied by a binding effect (`Dynamic`). See the module docs for
/// the exact backend-call contract of each path.
pub enum StyleProp {
    /// Resolved rules applied once at mount.
    Static(Rc<StyleRules>),
    /// A rules closure; a binding effect re-applies on every dependency
    /// change (signal reads inside the closure are the dependencies).
    Dynamic(Box<dyn Fn() -> Rc<StyleRules>>),
}

/// Conversions into [`StyleProp`] — what builders' `.style(...)` accepts:
/// `StyleRules` / `Rc<StyleRules>` (static), or a closure returning
/// either (dynamic).
pub trait IntoStyleProp {
    fn into_style_prop(self) -> StyleProp;
}

impl IntoStyleProp for StyleProp {
    fn into_style_prop(self) -> StyleProp {
        self
    }
}

impl IntoStyleProp for StyleRules {
    fn into_style_prop(self) -> StyleProp {
        StyleProp::Static(Rc::new(self))
    }
}

impl IntoStyleProp for Rc<StyleRules> {
    fn into_style_prop(self) -> StyleProp {
        StyleProp::Static(self)
    }
}

/// What a dynamic style closure may return — sealed to the two rules
/// shapes so the blanket closure impl below stays unambiguous.
pub trait IntoStyleRules: sealed::Sealed {
    fn into_style_rules(self) -> Rc<StyleRules>;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for runtime_core::StyleRules {}
    impl Sealed for std::rc::Rc<runtime_core::StyleRules> {}
}

impl IntoStyleRules for StyleRules {
    fn into_style_rules(self) -> Rc<StyleRules> {
        Rc::new(self)
    }
}

impl IntoStyleRules for Rc<StyleRules> {
    fn into_style_rules(self) -> Rc<StyleRules> {
        self
    }
}

impl<F, R> IntoStyleProp for F
where
    F: Fn() -> R + 'static,
    R: IntoStyleRules,
{
    fn into_style_prop(self) -> StyleProp {
        StyleProp::Dynamic(Box::new(move || self().into_style_rules()))
    }
}

/// Register `f` to run when the enclosing realized subtree tears down
/// (its [`Owned`](runtime_world::Owned) drops) — the new-core analogue of
/// the old walker's scope-level `on_cleanup`.
///
/// Mechanism: a probe effect whose body reads no signals (so it never
/// re-fires) and returns `f` as its cleanup; the kernel runs effect
/// cleanups when the collected effect is freed, i.e. exactly at subtree
/// teardown, in effect-creation order. Handlers may call this at mount
/// time (inside a `MountCx` handler there is no running effect, so the
/// kernel's `on_cleanup` free function is unusable directly — it needs a
/// running effect; the probe provides one).
pub fn on_teardown(f: impl FnOnce() + 'static) {
    let mut slot = Some(f);
    let _ = effect(move || {
        let f = slot.take();
        move || {
            if let Some(f) = f {
                f();
            }
        }
    });
}

/// Attach `style` to an already-created `node`. Port of the old walker's
/// `attach_style` restricted to the two P2 paths (module docs).
///
/// Must run inside a `MountCx` handler (or any `collect_owned` scope with
/// the owning world ambient): the binding effect, the state signal, and
/// the teardown probe all register into the ambient collector and die
/// with the subtree.
pub fn attach_style<H: StyleOps>(backend: &Rc<RefCell<H>>, node: &H::Node, style: StyleProp) {
    match style {
        StyleProp::Static(rules) => {
            backend.borrow_mut().apply_style(node, &rules);
            // Teardown notification — the backend frees per-node style
            // state (the old path's `StyleHandle` drop).
            let b = backend.clone();
            let n = node.clone();
            on_teardown(move || {
                b.borrow_mut().on_node_unstyled(&n);
            });
        }
        StyleProp::Dynamic(f) => {
            // Per-node interaction-state signal. The binding effect
            // subscribes to it, so a native hover/press/focus flip
            // (delivered through the `attach_states` setter below)
            // re-fires the effect and re-applies — the same re-apply
            // contract as the old event-driven reactive path. Overlay
            // *resolution* from state bits is deferred with the sheet
            // model (module docs); the P2 closure returns the same rules
            // regardless of state.
            let states = runtime_world::signal(StateBits::NONE);
            let b = backend.clone();
            let n = node.clone();
            let _binding = effect(move || {
                let _ = states.get(); // subscribe: state flips re-apply
                let rules = f();
                b.borrow_mut().apply_style(&n, &rules);
            });
            // Teardown probe AFTER the binding effect, mirroring the old
            // path where `on_node_unstyled` fires from the style effect's
            // own teardown (the captured StyleHandle's drop).
            let b = backend.clone();
            let n = node.clone();
            on_teardown(move || {
                b.borrow_mut().on_node_unstyled(&n);
            });
            // Hand the backend the state setter. Writes stage into the
            // signal; the owning world's flush delivers the re-apply.
            let setter: Rc<dyn Fn(StateBits, bool)> = Rc::new(move |bit, on| {
                states.update(move |bits| if on { bits.with(bit) } else { bits.without(bit) });
            });
            backend.borrow_mut().attach_states(node, setter);
        }
    }
}
