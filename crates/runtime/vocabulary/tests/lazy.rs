//! Lazy chunk-boundary behavior (`handlers/lazy.rs`) — the new-core
//! port of the old walker's `Element::Lazy` driver, exercised against
//! the recording `host-mock` harness with its manually-pumped executor
//! (`host_mock::pump`) so deferred chunk loads are deterministic:
//! placeholder-first paint, body swap on load, SSR loading-UI freeze,
//! error → retry, teardown-mid-load abandonment, and chunk-scope
//! ownership of construction-time state (the `ScopedLoad` regression,
//! re-expressed at the mount site).

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use host_mock::pump::{install_executor, pending_tasks, pump_tasks};
use host_mock::Harness;
use runtime_scene::Element;
use runtime_vocabulary::glue::primitives::lazy::{lazy_split, LazyBodyThunk, LazyFuture};
use runtime_vocabulary::glue::IntoElement as _;

// ===========================================================================
// Harness
// ===========================================================================

/// The stock host-mock harness with the manually-pumped executor
/// installed and the SSR posture flag set (`renders_lazy_chunks() ==
/// false` is the server contract this suite gates on).
fn harness(renders_lazy_chunks: bool) -> Harness {
    install_executor();
    let h = Harness::new();
    h.shared.renders_lazy_chunks.set(renders_lazy_chunks);
    h
}

/// A loader whose future stays `Pending` until `ready` is set, then
/// resolves with the given outcome-producer (simulating the chunk
/// fetch landing on a later executor poll — the web shape).
fn gated_loader(
    ready: Rc<Cell<bool>>,
    outcome: impl Fn() -> Result<LazyBodyThunk, String> + 'static,
) -> impl Fn() -> LazyFuture + 'static {
    let outcome = Rc::new(outcome);
    move || {
        let ready = ready.clone();
        let outcome = outcome.clone();
        Box::pin(async move {
            GateFut { ready }.await;
            outcome()
        })
    }
}

struct GateFut {
    ready: Rc<Cell<bool>>,
}

