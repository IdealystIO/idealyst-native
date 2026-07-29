//! New-core adoption tests for the wgpu backend (idea-lite migration,
//! P5) — the GPU counterpart of the `backend-macos` newcore suite.
//!
//! Unlike AppKit (whose views only exist on the main thread, so the
//! macOS caps delegation is gated behind a launched smoke app), the
//! `WgpuBackend` is a plain host-side value: the full boot path —
//! `newcore::start` → registry dispatch → caps delegation → `finish` →
//! flush driver — runs headlessly here, and structural assertions read
//! the live `NodeData` tree the way the macOS smoke reads NSViews.
//!
//! Integration test (own process) ON PURPOSE: these tests install a
//! buffering test scheduler via the global first-install-wins
//! `runtime_core::scheduling::install_scheduler` slot. The lib unit
//! tests rely on the no-scheduler synchronous fallback; sharing a
//! process would make their behavior depend on test ordering.
//!
//! The buffering `TestScheduler` mirrors the shape of the production
//! schedulers the flush driver rides: `schedule_microtask` queues (like
//! the winit host's 0 ms timers and the headless scheduler's buffer)
//! and `drain_buffered_microtasks` pumps — so "staged, not committed
//! until the driver's microtask runs" is observable, which the
//! synchronous fallback would hide.

#![cfg(feature = "new-core")]

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use render_wgpu::newcore::{self, NewCoreApp};
use render_wgpu::{NativeSkin, NodeKind, WgpuBackend, WgpuNode};
use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_core::{ColorScheme, Length, Platform, StyleRules, Tokenized};
use runtime_scene::keyed;
use runtime_vocabulary::builders::IntoSceneElement;
use runtime_vocabulary::{button, pressable, slider, text, text_input, toggle, view};
use runtime_world::signal;

// ===========================================================================
// Test scheduler — buffering, thread-local, manually pumped
// ===========================================================================

thread_local! {
    static MICROTASKS: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> =
        RefCell::new(VecDeque::new());
    /// Total microtasks ever queued on this thread — the dedup probe.
    static MICROTASK_COUNT: Cell<usize> = const { Cell::new(0) };
    static TIMERS: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> =
        RefCell::new(VecDeque::new());
}

struct TestScheduler;

struct InertHandle;
impl ScheduleHandle for InertHandle {
    fn cancel(&mut self) {}
}

// SAFETY: same rationale as the winit scheduler — the struct is
// zero-sized; all live state is thread-local, so the required
// `Send + Sync` bound carries no shared data.
unsafe impl Send for TestScheduler {}
unsafe impl Sync for TestScheduler {}

impl Scheduler for TestScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        MICROTASK_COUNT.with(|c| c.set(c.get() + 1));
        MICROTASKS.with(|q| q.borrow_mut().push_back(f));
    }

    fn after_animation_frame(&self, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        TIMERS.with(|q| q.borrow_mut().push_back(f));
        Box::new(InertHandle)
    }

    fn after_ms(&self, _delay_ms: i32, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        TIMERS.with(|q| q.borrow_mut().push_back(f));
        Box::new(InertHandle)
    }

    fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(InertHandle)
    }

    fn drain_buffered_microtasks(&self) {
        // Loop, not a single pass — a drained microtask may schedule
        // further microtasks (same contract as the headless scheduler).
        loop {
            let next = MICROTASKS.with(|q| q.borrow_mut().pop_front());
            match next {
                Some(f) => f(),
                None => break,
            }
        }
    }
}

fn ensure_test_scheduler() {
    // First-install-wins inside runtime_core; calling per test keeps
    // any test-thread able to boot first.
    install_scheduler(Box::new(TestScheduler));
}

/// Pump every queued flush/build microtask (the test stand-in for the
/// winit event loop draining its 0 ms timers).
fn drain() {
    runtime_core::scheduling::drain_buffered_microtasks();
}

fn microtasks_queued() -> usize {
    MICROTASK_COUNT.with(|c| c.get())
}

/// Pump queued `after_ms` / frame one-shots.
fn pump_timers() {
    loop {
        let next = TIMERS.with(|q| q.borrow_mut().pop_front());
        match next {
            Some(f) => f(),
            None => break,
        }
    }
}

// ===========================================================================
// Boot + tree helpers
// ===========================================================================

