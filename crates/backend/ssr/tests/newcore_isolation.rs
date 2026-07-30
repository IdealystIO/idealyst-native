//! Per-request world isolation — the multi-world payoff the SSR port
//! exists for (design §10 P5): many `runtime_world::World`s per thread,
//! independent flush/teardown, zero cross-talk, zero accumulation.
//!
//! Covers the wave's isolation gates:
//! - two worlds alive simultaneously on one thread, interleaved renders
//!   and flushes, no cross-talk (structure, signals, or theme);
//! - sequential renders do not leak (weak-probe proves each request's
//!   world-owned state is freed before `render_path` returns — the
//!   observable equivalent of "world registry size stable", which has no
//!   public counter);
//! - a signal-driven dyn branch renders the COMMITTED initial state and
//!   the request's effects do not outlive the request (cleanups fire,
//!   post-render writes are dead-world no-ops).


use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use backend_ssr::SsrBackend;
use runtime_vocabulary::caps::LifecycleOps;
use runtime_scene::{dyn_keyed, realize, Registry};
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::register_builtins;
use runtime_world::{effect, signal, Signal, World};

/// One manually-driven SSR "request" whose world is kept alive so a test
/// can interleave it with another. `html()` reserializes the live tree
/// (nodes are shared handles, so post-flush mutations are visible).
struct LiveRequest {
    /// Field order is drop order: the realized tree unmounts (cleanups
    /// fire) BEFORE the world — its slots' owner — dies. Held for drop
    /// semantics only, hence the underscore.
    _realized: runtime_scene::Realized<backend_ssr::NodeRef>,
    backend: Rc<RefCell<SsrBackend>>,
    world: World,
}

impl LiveRequest {
    fn mount(build: impl FnOnce() -> runtime_scene::Element) -> Self {
        let backend = Rc::new(RefCell::new(SsrBackend::new()));
        let mut registry: Registry<SsrBackend> = Registry::new();
        register_builtins(&mut registry);
        let registry = Rc::new(registry);
        let world = World::new();
        let realized = world.enter(|| realize(&backend, &registry, build()));
        let mut roots = realized.collect_nodes();
        assert_eq!(roots.len(), 1, "single-root test trees");
        LifecycleOps::finish(&mut *backend.borrow_mut(), roots.pop().expect("len checked"));
        world.flush();
        LiveRequest { _realized: realized, backend, world }
    }

    fn html(&self) -> String {
        self.backend.borrow().into_html()
    }

    fn head_css(&self) -> String {
        self.backend.borrow().head_css()
    }
}

/// Two worlds alive AT THE SAME TIME on one thread: interleaved mounts,
/// writes, and flushes. Each world's signals commit only on its OWN
/// flush, and only its own backend's HTML changes; per-world theme state
/// (vocabulary `ThemeCtx` rides world context) never bleeds across.
#[test]
fn two_worlds_interleaved_renders_no_crosstalk() {
    let label_a: Rc<Cell<Option<Signal<String>>>> = Rc::new(Cell::new(None));
    let label_b: Rc<Cell<Option<Signal<String>>>> = Rc::new(Cell::new(None));

    // Token delivery to a backend rides the sheet-attach path (the
    // vocabulary theme drains pending host state when a sheet
    // registers), so each tree carries one sheet-styled root — same as
    // any real themed app.
    fn surface_sheet() -> runtime_shared::StyleApplication {
        runtime_shared::StyleApplication::new(Rc::new(runtime_shared::StyleSheet::new(|_vs| {
            runtime_shared::StyleRules {
                background: Some(runtime_shared::Tokenized::token(
                    "color-surface",
                    runtime_shared::Color("#000".into()),
                )),
                ..Default::default()
            }
        })))
    }

    let slot = label_a.clone();
    let req_a = LiveRequest::mount(move || {
        runtime_vocabulary::theme::install_tokens(&[runtime_shared::TokenEntry {
            name: "color-surface",
            value: runtime_shared::TokenValue::Color(runtime_shared::Color("#a0a0a0".into())),
        }]);
        let label = signal(String::from("a-initial"));
        slot.set(Some(label));
        view()
            .style(surface_sheet())
            .child(text().content(move || label.get()))
            .build()
    });
    let slot = label_b.clone();
    let req_b = LiveRequest::mount(move || {
        runtime_vocabulary::theme::install_tokens(&[runtime_shared::TokenEntry {
            name: "color-surface",
            value: runtime_shared::TokenValue::Color(runtime_shared::Color("#b1b1b1".into())),
        }]);
        let label = signal(String::from("b-initial"));
        slot.set(Some(label));
        view()
            .style(surface_sheet())
            .child(text().content(move || label.get()))
            .build()
    });

    assert!(req_a.html().contains("a-initial"));
    assert!(req_b.html().contains("b-initial"));

    // Handles route to their OWN world: write A, flush ONLY A.
    label_a.get().expect("captured").set("a-updated".into());
    label_b.get().expect("captured").set("b-updated".into());
    req_a.world.flush();

    assert!(req_a.html().contains("a-updated"), "A committed on A's flush: {}", req_a.html());
    assert!(
        req_b.html().contains("b-initial"),
        "B's staged write must NOT commit on A's flush: {}",
        req_b.html()
    );

    req_b.world.flush();
    assert!(req_b.html().contains("b-updated"), "B commits on B's flush: {}", req_b.html());

    // Theme isolation: each backend saw only its own world's tokens.
    assert!(req_a.head_css().contains("--color-surface:#a0a0a0;"), "{}", req_a.head_css());
    assert!(req_b.head_css().contains("--color-surface:#b1b1b1;"), "{}", req_b.head_css());
    assert!(!req_a.head_css().contains("#b1b1b1"));
    assert!(!req_b.head_css().contains("#a0a0a0"));

    // Dropping A does not disturb B.
    drop(req_a);
    label_b.get().expect("captured").set("b-final".into());
    req_b.world.flush();
    assert!(req_b.html().contains("b-final"), "B survives A's teardown: {}", req_b.html());
}