impl Future for GateFut {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.ready.get() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

fn text_el(s: &'static str) -> Element {
    runtime_vocabulary::builders::text().content(s).build()
}

// ===========================================================================
// Tests
// ===========================================================================

/// The core lifecycle: placeholder paints at mount (no clear — fresh
/// container), the chunk body mounts when the load lands (old subtree
/// dropped, container cleared, body inserted), and `on_state` observes
/// Loading → Rendered.
#[test]
fn placeholder_renders_then_body_mounts_on_chunk_load() {
    let h = harness(true);
    let ready = Rc::new(Cell::new(false));
    let states: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let el = {
        let states = states.clone();
        lazy_split(gated_loader(ready.clone(), || {
            Ok(Box::new(|| text_el("chunk-body")) as LazyBodyThunk)
        }))
        .placeholder(|| text_el("loading"))
        .on_state(move |s| states.borrow_mut().push(format!("{s:?}")))
        .into_element()
    };
    let realized = h.mount(el);

    let ops = h.ops();
    assert_eq!(ops[0], "create n0 view", "container view first: {ops:?}");
    assert!(
        ops.iter().any(|o| o.contains("text \"loading\"")),
        "placeholder painted at mount: {ops:?}"
    );
    assert!(
        !ops.iter().any(|o| o.starts_with("clear")),
        "initial placeholder paint must not clear the fresh container: {ops:?}"
    );
    assert_eq!(states.borrow().as_slice(), ["Loading"], "sync Loading at mount");
    assert_eq!(pending_tasks(), 1, "one chunk load in flight");

    // Chunk still fetching: pump does nothing observable.
    pump_tasks();
    h.world.flush();
    assert!(!h.ops().iter().any(|o| o.contains("chunk-body")));

    // Chunk lands: the continuation mailboxes the thunk + bumps the
    // tick; the flush runs the swap effect (dispatch-hook discipline —
    // tests stand in for the platform's post-poll flush).
    ready.set(true);
    h.clear_ops();
    pump_tasks();
    h.world.flush();

    let ops = h.ops();
    assert!(
        ops.iter().any(|o| o == "clear_children n0"),
        "swap clears the container: {ops:?}"
    );
    assert!(
        ops.iter().any(|o| o.contains("text \"chunk-body\"")),
        "chunk body realized: {ops:?}"
    );
    let clear_at = ops.iter().position(|o| o == "clear_children n0").unwrap();
    let body_at = ops.iter().position(|o| o.contains("chunk-body")).unwrap();
    assert!(clear_at < body_at, "clear before the new subtree builds: {ops:?}");
    assert_eq!(states.borrow().as_slice(), ["Loading", "Rendered"]);
    assert_eq!(pending_tasks(), 0, "load task completed");

    drop(realized);
}

/// SSR gate (`renders_lazy_chunks() == false`): the untouched loading
/// UI is the final output — no load is ever spawned, mirroring the old
/// walker's server contract (the client hydrates the placeholder and
/// then loads the real chunk).
#[test]
fn ssr_keeps_loading_ui_and_never_spawns() {
    let h = harness(false);
    let states: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let loader_ran = Rc::new(Cell::new(false));

    let el = {
        let states = states.clone();
        let loader_ran = loader_ran.clone();
        lazy_split(move || -> LazyFuture {
            loader_ran.set(true);
            Box::pin(async { Ok(Box::new(|| text_el("chunk-body")) as LazyBodyThunk) })
        })
        .placeholder(|| text_el("loading"))
        .on_state(move |s| states.borrow_mut().push(format!("{s:?}")))
        .into_element()
    };
    let realized = h.mount(el);
    h.world.flush();
    pump_tasks();

    assert!(!loader_ran.get(), "SSR must never invoke the loader");
    assert_eq!(pending_tasks(), 0, "SSR must never spawn");
    let ops = h.ops();
    assert!(ops.iter().any(|o| o.contains("text \"loading\"")), "{ops:?}");
    assert!(!ops.iter().any(|o| o.contains("chunk-body")), "{ops:?}");
    assert_eq!(states.borrow().as_slice(), ["Loading"]);
    drop(realized);
}

/// Teardown mid-load: dropping the enclosing subtree before the chunk
/// resolves must abandon the load — the body thunk is never invoked,
/// nothing touches the (detached) container. On the new core this falls
/// out of ownership (the swap effect died with the subtree; the late
/// tick bump notifies nobody) rather than an explicit cancelled flag.
#[test]
fn regression_teardown_mid_load_abandons_chunk() {
    let h = harness(true);
    let ready = Rc::new(Cell::new(false));
    let body_built = Rc::new(Cell::new(false));

    let el = {
        let body_built = body_built.clone();
        lazy_split(gated_loader(ready.clone(), move || {
            let body_built = body_built.clone();
            Ok(Box::new(move || {
                body_built.set(true);
                text_el("chunk-body")
            }) as LazyBodyThunk)
        }))
        .placeholder(|| text_el("loading"))
        .into_element()
    };
    let realized = h.mount(el);
    assert_eq!(pending_tasks(), 1);

    // Unmount while the fetch is in flight.
    drop(realized);
    h.clear_ops();

    // The chunk lands late.
    ready.set(true);
    pump_tasks();
    h.world.flush();

    assert!(!body_built.get(), "late chunk must not construct its body");
    let ops = h.ops();
    assert!(ops.is_empty(), "late chunk must not touch the tree: {ops:?}");
}

/// Load failure paints the error UI (with a live retry handle);
/// retrying re-mounts the loading UI and re-runs the loader; a
/// successful second attempt mounts the body. Ports the old walker's
/// weak-retry-holder contract.
#[test]
fn error_ui_mounts_and_retry_reloads() {
    let h = harness(true);
    let ready = Rc::new(Cell::new(false));
    let attempts = Rc::new(Cell::new(0u32));
    let retry_slot: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let states: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let el = {
        let attempts = attempts.clone();
        let retry_slot = retry_slot.clone();
        let states = states.clone();
        lazy_split(gated_loader(ready.clone(), move || {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                Err("fetch failed".to_string())
            } else {
                Ok(Box::new(|| text_el("chunk-body")) as LazyBodyThunk)
            }
        }))
        .placeholder(|| text_el("loading"))
        .on_error(move |e| {
            assert_eq!(e.message(), "fetch failed");
            *retry_slot.borrow_mut() = Some(e.retry());
            text_el("error-ui")
        })
        .on_state(move |s| states.borrow_mut().push(format!("{s:?}")))
        .into_element()
    };
    let realized = h.mount(el);