/// Production-shaped backend: `Host::new` builds the `WgpuBackend`,
/// loads the bundled font, and installs the global self-handle —
/// exactly what the windowed/headless hosts do before `newcore::start`.
fn make_backend() -> Rc<RefCell<WgpuBackend>> {
    ensure_test_scheduler();
    let host = render_wgpu::Host::new(
        Rc::new(NativeSkin::new(Platform::MacOs)),
        ColorScheme::Light,
    );
    host.backend().clone()
}

fn boot(build: impl FnOnce() -> runtime_scene::Element) -> NewCoreApp {
    newcore::start(make_backend(), |_| {}, build)
}

/// Depth-first collect of every text node's content under `node`.
fn collect_texts(node: &WgpuNode, out: &mut Vec<String>) {
    if let NodeKind::Text { content } = &node.borrow().kind {
        out.push(content.clone());
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in &children {
        collect_texts(child, out);
    }
}

fn texts_of(root: &WgpuNode) -> Vec<String> {
    let mut out = Vec::new();
    collect_texts(root, &mut out);
    out
}

/// Total live nodes in the subtree (the GPU analogue of the macOS
/// smoke's NSView count).
fn count_nodes(node: &WgpuNode) -> usize {
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    1 + children.iter().map(count_nodes).sum::<usize>()
}

fn root_of(app: &NewCoreApp) -> WgpuNode {
    let mut nodes = app.with_realized(|r| r.collect_nodes());
    assert_eq!(nodes.len(), 1, "single-root contract");
    nodes.pop().expect("len checked")
}

/// Find the first node matching `pred`, depth-first.
fn find_node(node: &WgpuNode, pred: &dyn Fn(&NodeKind) -> bool) -> Option<WgpuNode> {
    if pred(&node.borrow().kind) {
        return Some(node.clone());
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in &children {
        if let Some(hit) = find_node(child, pred) {
            return Some(hit);
        }
    }
    None
}

// ===========================================================================
// Flush driver
// ===========================================================================

/// `schedule_flush` queues exactly ONE deduped microtask no matter how
/// many staged writes ask for it, nothing commits until the driver's
/// microtask runs, and the dedup flag re-arms for the next cycle —
/// the exact interplay the winit event loop relies on.
#[test]
fn schedule_flush_dedups_and_commits_on_drain() {
    let slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app = boot(move || {
        let count = signal(0i32);
        s.set(Some(count));
        view()
            .child(text().content(move || format!("count = {}", count.get())))
            .build()
    });
    let count = slot.get().expect("signal smuggled out of build");
    let root = root_of(&app);
    assert_eq!(texts_of(&root), vec!["count = 0".to_string()]);

    // Stage twice, schedule twice: the second schedule_flush must
    // dedup, and nothing commits until the drain.
    let before = microtasks_queued();
    count.set(1);
    count.set(2);
    newcore::schedule_flush();
    newcore::schedule_flush();
    assert_eq!(
        microtasks_queued() - before,
        1,
        "second schedule_flush deduped (one queued microtask)"
    );
    assert_eq!(
        texts_of(&root),
        vec!["count = 0".to_string()],
        "staged, not committed"
    );

    drain();
    assert_eq!(
        texts_of(&root),
        vec!["count = 2".to_string()],
        "ONE flush committed the latest staged value"
    );

    // The flag re-arms for the next write→flush cycle.
    let before = microtasks_queued();
    count.set(3);
    newcore::schedule_flush();
    assert_eq!(microtasks_queued() - before, 1, "flag re-armed");
    drain();
    assert_eq!(texts_of(&root), vec!["count = 3".to_string()]);

    app.stop();
}

/// `flush_sync` / `schedule_flush` with no mounted app are no-ops (the
/// scheduler can fire before `start` finishes wiring on a cold boot),
/// and a re-entrant flush from inside an effect is skipped via
/// `world.is_flushing()`.
#[test]
fn flush_tolerates_no_world_and_reentry() {
    ensure_test_scheduler();
    // No app mounted on this thread: must not panic.
    newcore::flush_sync();
    newcore::schedule_flush();
    drain();
    assert!(!newcore::is_booted());

    let observed: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let obs = observed.clone();
    let slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app = boot(move || {
        let sig = signal(0i32);
        s.set(Some(sig));
        runtime_world::effect(move || {
            if sig.get() == 1 {
                // Re-entrant flush attempt from inside a flush:
                // world.is_flushing() short-circuits it.
                newcore::flush_sync();
                obs.set(true);
            }
        });
        view().child(text().content("x")).build()
    });
    assert!(newcore::is_booted());
    let sig = slot.get().expect("signal");
    sig.set(1);
    newcore::flush_sync();
    assert!(observed.get(), "effect ran; re-entrant flush didn't recurse/panic");
    app.stop();
    assert!(!newcore::is_booted(), "stop unhooks the driver");
}

/// The post-dispatch hook slot: `fire` is a no-op until installed,
/// dispatches to the installed fn, and `clear` reverts. (`start`
/// installs `schedule_flush` here; the winit scheduler fires it.)
#[test]
fn dispatch_hook_installs_fires_and_clears() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static FIRED: AtomicUsize = AtomicUsize::new(0);
    fn probe() {
        FIRED.fetch_add(1, Ordering::SeqCst);
    }

    render_wgpu::dispatch_hook::clear_dispatch_hook();
    render_wgpu::dispatch_hook::fire_dispatch_hook(); // no-op
    assert_eq!(FIRED.load(Ordering::SeqCst), 0);

    render_wgpu::dispatch_hook::install_dispatch_hook(probe);
    render_wgpu::dispatch_hook::fire_dispatch_hook();
    render_wgpu::dispatch_hook::fire_dispatch_hook();
    assert_eq!(FIRED.load(Ordering::SeqCst), 2);

    render_wgpu::dispatch_hook::clear_dispatch_hook();
    render_wgpu::dispatch_hook::fire_dispatch_hook();
    assert_eq!(FIRED.load(Ordering::SeqCst), 2, "cleared hook is inert");
}

/// An `after_ms`-staged write commits through the dispatch-hook route:
/// the timer callback runs author code, the hook (installed by `start`)
/// schedules the flush, the drain commits — the exact chain the iOS /
/// Android smokes proved live, reproduced host-side here.
#[test]
fn timer_staged_write_commits_via_dispatch_hook_route() {
    let slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app = boot(move || {
        let count = signal(0i32);
        s.set(Some(count));
        view()
            .child(text().content(move || format!("n={}", count.get())))
            .build()
    });
    let count = slot.get().expect("signal");
    let root = root_of(&app);

    // Author schedules a timer that stages a write (a debounce shape).
    runtime_core::scheduling::after_ms_detached(0, move || {
        count.set(7);
        // The production scheduler (host-winit) fires the hook after
        // this callback returns; the test scheduler is minimal, so
        // fire it the same way the wrapped `after_ms` closure does.
        render_wgpu::dispatch_hook::fire_dispatch_hook();
    });
    pump_timers();
    assert_eq!(texts_of(&root), vec!["n=0".to_string()], "staged only");
    drain();
    assert_eq!(
        texts_of(&root),
        vec!["n=7".to_string()],
        "hook → schedule_flush → drain committed the timer's write"
    );
    app.stop();
}

// ===========================================================================
// Boot path + caps delegation (structural)
// ===========================================================================

/// The vocabulary-builder smoke tree realizes through registry dispatch
/// into a live WgpuNode tree: single root recorded via `finish`, every
/// primitive present with its payload delegated to the Backend bodies.
#[test]
fn realize_builds_live_node_tree_and_finish_records_root() {
    let app = boot(|| {
        view()
            .style(StyleRules {
                padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
                gap: Some(Tokenized::Literal(Length::Px(8.0))),
                ..StyleRules::default()
            })
            .child(text().content("New-core GPU smoke"))
            .child(button().label("Increment").on_press(|| {}))
            .child(toggle().value(signal(false)).on_change(|_| {}))
            .build()
    });
    let root = root_of(&app);

    // `finish` recorded the root for the renderer's frame walk.
    let backend_root = newcore::with_backend(|b| b.borrow().root()).flatten();
    let backend_root = backend_root.expect("finish recorded a root");
    assert!(
        Rc::ptr_eq(&backend_root, &root),
        "backend.roots holds the realized root"
    );

    // Style delegated through StyleOps::apply_style.
    assert!(root.borrow().style.is_some(), "author style applied to root");

    // Children: text + button + toggle, payloads intact.
    assert_eq!(texts_of(&root), vec!["New-core GPU smoke".to_string()]);
    assert!(
        find_node(&root, &|k| matches!(k, NodeKind::Button { label, .. } if label == "Increment"))
            .is_some(),
        "button realized with label"
    );
    assert!(
        find_node(&root, &|k| matches!(k, NodeKind::Toggle { value: false, .. })).is_some(),
        "toggle realized with initial value"
    );
    assert!(count_nodes(&root) >= 4, "root + three primitive children");
    app.stop();
}

/// Two apps on the same thread: `stop` then `start` again — the flush
/// driver re-targets the new world (regression guard for a stale
/// FLUSH_WORLD surviving `stop`).
#[test]
fn restart_retargets_flush_driver() {
    let app1 = boot(|| view().child(text().content("first")).build());
    assert!(newcore::is_booted());
    app1.stop();
    assert!(!newcore::is_booted());

    let slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app2 = boot(move || {
        let n = signal(0i32);
        s.set(Some(n));
        view()
            .child(text().content(move || format!("second {}", n.get())))
            .build()
    });
    let n = slot.get().expect("signal");
    let root = root_of(&app2);
    n.set(9);
    newcore::schedule_flush();
    drain();
    assert_eq!(texts_of(&root), vec!["second 9".to_string()]);
    app2.stop();
}

/// Multi-root builds violate the single-root mount contract and panic
/// loudly (same message shape as web/macOS).
#[test]
#[should_panic(expected = "exactly one")]
fn multi_root_build_panics() {
    let _ = boot(|| {
        runtime_scene::fragment(vec![
            text().content("a").build(),
            text().content("b").build(),
        ])
    });
}

// ===========================================================================
// Dispatch-site glue — wrapped author callbacks drive the flush
// ===========================================================================

/// Pressing a button (invoking the installed `on_click`, exactly what
/// `Host::pointer_up` does after hit-testing) stages the write and the
/// wrapper's `schedule_flush` commits it on the next drain — the whole
/// event → staged write → driver flush → `update_text` chain.
#[test]
fn button_press_commits_through_wrapped_on_click() {
    let slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app = boot(move || {
        let count = signal(0i32);
        s.set(Some(count));
        view()
            .child(text().content(move || format!("count = {}", count.get())))
            .child(button().label("Increment").on_press(move || {
                count.update(|n| n + 1);
            }))
            .build()
    });
    let root = root_of(&app);
    let button_node = find_node(&root, &|k| matches!(k, NodeKind::Button { .. }))
        .expect("button in tree");
    let on_click = match &button_node.borrow().kind {
        NodeKind::Button { on_click, .. } => on_click.clone(),
        _ => unreachable!(),
    };

    let before = microtasks_queued();
    on_click(); // what the interaction Host fires on release
    assert_eq!(
        microtasks_queued() - before,
        1,
        "wrapped on_click queued exactly one flush microtask"
    );
    assert_eq!(
        texts_of(&root),
        vec!["count = 0".to_string()],
        "staged during dispatch, not yet committed"
    );
    drain();
    assert_eq!(
        texts_of(&root),
        vec!["count = 1".to_string()],
        "flush committed + TextOps::update_text applied"
    );

    // Burst: three presses before the drain = one commit at +3.
    on_click();
    on_click();
    on_click();
    drain();
    assert_eq!(texts_of(&root), vec!["count = 4".to_string()]);
    app.stop();
}

/// A pressable's wrapped `on_click` drives the flush the same way
/// (PressableOps path, distinct from ButtonOps' Action wrapping).
#[test]
fn pressable_click_commits_through_wrapped_on_click() {
    let slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app = boot(move || {
        let hits = signal(0i32);
        s.set(Some(hits));
        view()
            .child(text().content(move || format!("hits={}", hits.get())))
            .child(
                pressable(move || hits.update(|n| n + 1))
                    .child(text().content("press me")),
            )
            .build()
    });
    let root = root_of(&app);
    let node = find_node(&root, &|k| matches!(k, NodeKind::Pressable { .. }))
        .expect("pressable in tree");
    let on_click = match &node.borrow().kind {
        NodeKind::Pressable { on_click } => on_click.clone(),
        _ => unreachable!(),
    };
    on_click();
    drain();
    assert!(texts_of(&root).contains(&"hits=1".to_string()));
    app.stop();
}

/// The wrapped toggle `on_change` commits the staged write AND the
/// two-way value effect pushes the committed value back through
/// `ToggleOps::update_toggle_value` onto the node.
#[test]
fn toggle_change_commits_and_updates_node_value() {
    let slot: Rc<Cell<Option<runtime_world::Signal<bool>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app = boot(move || {
        let on = signal(false);
        s.set(Some(on));
        view()
            .child(toggle().value(on).on_change(move |v| on.set(v)))
            .build()
    });
    let on = slot.get().expect("signal");
    let root = root_of(&app);
    let node = find_node(&root, &|k| matches!(k, NodeKind::Toggle { .. })).expect("toggle");
    let on_change = match &node.borrow().kind {
        NodeKind::Toggle { on_change, .. } => on_change.clone(),
        _ => unreachable!(),
    };

    on_change(true); // what the Host's FlipToggle release fires
    drain();
    assert!(on.peek(), "author signal committed");
    assert!(
        matches!(node.borrow().kind, NodeKind::Toggle { value: true, .. }),
        "controlled value round-tripped onto the node"
    );
    app.stop();
}

/// The wrapped slider `on_change` (the `flushing1::<f32>` glue) commits
/// and the controlled-value effect round-trips onto the node via
/// `SliderOps::update_slider_value`.
#[test]
fn slider_change_commits_and_updates_node_value() {
    let slot: Rc<Cell<Option<runtime_world::Signal<f32>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app = boot(move || {
        let v = signal(0.25f32);
        s.set(Some(v));
        view()
            .child(
                slider()
                    .value(v)
                    .range(0.0, 1.0)
                    .on_change(move |x| v.set(x)),
            )
            .build()
    });
    let v = slot.get().expect("signal");
    let root = root_of(&app);
    let node = find_node(&root, &|k| matches!(k, NodeKind::Slider { .. })).expect("slider");
    let on_change = match &node.borrow().kind {
        NodeKind::Slider { on_change, .. } => on_change.clone(),
        _ => unreachable!(),
    };
    on_change(0.75); // what the Host's thumb-drag release fires
    drain();
    assert_eq!(v.peek(), 0.75, "author signal committed");
    assert!(
        matches!(node.borrow().kind, NodeKind::Slider { value, .. } if (value - 0.75).abs() < f32::EPSILON),
        "controlled value round-tripped onto the node"
    );
    app.stop();
}

/// The wrapped text-input `on_change` (`flushing1::<String>`) commits;
/// the controlled value re-enters through
/// `TextInputOps::update_text_input_value`.
#[test]
fn text_input_change_commits_and_updates_node_value() {
    let slot: Rc<Cell<Option<runtime_world::Signal<String>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app = boot(move || {
        let value = signal(String::new());
        s.set(Some(value));
        view()
            .child(
                text_input()
                    .value(value)
                    .placeholder("type here")
                    .on_change(move |t| value.set(t)),
            )
            .build()
    });
    let value = slot.get().expect("signal");
    let root = root_of(&app);
    let node =
        find_node(&root, &|k| matches!(k, NodeKind::TextInput { .. })).expect("text input");
    let on_change = match &node.borrow().kind {
        NodeKind::TextInput { on_change, .. } => on_change.clone(),
        _ => unreachable!(),
    };
    on_change("hello".to_string()); // what the keyboard router fires per edit
    drain();
    assert_eq!(value.peek(), "hello", "author signal committed");
    assert!(
        matches!(&node.borrow().kind, NodeKind::TextInput { value, .. } if value == "hello"),
        "controlled value round-tripped onto the node"
    );
    app.stop();
}

// ===========================================================================
// Structural reactivity — Dyn hole + keyed list against live nodes
// ===========================================================================

/// A closure child (structural Dyn hole) rebuilds on flush when its
/// reads change; the replaced subtree is a fresh node.
#[test]
fn dyn_hole_swaps_child_on_flush() {
    let slot: Rc<Cell<Option<runtime_world::Signal<bool>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app = boot(move || {
        let on = signal(false);
        s.set(Some(on));
        view()
            .child(move || {
                if on.get() {
                    view()
                        .child(text().content("toggle is ON"))
                        .into_scene_element()
                } else {
                    text().content("toggle is OFF").into_scene_element()
                }
            })
            .build()
    });
    let on = slot.get().expect("signal");
    let root = root_of(&app);
    assert_eq!(texts_of(&root), vec!["toggle is OFF".to_string()]);

    on.set(true);
    newcore::schedule_flush();
    drain();
    assert_eq!(texts_of(&root), vec!["toggle is ON".to_string()]);

    on.set(false);
    newcore::schedule_flush();
    drain();
    assert_eq!(texts_of(&root), vec!["toggle is OFF".to_string()]);
    app.stop();
}

/// Keyed lists on this backend take the ANCHORED driver: wgpu keeps
/// the trait-default `supports_child_splice() == false`, so every items
/// change is a full rebuild into the anchor — the same contract the OLD
/// walker's `each.rs` no-splice branch pinned for this backend
/// (`each_append.anchored.golden`). Content order tracks every edit;
/// node identity is NOT preserved (that requires splice support, which
/// is a backend capability decision out of adoption scope).
#[test]
fn keyed_list_anchored_rebuild_tracks_edits() {
    let slot: Rc<Cell<Option<runtime_world::Signal<Vec<u32>>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app = boot(move || {
        let rows = signal(vec![1u32, 2, 3]);
        s.set(Some(rows));
        view()
            .child(keyed(
                move || rows.get(),
                |n| *n,
                |n| text().content(format!("row #{n}")).build(),
            ))
            .build()
    });
    let rows = slot.get().expect("signal");
    let root = root_of(&app);

    let row_nodes = |root: &WgpuNode| -> Vec<(String, WgpuNode)> {
        let mut out = Vec::new();
        fn walk(node: &WgpuNode, out: &mut Vec<(String, WgpuNode)>) {
            if let NodeKind::Text { content } = &node.borrow().kind {
                if content.starts_with("row #") {
                    out.push((content.clone(), node.clone()));
                }
            }
            let children: Vec<WgpuNode> = node.borrow().children.clone();
            for c in &children {
                walk(c, out);
            }
        }
        walk(root, &mut out);
        out
    };

    let initial = row_nodes(&root);
    assert_eq!(
        initial.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
        vec!["row #1", "row #2", "row #3"]
    );

    // Reverse: content order flips; the anchored driver rebuilt every
    // row fresh (old scopes dropped first, anchor cleared, new nodes) —
    // no stale node may survive in the live tree.
    rows.update(|r| r.iter().rev().copied().collect());
    newcore::schedule_flush();
    drain();
    let reversed = row_nodes(&root);
    assert_eq!(
        reversed.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
        vec!["row #3", "row #2", "row #1"]
    );
    for (content, node) in &reversed {
        let original = initial.iter().find(|(t, _)| t == content).expect("same key set");
        assert!(
            !Rc::ptr_eq(node, &original.1),
            "{content}: anchored keyed rebuilds rows (old-core no-splice contract)"
        );
    }
    assert_eq!(
        reversed.len(),
        3,
        "exactly the three rebuilt rows are attached — old rows detached"
    );

    // Remove head, add tail.
    rows.update(|r| r.iter().copied().skip(1).collect());
    newcore::schedule_flush();
    drain();
    assert_eq!(
        row_nodes(&root).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
        vec!["row #2", "row #1"]
    );

    rows.update(|r| {
        let mut r = r.clone();
        r.push(4);
        r
    });
    newcore::schedule_flush();
    drain();
    assert_eq!(
        row_nodes(&root).iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
        vec!["row #2", "row #1", "row #4"]
    );
    app.stop();
}

/// Dropping the app (stop) detaches the keyed rows' subtree from the
/// live parent — teardown runs through Host::remove_child /
/// clear_children on real nodes without panicking, and the driver is
/// unhooked.
#[test]
fn stop_tears_down_without_panic_and_unhooks() {
    let app = boot(|| {
        view()
            .child(text().content("a"))
            .child(view().child(text().content("b")))
            .build()
    });
    let root = root_of(&app);
    assert_eq!(count_nodes(&root), 4, "root + text + inner view + inner text");
    app.stop();
    assert!(!newcore::is_booted());
    assert!(
        newcore::with_backend(|_| ()).is_none(),
        "backend handle cleared on stop"
    );
}

/// Regression (flat_list zero-rows class, GPU leg): virtualizer rows
/// realize on the new core. Two distinct pre-fix failure modes are
/// pinned at once:
///
/// 1. **Re-entrant borrow** — the GPU backend used to mount every row
///    EAGERLY inside `create_virtualizer` / `virtualizer_data_changed`,
///    both of which run under `backend.borrow_mut()` on the new core
///    while `mount_item` realizes rows through the same
///    `Rc<RefCell<WgpuBackend>>` → "RefCell already borrowed" abort
///    (the vocabulary contract forbids the synchronous fill; the fill
///    is now deferred via `schedule_virtualizer_fill`).
/// 2. **World entry** — the deferred fill invokes `mount_item` from a
///    scheduler microtask, OUTSIDE `World::enter`; row realization is
///    creation-side (row signals, Dyn text effects) and panics there
///    unless the caps wrapper enters the boot-stored world
///    (`enter_mounted_world` — the same gap every backend shared).
///
/// Also covers the data-changed path: growing the count re-fills with
/// the new rows, again via world-entered deferred callbacks.
#[test]
fn regression_virtualizer_rows_realize_world_entered() {
    let count = Rc::new(Cell::new(3usize));
    let count_slot: Rc<Cell<Option<runtime_world::Signal<u32>>>> = Rc::new(Cell::new(None));
    let app = {
        let count = count.clone();
        let count_slot = count_slot.clone();
        boot(move || {
            // A data signal the vocab handler's data effect subscribes
            // to, so bumping it re-runs `virtualizer_data_changed`.
            let rev = signal(0u32);
            count_slot.set(Some(rev));
            let count_for_items = count.clone();
            runtime_vocabulary::builders::virtualizer(
                move || {
                    let _ = rev.get();
                    count_for_items.get()
                },
                |i| i as u64,
                runtime_core::primitives::virtualizer::ItemSize::Known(Rc::new(|_| 20.0)),
                |i| {
                    // Creation-side row work — the aborting class: a
                    // row-local signal plus a Dyn text effect.
                    let n = signal(i as i32);
                    text().content(move || format!("row-{}", n.get())).build()
                },
            )
            .build()
        })
    };
    // The deferred fill + the row texts' batched microtasks.
    drain();
    let root = root_of(&app);
    let texts = texts_of(&root);
    for row in ["row-0", "row-1", "row-2"] {
        assert!(texts.iter().any(|t| t.contains(row)), "expected {row}, got {texts:?}");
    }

    // Data change: grow the list; the re-fill mounts the new row.
    count.set(4);
    count_slot.get().expect("build ran").update(|r| r + 1);
    app.world().flush();
    drain();
    let texts = texts_of(&root_of(&app));
    assert!(
        texts.iter().any(|t| t.contains("row-3")),
        "data-changed re-fill mounted the new row, got {texts:?}"
    );
    app.stop();
}

// ===========================================================================
// Embedded boot — `start_in_world` (the website wgpu-Simulator seam)
// ===========================================================================

/// `start_in_world` realizes into an EXTERNALLY-owned world and both
/// flush routes commit against it: the embedding host's own driver
/// (modeled by a direct `world.flush()` — on web that's backend-web's
/// dispatch hook) and this backend's `schedule_flush` (the
/// canvas-input route the wgpu caps wrappers use).
#[test]
fn start_in_world_realizes_into_the_host_world_and_shares_its_flush() {
    ensure_test_scheduler();
    let world = runtime_world::World::new();
    let slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app = newcore::start_in_world(
        make_backend(),
        |_| {},
        move || {
            let count = signal(0i32);
            s.set(Some(count));
            view()
                .child(text().content(move || format!("count = {}", count.get())))
                .build()
        },
        world.clone(),
    );
    let count = slot.get().expect("signal smuggled out of build");
    let root = root_of(&app);
    assert_eq!(texts_of(&root), vec!["count = 0".to_string()]);

    // Route 1: the embedding host's driver flushes the shared world.
    count.set(1);
    assert_eq!(texts_of(&root), vec!["count = 0".to_string()], "staged");
    world.flush();
    assert_eq!(texts_of(&root), vec!["count = 1".to_string()]);

    // Route 2: this backend's own schedule_flush (wgpu-dispatched
    // author callbacks) flushes the SAME world.
    count.set(2);
    newcore::schedule_flush();
    drain();
    assert_eq!(texts_of(&root), vec!["count = 2".to_string()]);

    // Embedded stop leaves the host-lifetime driver state in place
    // (documented on `stop`): the world belongs to the page.
    app.stop();
    assert!(
        newcore::is_booted(),
        "embedded stop must NOT clear FLUSH_WORLD — it points at the host's world"
    );
    // Hygiene for later tests in this process.
    newcore::flush_sync();
}

/// Regression (embedded lifetime): build-level state — a free effect's
/// cleanup and a glue `after_ms_scoped` timer — dies with the embedded
/// app's `stop()`, NOT with the (page-lifetime) world. Without the
/// `collect_owned` harvest in `start_in_world` both would be
/// world-root-owned and leak past the unmount; without the
/// scoped-scheduling anchor the timer would either never fire (old-core
/// anchoring is inert on the new core) or fire after teardown.
#[test]
fn start_in_world_stop_disposes_build_level_effects_and_scoped_timers() {
    ensure_test_scheduler();
    let world = runtime_world::World::new();
    let cleaned: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let fired: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let (cl, fi) = (cleaned.clone(), fired.clone());
    let app = newcore::start_in_world(
        make_backend(),
        |_| {},
        move || {
            // Build-level (non-component) effect with a cleanup.
            let cl2 = cl.clone();
            let _ = runtime_world::effect(move || {
                let cl3 = cl2.clone();
                move || cl3.set(true)
            });
            // Build-level scoped timer (the welcome app's `timeline!`
            // shape rides this exact path).
            let fi2 = fi.clone();
            runtime_vocabulary::glue::after_ms_scoped(0, move || fi2.set(true));
            view().child(text().content("embedded")).build()
        },
        world.clone(),
    );
    assert!(!cleaned.get());

    app.stop();
    assert!(
        cleaned.get(),
        "build-level effect cleanup fired on embedded stop (Owned harvest)"
    );
    // The queued timer body must be inert after teardown (the anchor's
    // dead flag — cancellation can't unqueue an already-dispatched
    // browser tick, so the flag is load-bearing).
    pump_timers();
    assert!(
        !fired.get(),
        "scoped timer registered at build must not fire after the embedded app stopped"
    );
    drop(world);
}

/// Positive control for the scoped-timer path: while the embedded app
/// is mounted, a build-level `after_ms_scoped` fires when the host
/// scheduler dispatches it.
#[test]
fn start_in_world_scoped_timer_fires_while_mounted() {
    ensure_test_scheduler();
    let world = runtime_world::World::new();
    let fired: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let fi = fired.clone();
    let app = newcore::start_in_world(
        make_backend(),
        |_| {},
        move || {
            let fi2 = fi.clone();
            runtime_vocabulary::glue::after_ms_scoped(0, move || fi2.set(true));
            view().child(text().content("embedded")).build()
        },
        world.clone(),
    );
    pump_timers();
    assert!(fired.get(), "scoped timer fires while the embedded app is live");
    app.stop();
    newcore::flush_sync();
}

/// Regression (the skin-toggle remount race): a REPLACEMENT embedded
/// app can mount before the old one drops. The old app's `stop()` must
/// not sever the replacement's flush driver (`FLUSH_WORLD`) or its
/// diagnostic backend handle.
#[test]
fn embedded_stop_keeps_replacement_flush_driver_alive() {
    ensure_test_scheduler();
    let world = runtime_world::World::new();

    let app_a = newcore::start_in_world(
        make_backend(),
        |_| {},
        || view().child(text().content("A")).build(),
        world.clone(),
    );

    // Replacement mounts BEFORE the old app drops.
    let slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
    let s = slot.clone();
    let app_b = newcore::start_in_world(
        make_backend(),
        |_| {},
        move || {
            let n = signal(0i32);
            s.set(Some(n));
            view()
                .child(text().content(move || format!("B = {}", n.get())))
                .build()
        },
        world.clone(),
    );
    let n = slot.get().expect("signal");
    let root_b = root_of(&app_b);

    app_a.stop();

    // The replacement's canvas-input flush route still works.
    n.set(7);
    newcore::schedule_flush();
    drain();
    assert_eq!(texts_of(&root_b), vec!["B = 7".to_string()]);

    // The diagnostic backend handle survived A's guarded clear and
    // points at B's backend.
    let b_ptr = newcore::with_backend(|rc| Rc::as_ptr(rc)).expect("backend handle live");
    app_b.with_realized(|_| {}); // keep app_b alive to here
    let _ = b_ptr;

    app_b.stop();
    newcore::flush_sync();
}