/// 100 sequential renders through the public per-request entry: each
/// request's world-owned state is freed before `render_path` returns
/// (the weak probe upgrades to `None` — the effect closure holding the
/// Rc died with the world), and the output stays byte-stable. This is
/// the "worlds must not accumulate" gate: a leaked world would keep its
/// effects (and the probe Rc) alive.
#[test]
fn sequential_renders_do_not_leak() {
    let mut first: Option<String> = None;
    for i in 0..100 {
        let probe: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let weak: Weak<Cell<u32>> = Rc::downgrade(&probe);
        let page = backend_ssr::newcore::render_path("/", move || {
            let count = signal(0u32);
            // World-root-owned effect capturing the probe Rc: it can only
            // die with the world.
            effect(move || {
                probe.set(probe.get() + count.get());
            });
            view().child(text().content("stable")).build()
        });
        assert!(
            weak.upgrade().is_none(),
            "render {i}: the request's world leaked (effect closure still alive)"
        );
        match &first {
            None => first = Some(page.html),
            Some(f) => assert_eq!(f, &page.html, "render {i}: output drifted"),
        }
    }
}

/// A dyn branch chosen by a signal renders the COMMITTED initial state;
/// the request's effects run during the request only — cleanups fire at
/// teardown, and post-render writes on smuggled handles are dead-world
/// no-ops (no panic, no observable change).
#[test]
fn signal_dyn_branch_renders_committed_state_and_effects_die() {
    let cleaned: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let fires: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let smuggled: Rc<Cell<Option<Signal<bool>>>> = Rc::new(Cell::new(None));

    let cleaned2 = cleaned.clone();
    let fires2 = fires.clone();
    let smuggled2 = smuggled.clone();
    let page = backend_ssr::newcore::render_path("/", move || {
        let on = signal(false);
        smuggled2.set(Some(on));
        let fires = fires2.clone();
        let cleaned = cleaned2.clone();
        effect(move || {
            let _ = on.get();
            fires.set(fires.get() + 1);
            let cleaned = cleaned.clone();
            move || cleaned.set(true)
        });
        view()
            .child(dyn_keyed(
                move || on.get(),
                |&on| {
                    if on {
                        text().content("branch-on").build()
                    } else {
                        text().content("branch-off").build()
                    }
                },
            ))
            .build()
    });

    assert!(page.html.contains("branch-off"), "committed initial state renders: {}", page.html);
    assert!(!page.html.contains("branch-on"), "untaken branch absent: {}", page.html);
    assert!(cleaned.get(), "effect cleanup fired at request teardown");

    let fired_during = fires.get();
    assert!(fired_during >= 1, "effect ran during the request");
    // Post-request write on the smuggled handle: the world is dead, so
    // this is the kernel's documented silent no-op — no panic, and the
    // effect can never fire again.
    smuggled.get().expect("captured").set(true);
    assert_eq!(fires.get(), fired_during, "effects must not outlive the request");
}