    // First attempt fails → error UI replaces the placeholder.
    ready.set(true);
    pump_tasks();
    h.world.flush();
    let ops = h.ops();
    assert!(ops.iter().any(|o| o.contains("text \"error-ui\"")), "{ops:?}");
    assert!(
        states.borrow().iter().any(|s| s.contains("Error")),
        "{:?}",
        states.borrow()
    );

    // Retry (as the author's error-UI button would): loading UI
    // re-mounts, the loader runs again, the body lands.
    let retry = retry_slot.borrow().clone().expect("error UI received a retry handle");
    h.clear_ops();
    h.world.enter(|| retry());
    h.world.flush();
    let ops = h.ops();
    assert!(
        ops.iter().any(|o| o.contains("text \"loading\"")),
        "retry resets to the loading UI first: {ops:?}"
    );
    pump_tasks();
    h.world.flush();
    let ops = h.ops();
    assert!(ops.iter().any(|o| o.contains("text \"chunk-body\"")), "{ops:?}");
    assert_eq!(attempts.get(), 2);
    assert!(
        states.borrow().last().unwrap().contains("Rendered"),
        "{:?}",
        states.borrow()
    );
    drop(realized);
}

/// The `ScopedLoad` regression, at its new-core seam: reactive state the
/// chunk creates eagerly during CONSTRUCTION (an `Element::External`
/// extension allocating signals in its constructor — the whiteboard
/// `CoreCanvas::new()` shape) must be owned by the chunk subtree and
/// freed at teardown. The mount handler invokes the body thunk inside
/// `component_scope`, so construction-time signals fold into the
/// swapped-in `Realized`; unmounting must drop them.
#[test]
fn regression_chunk_construction_state_owned_by_subtree() {
    struct DropFlag(#[allow(dead_code)] Rc<Cell<bool>>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    impl PartialEq for DropFlag {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    let h = harness(true);
    let ready = Rc::new(Cell::new(false));
    let dropped = Rc::new(Cell::new(false));

    let el = {
        let dropped = dropped.clone();
        lazy_split(gated_loader(ready.clone(), move || {
            let dropped = dropped.clone();
            Ok(Box::new(move || {
                // Construction-time signal, created while the thunk runs
                // (inside the handler's component_scope).
                let _sig = runtime_world::signal(Rc::new(DropFlag(dropped.clone())));
                text_el("chunk-body")
            }) as LazyBodyThunk)
        }))
        .into_element()
    };
    let realized = h.mount(el);

    ready.set(true);
    pump_tasks();
    h.world.flush();
    assert!(
        h.ops().iter().any(|o| o.contains("chunk-body")),
        "body mounted: {:?}",
        h.ops()
    );
    assert!(!dropped.get(), "construction-time signal lives while the chunk is mounted");

    drop(realized);
    assert!(
        dropped.get(),
        "unmount must free construction-time signals (chunk-scope ownership)"
    );
}

// ===========================================================================
// Off-web fast path
// ===========================================================================

/// A loader whose future is ALREADY COMPLETE at first poll — the shape
/// `wasm_split` emits off wasm32, where the chunk body is a plain fn and
/// there is nothing to fetch.
fn ready_loader(
    outcome: impl Fn() -> Result<LazyBodyThunk, String> + 'static,
) -> impl Fn() -> LazyFuture + 'static {
    let outcome = Rc::new(outcome);
    move || {
        let outcome = outcome.clone();
        Box::pin(async move { outcome() })
    }
}

/// Regression: off web, `lazy` must be a NO-OP — the body mounts
/// synchronously at realize, with no loading placeholder ever painted.
///
/// ## What this pins
///
/// `wasm_split` emits the chunk body as a plain fn off wasm32
/// (`#[cfg(not(target_arch = "wasm32"))] #default_item`), so there is no
/// chunk to fetch and the loader's future is complete on first poll. The
/// handler nevertheless ran the full streaming dance for it: paint the
/// placeholder, `spawn_async`, mailbox the outcome, bump a tick, then on
/// the next flush tear the placeholder subtree down and realize the body.
///
/// Three costs, all user-visible on desktop:
///
/// 1. a flash of the loading UI on every screen mount;
/// 2. a double realize (placeholder subtree, then body subtree) plus a
///    `clear_children` between them — churning anything the placeholder
///    held;
/// 3. a dependency on a per-executor-poll flush hook, which only the web
///    and Apple backends install. On GTK the swap rode `pollster`'s
///    inline completion — it happened to work, but nothing on that host
///    flushes on a timer, so it was one scheduling change away from
///    screens that never leave "loading".
///
/// The assertions are deliberately about the ABSENCE of the streaming
/// ops (no placeholder paint, no clear, no spawned task), not just the
/// presence of the body: a body that arrives via the slow path would
/// still satisfy a naive "is the body there?" check.
#[test]
fn regression_ready_loader_mounts_body_synchronously_without_placeholder() {
    let h = harness(true);
    let states: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let el = {
        let states = states.clone();
        lazy_split(ready_loader(|| {
            Ok(Box::new(|| text_el("chunk-body")) as LazyBodyThunk)
        }))
        .placeholder(|| text_el("loading"))
        .on_state(move |s| states.borrow_mut().push(format!("{s:?}")))
        .into_element()
    };
    let realized = h.mount(el);

    let ops = h.ops();
    assert_eq!(ops[0], "create n0 view", "container view first: {ops:?}");
    assert!(
        ops.iter().any(|o| o.contains("text \"chunk-body\"")),
        "body must realize synchronously at mount: {ops:?}"
    );
    assert!(
        !ops.iter().any(|o| o.contains("text \"loading\"")),
        "the loading placeholder must NEVER be painted when the body is \
         already available — that flash is the whole bug: {ops:?}"
    );
    assert!(
        !ops.iter().any(|o| o.starts_with("clear")),
        "no placeholder subtree to tear down, so nothing may be cleared: {ops:?}"
    );
    assert_eq!(
        pending_tasks(),
        0,
        "no executor task may be spawned for a body that is already resolved"
    );
    assert_eq!(
        states.borrow().as_slice(),
        ["Loading", "Rendered"],
        "state contract is unchanged — authors still observe Loading then \
         Rendered, they just never see a rendered placeholder"
    );

    drop(realized);
}

/// The fast path must not fire when the loader really is async: a
/// `Pending` first poll keeps the streaming behaviour intact. Guards
/// against "optimise off `cfg(not(wasm32))`" — the handler polls to find
/// out rather than assuming, so a native host with a genuinely deferred
/// loader still gets placeholder-then-swap.
#[test]
fn regression_pending_loader_still_streams_on_native() {
    let h = harness(true);
    let ready = Rc::new(Cell::new(false));

    let el = lazy_split(gated_loader(ready.clone(), || {
        Ok(Box::new(|| text_el("chunk-body")) as LazyBodyThunk)
    }))
    .placeholder(|| text_el("loading"))
    .into_element();
    let realized = h.mount(el);

    let ops = h.ops();
    assert!(
        ops.iter().any(|o| o.contains("text \"loading\"")),
        "a Pending loader must still paint the placeholder: {ops:?}"
    );
    assert_eq!(pending_tasks(), 1, "and must still spawn the load");

    ready.set(true);
    h.clear_ops();
    pump_tasks();
    h.world.flush();
    assert!(
        h.ops().iter().any(|o| o.contains("chunk-body")),
        "body still swaps in on completion: {:?}",
        h.ops()
    );

    drop(realized);
}
