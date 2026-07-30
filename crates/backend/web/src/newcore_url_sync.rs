//! Browser URL sync for NEW-core outlet navigators — the web
//! implementation of `runtime_vocabulary::handlers::nav_url_sync`.
//!
//! This is the port of the old substrate's
//! `runtime_shared::primitives::navigator::url_sync` onto the new-core
//! seam. The *semantics* are carried over invariant-for-invariant
//! (owned-slice comparison, pending-self-pop swallowing, reconciling
//! echo suppression, per-entry scroll memory, cold-start history seed);
//! what changed is the wiring:
//!
//! - The old module keyed off `NavigatorControl` (an old-core type whose
//!   registration surface is `pub(crate)`; runtime-core is frozen during
//!   the migration), so the logic is re-homed HERE against the
//!   vocabulary's [`UrlSyncService`] trait — installed by
//!   [`crate::newcore::start_in`], registered into by the swap/stack
//!   mount handlers.
//! - Old dispatch committed synchronously, so `RECONCILING` could be
//!   checked at commit time. New-core dispatch STAGES (driver commits on
//!   the next flush), so the reconciling state is captured at dispatch
//!   time as [`UrlSyncService::before_command`]'s suppress bit — the
//!   driver skips `after_commit` for reconciler echoes.
//! - The old register deferred its history seed a microtask (the old
//!   handler's initial mount was deferred); the new handlers mount
//!   synchronously, so the seed runs inline at registration.
//! - Deep-link boot needs nothing here: `install_scheduler` already
//!   seeds `runtime_shared`'s initial-path slot from
//!   `window.location.pathname` (url_provider.rs), and the new-core
//!   navigator handlers peek it during `resolve_initial`.
//! - Popstate dispatch only STAGES commands, so the listener calls
//!   [`crate::newcore::schedule_flush`] afterwards — popstate is a raw
//!   DOM event outside every wrapped author callback (the residual the
//!   newcore module docs called out).
//!
//! Scroll restore reads/writes the OUTLET node's scroll offset (the
//! registration's type-erased node, downcast to `web_sys::Node`). Only
//! meaningful when the outlet is itself a scroll surface — screens that
//! own their scroll via `scroll_view` are unaffected, same as legacy.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_shared::primitives::navigator::NavCommand;
use runtime_vocabulary::handlers::nav_url_sync::{
    CommittedKind, NavSyncKind, NavSyncRegistration, UrlSyncService,
};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

// ---------------------------------------------------------------------------
// Browser History surface (same calls url_provider.rs makes)
// ---------------------------------------------------------------------------
//
// Routed through a swappable port so the browser-history CONTRACT is
// testable: the op sequence (`push:/x`, `replace:/x`, `back`) is the
// observable, and a real `window.history` cannot report it — nor can a
// test let a root pop actually leave the page. The old core made the
// same surface injectable (`UrlProvider` +
// `runtime_shared::…::install_url_provider`), and its `SimHistory` fake
// is what the dying `mock-backend/tests/navigator_url_sync.rs` drove;
// [`HistoryPort`] is that seam re-homed on this module, so the fake and
// the nine invariants it pinned survive the old core's deletion.
//
// Production installs nothing: the `None` slot means the real calls
// below, unchanged (one thread-local read per history op, and history
// ops happen once per navigation).

/// The platform History API surface. `None` (production) ⇒ the real
/// `window.history` calls.
pub(crate) struct HistoryPort {
    pub(crate) current_path: Box<dyn Fn() -> String>,
    pub(crate) push_state: Box<dyn Fn(&str)>,
    pub(crate) replace_state: Box<dyn Fn(&str)>,
    pub(crate) history_back: Box<dyn Fn()>,
}

thread_local! {
    static HISTORY_PORT: RefCell<Option<HistoryPort>> = const { RefCell::new(None) };
}

/// Install a fake history surface (tests). Cleared by [`reset`].
///
/// Test-only: production leaves the slot `None` (see the module note
/// above), so this is gated rather than left to trip `dead_code`.
#[cfg(test)]
pub(crate) fn install_history_port(port: HistoryPort) {
    HISTORY_PORT.with(|p| *p.borrow_mut() = Some(port));
}

fn pathname() -> String {
    if let Some(p) = HISTORY_PORT.with(|p| p.borrow().as_ref().map(|p| (p.current_path)())) {
        return p;
    }
    web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_else(|| "/".to_string())
}

fn push_state(url: &str) {
    if HISTORY_PORT.with(|p| p.borrow().as_ref().map(|p| (p.push_state)(url))).is_some() {
        return;
    }
    if let Some(w) = web_sys::window() {
        if let Ok(h) = w.history() {
            let _ = h.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(url));
        }
    }
}

fn replace_state(url: &str) {
    if HISTORY_PORT.with(|p| p.borrow().as_ref().map(|p| (p.replace_state)(url))).is_some() {
        return;
    }
    if let Some(w) = web_sys::window() {
        if let Ok(h) = w.history() {
            let _ = h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(url));
        }
    }
}

fn history_back() {
    if HISTORY_PORT.with(|p| p.borrow().as_ref().map(|p| (p.history_back)())).is_some() {
        return;
    }
    if let Some(w) = web_sys::window() {
        if let Ok(h) = w.history() {
            let _ = h.back();
        }
    }
}

// ---------------------------------------------------------------------------
// Per-navigator entry (port of the old `NavEntry`)
// ---------------------------------------------------------------------------

/// One recorded back-history step: the owned URL of the screen a
/// forward navigation covered, plus the outlet scroll at that moment.
struct HistoryEntry {
    owned_url: String,
    scroll: (f32, f32),
}

struct NavEntry {
    id: u64,
    kind: NavSyncKind,
    base: String,
    resolve_entry: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>, String)>>,
    dispatch: Rc<dyn Fn(NavCommand)>,
    /// The outlet as a DOM element, when the registration's type-erased
    /// node downcast (foreign node types → `None`, scroll is a no-op).
    outlet: Option<web_sys::Element>,
    /// The slice of the platform URL this navigator currently owns.
    active_owned: RefCell<String>,
    history: RefCell<Vec<HistoryEntry>>,
}

impl NavEntry {
    /// The portion of `url` THIS navigator owns: the full URL minus the
    /// unconsumed remainder a nested navigator resolves. Ported from the
    /// legacy helpers (`NavigatorInstance::owned_of`).
    fn owned_of(&self, url: &str) -> String {
        match (self.resolve_entry)(url) {
            Some((_, _, remainder)) if !remainder.is_empty() => url
                .strip_suffix(&remainder)
                .unwrap_or(url)
                .trim_end_matches('/')
                .to_string(),
            _ => url.trim_end_matches('/').to_string(),
        }
    }

    fn current_scroll(&self) -> (f32, f32) {
        self.outlet
            .as_ref()
            .map(|el| (el.scroll_left() as f32, el.scroll_top() as f32))
            .unwrap_or((0.0, 0.0))
    }

    fn set_scroll(&self, x: f32, y: f32) {
        if let Some(el) = &self.outlet {
            el.set_scroll_left(x as i32);
            el.set_scroll_top(y as i32);
        }
    }
}

thread_local! {
    static REGISTRY: RefCell<Vec<Rc<NavEntry>>> = const { RefCell::new(Vec::new()) };
    /// `history.back()` calls we initiated whose `popstate` hasn't
    /// arrived yet — those events are bookkeeping-only, never dispatch.
    static PENDING_SELF_POPS: Cell<u32> = const { Cell::new(0) };
    /// True while the popstate reconciler dispatches commands — those
    /// dispatches must not write history for their own echoes
    /// (`before_command` returns the suppress bit instead).
    static RECONCILING: Cell<bool> = const { Cell::new(false) };
    static NEXT_ENTRY_ID: Cell<u64> = const { Cell::new(1) };
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
    /// Keeps the popstate listener alive for the page's lifetime.
    static POPSTATE_LISTENER: RefCell<Option<Closure<dyn FnMut(web_sys::Event)>>> =
        const { RefCell::new(None) };
}

fn entry_by_id(id: u64) -> Option<Rc<NavEntry>> {
    REGISTRY.with(|r| r.borrow().iter().find(|e| e.id == id).cloned())
}

// ---------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------

/// Install the new-core URL-sync service + popstate wiring. Idempotent;
/// called from `newcore::start_in` (before the app mounts, so every
/// navigator registers). Safe headless (no `window` ⇒ listener no-op;
/// history calls no-op).
pub(crate) fn install() {
    runtime_vocabulary::handlers::nav_url_sync::install_url_sync_service(Rc::new(WebUrlSync));
    if INSTALLED.with(|c| c.get()) {
        return;
    }
    let Some(window) = web_sys::window() else { return };
    INSTALLED.with(|c| c.set(true));
    let listener = Closure::wrap(Box::new(move |_e: web_sys::Event| {
        handle_popstate(&pathname());
    }) as Box<dyn FnMut(web_sys::Event)>);
    let _ =
        window.add_event_listener_with_callback("popstate", listener.as_ref().unchecked_ref());
    POPSTATE_LISTENER.with(|slot| *slot.borrow_mut() = Some(listener));
}

/// Uninstall the service + drop all sync entries (host `stop()` /
/// tests). The page-lifetime popstate listener stays; with no service
/// and an empty registry it is inert.
pub(crate) fn reset() {
    runtime_vocabulary::handlers::nav_url_sync::clear_url_sync_service();
    REGISTRY.with(|r| r.borrow_mut().clear());
    PENDING_SELF_POPS.with(|c| c.set(0));
    RECONCILING.with(|c| c.set(false));
    HISTORY_PORT.with(|p| *p.borrow_mut() = None);
}

// ---------------------------------------------------------------------------
// The service (port of register / before_command / after_command)
// ---------------------------------------------------------------------------

struct WebUrlSync;

impl UrlSyncService for WebUrlSync {
    fn register(&self, reg: NavSyncRegistration) -> Option<u64> {
        let id = NEXT_ENTRY_ID.with(|c| {
            let v = c.get();
            c.set(v + 1);
            v
        });
        let outlet = reg
            .outlet
            .downcast_ref::<web_sys::Node>()
            .and_then(|n| n.dyn_ref::<web_sys::Element>().cloned());
        let entry = Rc::new(NavEntry {
            id,
            kind: reg.kind,
            base: reg.base.clone(),
            resolve_entry: reg.resolve_entry,
            dispatch: reg.dispatch,
            outlet,
            active_owned: RefCell::new(String::new()),
            history: RefCell::new(Vec::new()),
        });
        *entry.active_owned.borrow_mut() = entry.owned_of(&reg.active_path);
        REGISTRY.with(|r| r.borrow_mut().push(entry.clone()));

        // Cold-start browser-history seed — ROOT navigator only (a
        // nested navigator must not stomp the entry its parent owns).
        // Synchronous: the new handlers seat their initial screen (and
        // any deep-link back-stack synthesis) before registering, so
        // `depth`/`active_path` are already committed. Old semantics,
        // minus the microtask (module docs).
        if reg.base.is_empty() {
            if reg.depth > 1 && reg.active_path != reg.initial_full_path {
                // Deep link with a synthesized entry below (a stack root
                // that reconstructed its index): make the browser back
                // button work immediately — index entry under the
                // deep-link entry. Capture the FULL platform path BEFORE
                // the replace (so a deeper nested remainder survives),
                // then re-push it above the index entry.
                let full = pathname();
                replace_state(&reg.initial_full_path);
                push_state(&full);
                entry.history.borrow_mut().push(HistoryEntry {
                    owned_url: entry.owned_of(&reg.initial_full_path),
                    scroll: (0.0, 0.0),
                });
            } else {
                // Plain mount: claim the current history entry (clears
                // stray hash/state) WITHOUT rewriting the path —
                // replacing with our own slice would clobber a nested
                // navigator's remainder on a cold deep link.
                replace_state(&pathname());
            }
            *entry.active_owned.borrow_mut() = entry.owned_of(&reg.active_path);
        }
        Some(id)
    }

    fn before_command(&self, id: u64, cmd: &NavCommand) -> bool {
        if RECONCILING.with(|c| c.get()) {
            // Reconciler echo: history is already correct (the browser
            // moved first); suppress the driver's after_commit too.
            return true;
        }
        let Some(entry) = entry_by_id(id) else { return false };
        match cmd {
            NavCommand::Push { url, .. } | NavCommand::Select { url, .. } => {
                // Re-selecting the already-active URL is a handler no-op
                // (the swap ignores it) — don't push a duplicate history
                // entry. Pushing the same URL onto a STACK is legitimate
                // depth growth, so Push is exempted.
                let owned = entry.owned_of(url);
                if matches!(cmd, NavCommand::Select { .. })
                    && owned == *entry.active_owned.borrow()
                {
                    return false;
                }
                let covered = entry.active_owned.borrow().clone();
                let scroll = entry.current_scroll();
                entry
                    .history
                    .borrow_mut()
                    .push(HistoryEntry { owned_url: covered, scroll });
                *entry.active_owned.borrow_mut() = owned;
                push_state(url);
            }
            NavCommand::Replace { url, .. } => {
                *entry.active_owned.borrow_mut() = entry.owned_of(url);
                replace_state(url);
            }
            NavCommand::Reset { url, .. } => {
                entry.history.borrow_mut().clear();
                *entry.active_owned.borrow_mut() = entry.owned_of(url);
                replace_state(url);
            }
            NavCommand::Pop => {
                // The handler commits the pop on the flush; we move the
                // browser back NOW and swallow the echoed popstate.
                // Guarded on our own recorded history so a root pop (a
                // handler no-op) never backs out of the app.
                if !entry.history.borrow().is_empty() {
                    PENDING_SELF_POPS.with(|c| c.set(c.get() + 1));
                    history_back();
                }
            }
            NavCommand::Custom(_) => {}
        }
        false
    }

    fn after_commit(&self, id: u64, kind: CommittedKind) {
        let Some(entry) = entry_by_id(id) else { return };
        match kind {
            CommittedKind::Forward => {
                // Fresh screen starts at the top (legacy `mount_internal`
                // behavior). No-op when the outlet isn't a scroll
                // surface.
                entry.set_scroll(0.0, 0.0);
            }
            CommittedKind::Pop => {
                // Reveal bookkeeping: the popped-to entry's URL becomes
                // the active owned slice and its scroll is restored.
                let revealed = entry.history.borrow_mut().pop();
                if let Some(h) = revealed {
                    *entry.active_owned.borrow_mut() = h.owned_url.clone();
                    entry.set_scroll(h.scroll.0, h.scroll.1);
                }
            }
            CommittedKind::Other => {}
        }
    }

    fn deregister(&self, id: u64) {
        // `try_with`: teardown can run during thread death (the old
        // module's TLS-destruction abort guard, kept).
        let _ = REGISTRY.try_with(|r| {
            r.borrow_mut().retain(|e| e.id != id);
        });
    }
}

// ---------------------------------------------------------------------------
// Popstate reconciliation (port of the old `handle_popstate`)
// ---------------------------------------------------------------------------

/// Translate a browser-initiated URL change into staged `NavCommand`s on
/// the navigator(s) whose owned slice changed. Public within the crate
/// for the wasm-bindgen browser tests, which drive it directly as well
/// as through real `history.back()` events.
pub(crate) fn handle_popstate(new_path: &str) {
    // Echo of our own `history.back()` — before_command already did the
    // bookkeeping (and after_commit restores scroll on the flush).
    if PENDING_SELF_POPS.with(|c| c.get()) > 0 {
        PENDING_SELF_POPS.with(|c| c.set(c.get() - 1));
        return;
    }

    let entries: Vec<Rc<NavEntry>> = REGISTRY.with(|r| r.borrow().iter().cloned().collect());
    let mut dispatched = false;

    for entry in entries {
        // Not under this navigator's routes at all → not ours.
        let Some((name, params, _remainder)) = (entry.resolve_entry)(new_path) else {
            continue;
        };
        let owned = entry.owned_of(new_path);
        // Our slice is unchanged → the change belongs to a NESTED
        // navigator; touching our screen would tear its subtree down
        // mid-transition (the legacy teardown race). Skip.
        if owned == *entry.active_owned.borrow() {
            continue;
        }

        let relative = owned
            .strip_prefix(entry.base.as_str())
            .unwrap_or(&owned)
            .to_string();

        // Backward: the new owned slice matches an entry in our recorded
        // history. A stack goes back by POPPING that many entries (the
        // browser collapses a multi-entry jump into one popstate); a
        // depth-less swap re-SELECTs the previous route.
        let match_idx = entry
            .history
            .borrow()
            .iter()
            .rposition(|h| h.owned_url == owned);
        if let Some(idx) = match_idx {
            RECONCILING.with(|c| c.set(true));
            match entry.kind {
                NavSyncKind::Swap => (entry.dispatch)(NavCommand::Select {
                    name,
                    url: relative,
                    params,
                    state: None,
                }),
                NavSyncKind::Stack => {
                    let pops = entry.history.borrow().len() - idx;
                    for _ in 0..pops {
                        (entry.dispatch)(NavCommand::Pop);
                    }
                }
            }
            RECONCILING.with(|c| c.set(false));
            dispatched = true;
            // Bookkeeping: drop the popped-over entries, restore the
            // scroll recorded for the entry we landed on.
            let landed = {
                let mut h = entry.history.borrow_mut();
                let landed = h.get(idx).map(|e| e.scroll);
                h.truncate(idx);
                landed
            };
            *entry.active_owned.borrow_mut() = owned;
            if let Some((x, y)) = landed {
                entry.set_scroll(x, y);
            }
        } else {
            // Forward (or unknown) navigation: the verb matches the
            // navigator kind (`Select` for swap, `Push` for stacks).
            let covered = entry.active_owned.borrow().clone();
            let scroll = entry.current_scroll();
            let cmd = match entry.kind {
                NavSyncKind::Swap => NavCommand::Select {
                    name,
                    url: relative,
                    params,
                    state: None,
                },
                NavSyncKind::Stack => NavCommand::Push {
                    name,
                    url: relative,
                    params,
                    state: None,
                },
            };
            RECONCILING.with(|c| c.set(true));
            (entry.dispatch)(cmd);
            RECONCILING.with(|c| c.set(false));
            dispatched = true;
            entry
                .history
                .borrow_mut()
                .push(HistoryEntry { owned_url: covered, scroll });
            *entry.active_owned.borrow_mut() = owned;
            entry.set_scroll(0.0, 0.0);
        }
    }

    // The staged commands commit on a flush; popstate fires outside
    // every wrapped author callback, so queue one explicitly (the
    // residual the newcore module docs reserved for this port).
    if dispatched {
        crate::newcore::schedule_flush();
    }
}

// ===========================================================================
// Browser-side regression tests. Run with:
//   cd crates/backend/web
//   wasm-pack test --headless --chrome -- --features new-core
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::newcore::{start, stop};
    use runtime_shared::Route;
    use runtime_vocabulary::builders::{navigator_outlet, stack_navigator};
    use runtime_vocabulary::prims::NavHandle;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    const ROOT: Route<()> = Route::<()>::new("root", "/");
    const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");

    fn setup_mount() -> web_sys::Element {
        let document = web_sys::window().unwrap().document().unwrap();
        if let Some(prior) = document.get_element_by_id("app") {
            prior.remove();
        }
        let el = document.create_element("div").unwrap();
        el.set_id("app");
        document.body().unwrap().append_child(&el).unwrap();
        // History isolation: tests share the page; pin the path so a
        // prior test's pushState never leaks into this one's asserts.
        replace_state("/");
        el
    }

    async fn sleep_ms(ms: i32) {
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            web_sys::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms)
                .unwrap();
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    // -----------------------------------------------------------------
    // Simulated browser history (port of the fake in the deleted
    // `mock-backend/tests/navigator_url_sync.rs`, which was the ONLY
    // place it existed). Drives this module through [`HistoryPort`], so
    // the op LOG is observable and a root pop can't navigate the test
    // page away. Same shape, same assertions.
    // -----------------------------------------------------------------

    /// A fake History API: entry list + index + an op log. `history_back`
    /// only moves the index and logs — the popstate the real browser
    /// would fire is delivered by the TEST calling [`handle_popstate`],
    /// mirroring the async delivery order.
    #[derive(Default)]
    struct SimHistory {
        entries: Vec<String>,
        index: usize,
        log: Vec<String>,
    }

    impl SimHistory {
        fn current(&self) -> String {
            self.entries
                .get(self.index)
                .cloned()
                .unwrap_or_else(|| "/".to_string())
        }
        fn pushes(&self) -> usize {
            self.log.iter().filter(|l| l.starts_with("push:")).count()
        }
        fn backs(&self) -> usize {
            self.log.iter().filter(|l| *l == "back").count()
        }
    }

    /// Install the fake seeded at `initial`; returns the shared history
    /// for assertions. Call BEFORE `start` so the registration's
    /// cold-start history claim lands in the fake.
    fn install_sim_history(initial: &str) -> Rc<RefCell<SimHistory>> {
        let sim = Rc::new(RefCell::new(SimHistory {
            entries: vec![initial.to_string()],
            index: 0,
            log: Vec::new(),
        }));
        let (s1, s2, s3, s4) = (sim.clone(), sim.clone(), sim.clone(), sim.clone());
        install_history_port(HistoryPort {
            current_path: Box::new(move || s1.borrow().current()),
            push_state: Box::new(move |url| {
                let mut h = s2.borrow_mut();
                let idx = h.index;
                h.entries.truncate(idx + 1);
                h.entries.push(url.to_string());
                h.index += 1;
                h.log.push(format!("push:{url}"));
            }),
            replace_state: Box::new(move |url| {
                let mut h = s3.borrow_mut();
                let idx = h.index;
                h.entries[idx] = url.to_string();
                h.log.push(format!("replace:{url}"));
            }),
            history_back: Box::new(move || {
                let mut h = s4.borrow_mut();
                if h.index > 0 {
                    h.index -= 1;
                }
                h.log.push("back".to_string());
            }),
        });
        sim
    }

    /// Fresh mount + fake history + cleared initial-path slot. The slot
    /// is per-thread and this binary shares one page, so a deep-link
    /// test would otherwise leak into its neighbours.
    fn setup_sim(initial: &str) -> (web_sys::Element, Rc<RefCell<SimHistory>>) {
        let mount = setup_mount();
        runtime_shared::primitives::navigator::set_initial_path(None);
        let sim = install_sim_history(initial);
        (mount, sim)
    }

    /// Simulate the user pressing browser Back: move the fake's index
    /// and deliver the popstate the browser would. The reconciler stages
    /// commands; `flush_sync` is the driver turn that commits them.
    fn browser_back(sim: &Rc<RefCell<SimHistory>>) {
        {
            let mut h = sim.borrow_mut();
            assert!(h.index > 0, "browser_back below the first entry");
            h.index -= 1;
        }
        let path = sim.borrow().current();
        handle_popstate(&path);
        crate::newcore::flush_sync();
    }

    /// Simulate browser Forward.
    fn browser_forward(sim: &Rc<RefCell<SimHistory>>) {
        {
            let mut h = sim.borrow_mut();
            assert!(
                h.index + 1 < h.entries.len(),
                "browser_forward past the last entry"
            );
            h.index += 1;
        }
        let path = sim.borrow().current();
        handle_popstate(&path);
        crate::newcore::flush_sync();
    }

    fn text_of(mount: &web_sys::Element) -> String {
        mount.text_content().unwrap_or_default()
    }

    /// A two-screen stack app; the captured `NavHandle` drives it.
    fn boot_stack_app(mount: &web_sys::Element) -> NavHandle {
        let _ = mount;
        let handle: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let handle_for_build = handle.clone();
        start(move || {
            stack_navigator(&ROOT)
                .screen(ROOT, |_| {
                    runtime_vocabulary::text().content("root-screen").build()
                })
                .screen(DETAIL, |_| {
                    runtime_vocabulary::text().content("detail-screen").build()
                })
                .layout(|| navigator_outlet().build())
                .on_handle(move |h| *handle_for_build.borrow_mut() = Some(h))
                .build()
        });
        let h = handle.borrow_mut().take().expect("NavHandle filled at mount");
        h
    }

    /// Push writes `pushState` at dispatch time and the driver commits
    /// the screen on the flush — `regression`: without the URL service
    /// the path would stay `/` (the unported seam this module closes).
    #[wasm_bindgen_test]
    async fn regression_push_writes_pushstate_and_commits() {
        let mount = setup_mount();
        let nav = boot_stack_app(&mount);
        sleep_ms(10).await;
        assert!(
            mount.text_content().unwrap().contains("root-screen"),
            "initial screen seated"
        );

        nav.push(&DETAIL, ());
        // before_command ran synchronously at dispatch: URL already moved.
        assert_eq!(pathname(), "/detail", "pushState at dispatch time");
        // A real event's dispatch-site glue queues the flush; a direct
        // NavHandle call from test code must flush explicitly.
        crate::newcore::flush_sync();
        assert!(
            mount.text_content().unwrap().contains("detail-screen"),
            "detail committed by the driver"
        );

        // Programmatic pop: history.back() + swallowed popstate echo.
        nav.pop();
        crate::newcore::flush_sync();
        sleep_ms(60).await; // history.back() → async popstate (swallowed)
        assert_eq!(pathname(), "/", "programmatic pop moved the browser back");
        assert!(
            mount.text_content().unwrap().contains("root-screen"),
            "pop revealed the root"
        );
        stop();
    }

    /// Browser-initiated back (`history.back()` with NO programmatic
    /// pop) reconciles into a staged `Pop` and the flush commits it.
    #[wasm_bindgen_test]
    async fn regression_popstate_navigates_back() {
        let mount = setup_mount();
        let nav = boot_stack_app(&mount);
        sleep_ms(10).await;
        nav.push(&DETAIL, ());
        crate::newcore::flush_sync();
        assert!(mount.text_content().unwrap().contains("detail-screen"));

        web_sys::window().unwrap().history().unwrap().back().unwrap();
        sleep_ms(80).await; // popstate → reconciler → staged Pop → flush
        assert_eq!(pathname(), "/", "browser back landed on the root URL");
        assert!(
            mount.text_content().unwrap().contains("root-screen"),
            "popstate reconciled into a Pop"
        );
        stop();
    }

    /// Cold-start deep link: the initial-path slot resolves the URL's
    /// screen at boot, and the registration seeds browser history with
    /// the index entry BELOW it so back works immediately. The slot is
    /// seeded manually here — in a real boot `install_scheduler`'s URL
    /// provider does it from `location.pathname`, but that install is
    /// page-once and this test binary shares one page.
    #[wasm_bindgen_test]
    async fn regression_deep_link_boot_resolves_and_seeds_history() {
        let mount = setup_mount();
        replace_state("/detail");
        runtime_shared::primitives::navigator::set_initial_path(Some("/detail".to_string()));
        let _nav = boot_stack_app(&mount);
        sleep_ms(10).await;
        assert!(
            mount.text_content().unwrap().contains("detail-screen"),
            "deep link mounted the URL's screen, not the configured initial"
        );
        assert_eq!(pathname(), "/detail", "URL untouched by the seed");

        // The seed placed the index entry under us: browser back reveals it.
        web_sys::window().unwrap().history().unwrap().back().unwrap();
        sleep_ms(80).await;
        assert_eq!(pathname(), "/", "back landed on the seeded index entry");
        assert!(
            mount.text_content().unwrap().contains("root-screen"),
            "back from a deep link reveals the synthesized index screen"
        );
        stop();
    }

    // =================================================================
    // Ported suite: the nine invariants the deleted
    // `mock-backend/tests/navigator_url_sync.rs` pinned on the old core,
    // re-expressed against THIS module through the `SimHistory` fake.
    // Each test names the invariant it carries over.
    // =================================================================

    const SETTINGS: Route<()> = Route::<()>::new("settings", "/settings");
    const DOCS: Route<()> = Route::<()>::new("docs", "/docs");
    const DEEP: Route<()> = Route::<()>::new("deep", "/deep");
    const NESTED_INDEX: Route<()> = Route::<()>::new("nindex", "");
    const NESTED_DETAIL: Route<()> = Route::<()>::new("ndetail", "/detail");

    /// Two-screen swap app (`/` + `/settings`) with a chrome bar, so the
    /// layout is a real author tree around the outlet.
    fn boot_swap_app() -> NavHandle {
        let handle: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let fill = handle.clone();
        start(move || {
            runtime_vocabulary::builders::swap_navigator(&ROOT)
                .screen(ROOT, |_| {
                    runtime_vocabulary::text().content("HOME CONTENT").build()
                })
                .screen(SETTINGS, |_| {
                    runtime_vocabulary::text().content("SETTINGS CONTENT").build()
                })
                .layout(|| {
                    runtime_vocabulary::view()
                        .child(navigator_outlet())
                        .child(runtime_vocabulary::text().content("BAR"))
                        .build()
                })
                .on_handle(move |h| *fill.borrow_mut() = Some(h))
                .build()
        });
        let h = handle.borrow_mut().take();
        h.expect("NavHandle filled at mount")
    }

    /// `Select` mirrors into pushState; browser Back reconciles into a
    /// `Select` of the previous route and writes NO history of its own
    /// (the staged-dispatch suppress bit — `before_command` returns
    /// `true` while RECONCILING, so nothing is pushed for the echo).
    #[wasm_bindgen_test]
    fn swap_select_pushes_url_and_browser_back_selects_previous() {
        let (mount, sim) = setup_sim("/");
        let nav = boot_swap_app();

        nav.select(&SETTINGS, ());
        crate::newcore::flush_sync();
        assert!(text_of(&mount).contains("SETTINGS CONTENT"));
        assert!(
            sim.borrow().log.contains(&"push:/settings".to_string()),
            "Select pushed the URL, log: {:?}",
            sim.borrow().log
        );
        assert_eq!(sim.borrow().current(), "/settings");

        let log_len_before = sim.borrow().log.len();
        browser_back(&sim);
        let t = text_of(&mount);
        assert!(t.contains("HOME CONTENT"), "back re-selects home: {t}");
        assert!(!t.contains("SETTINGS CONTENT"), "settings swapped out: {t}");
        assert_eq!(
            sim.borrow().log.len(),
            log_len_before,
            "reconciling a popstate must not write history again, log: {:?}",
            sim.borrow().log
        );
        stop();
    }

    /// Browser Forward after Back re-selects the forward route.
    #[wasm_bindgen_test]
    fn swap_browser_forward_reselects() {
        let (mount, sim) = setup_sim("/");
        let nav = boot_swap_app();

        nav.select(&SETTINGS, ());
        crate::newcore::flush_sync();
        browser_back(&sim);
        assert!(text_of(&mount).contains("HOME CONTENT"));

        browser_forward(&sim);
        let t = text_of(&mount);
        assert!(t.contains("SETTINGS CONTENT"), "forward re-selects settings: {t}");
        stop();
    }

    /// `Push` mirrors into pushState; browser Back pops the stack.
    #[wasm_bindgen_test]
    fn stack_push_pushes_url_and_browser_back_pops() {
        let (mount, sim) = setup_sim("/");
        let nav = boot_stack_app(&mount);

        nav.push(&DETAIL, ());
        crate::newcore::flush_sync();
        assert!(text_of(&mount).contains("detail-screen"));
        assert!(
            sim.borrow().log.contains(&"push:/detail".to_string()),
            "Push pushed the URL, log: {:?}",
            sim.borrow().log
        );

        browser_back(&sim);
        let t = text_of(&mount);
        assert!(t.contains("root-screen"), "back popped to root: {t}");
        assert!(!t.contains("detail-screen"), "detail released: {t}");
        stop();
    }

    /// Regression: a programmatic `pop()` moves the browser back exactly
    /// once and the echoed popstate must NOT pop again (pending-self-pop
    /// swallow — double-popping past the root was the failure mode); and
    /// a ROOT pop is a handler no-op that must never `history.back()`
    /// out of the app. The second half is only testable against a fake
    /// history: a real `back()` here would unload the test page.
    #[wasm_bindgen_test]
    fn regression_programmatic_pop_swallows_echo_and_root_pop_never_leaves_the_app() {
        let (mount, sim) = setup_sim("/");
        let nav = boot_stack_app(&mount);

        nav.push(&DETAIL, ());
        crate::newcore::flush_sync();

        nav.pop();
        crate::newcore::flush_sync();
        assert!(text_of(&mount).contains("root-screen"), "pop committed");
        assert_eq!(sim.borrow().backs(), 1, "one history.back(), log: {:?}", sim.borrow().log);

        // The browser delivers the popstate for OUR back — must be inert.
        let path = sim.borrow().current();
        handle_popstate(&path);
        crate::newcore::flush_sync();
        assert!(
            text_of(&mount).contains("root-screen"),
            "echo did not pop again: {}",
            text_of(&mount)
        );

        // A root pop is a handler no-op and must not move the browser.
        let backs_before = sim.borrow().backs();
        nav.pop();
        crate::newcore::flush_sync();
        assert_eq!(
            sim.borrow().backs(),
            backs_before,
            "root pop must not history.back() out of the app, log: {:?}",
            sim.borrow().log
        );
        stop();
    }

    /// The suppress bit's second half: a reconciled Pop must not ALSO
    /// run `after_commit`'s reveal bookkeeping — that would drop a
    /// second entry from the recorded history and the NEXT browser back
    /// would land on the wrong screen. Two forward pushes, two backs.
    #[wasm_bindgen_test]
    fn regression_reconciled_pop_bookkeeping_is_not_applied_twice() {
        let (mount, sim) = setup_sim("/");
        let handle: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let fill = handle.clone();
        start(move || {
            stack_navigator(&ROOT)
                .screen(ROOT, |_| runtime_vocabulary::text().content("S-root").build())
                .screen(DETAIL, |_| runtime_vocabulary::text().content("S-detail").build())
                .screen(DEEP, |_| runtime_vocabulary::text().content("S-deep").build())
                .layout(|| navigator_outlet().build())
                .on_handle(move |h| *fill.borrow_mut() = Some(h))
                .build()
        });
        let nav = { let h = handle.borrow_mut().take(); h.expect("handle") };

        nav.push(&DETAIL, ());
        crate::newcore::flush_sync();
        nav.push(&DEEP, ());
        crate::newcore::flush_sync();
        assert!(text_of(&mount).contains("S-deep"));
        assert_eq!(sim.borrow().pushes(), 2);

        browser_back(&sim);
        assert_eq!(sim.borrow().current(), "/detail");
        assert!(
            text_of(&mount).contains("S-detail"),
            "first back landed on /detail: {}",
            text_of(&mount)
        );

        browser_back(&sim);
        assert_eq!(sim.borrow().current(), "/");
        assert!(
            text_of(&mount).contains("S-root"),
            "second back landed on the root — the first reconcile popped \
             exactly one recorded entry: {}",
            text_of(&mount)
        );
        stop();
    }

    /// Regression (owned-slice guard): a popstate whose URL change lives
    /// in a NESTED navigator's slice must not remount the parent's
    /// screen — remounting would tear the nested navigator down
    /// mid-transition. Also pins nested base-path composition
    /// (`/docs` + `/detail`).
    #[wasm_bindgen_test]
    fn regression_nested_stack_popstate_does_not_remount_parent_swap_screen() {
        let (mount, sim) = setup_sim("/");
        let outer: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let inner: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let docs_builds = Rc::new(Cell::new(0u32));

        let (o, i, dbuilds) = (outer.clone(), inner.clone(), docs_builds.clone());
        start(move || {
            let i = i.clone();
            let dbuilds = dbuilds.clone();
            runtime_vocabulary::builders::swap_navigator(&ROOT)
                .screen(ROOT, |_| {
                    runtime_vocabulary::text().content("HOME CONTENT").build()
                })
                .screen(DOCS, move |_| {
                    dbuilds.set(dbuilds.get() + 1);
                    let i = i.clone();
                    stack_navigator(&NESTED_INDEX)
                        .screen(NESTED_INDEX, |_| {
                            runtime_vocabulary::text().content("DOCS INDEX").build()
                        })
                        .screen(NESTED_DETAIL, |_| {
                            runtime_vocabulary::text().content("NESTED DETAIL").build()
                        })
                        .layout(|| navigator_outlet().build())
                        .on_handle(move |h| *i.borrow_mut() = Some(h))
                        .build()
                })
                .layout(|| navigator_outlet().build())
                .on_handle(move |h| *o.borrow_mut() = Some(h))
                .build()
        });
        let onav = { let h = outer.borrow_mut().take(); h.expect("outer handle") };

        onav.select(&DOCS, ());
        crate::newcore::flush_sync();
        assert!(text_of(&mount).contains("DOCS INDEX"));
        assert_eq!(docs_builds.get(), 1);
        let inav = { let h = inner.borrow_mut().take(); h.expect("nested handle at screen build") };

        inav.push(&NESTED_DETAIL, ());
        crate::newcore::flush_sync();
        assert!(text_of(&mount).contains("NESTED DETAIL"));
        assert_eq!(
            sim.borrow().current(),
            "/docs/detail",
            "nested push composed its base, log: {:?}",
            sim.borrow().log
        );

        // Browser back to /docs: the NESTED stack pops; the parent swap's
        // owned slice (/docs) is unchanged and must not remount.
        browser_back(&sim);
        let t = text_of(&mount);
        assert!(t.contains("DOCS INDEX"), "nested stack popped to index: {t}");
        assert!(!t.contains("NESTED DETAIL"), "nested detail released: {t}");
        assert_eq!(
            docs_builds.get(),
            1,
            "parent swap screen must NOT rebuild when only the nested slice changed"
        );
        stop();
    }

    /// Scroll is snapshotted on push and restored on browser back;
    /// forward navigations land at the top. The outlet is styled as a
    /// real scroll box (bounded height + `overflow: hidden`, which is
    /// still programmatically scrollable) over tall screens, so this
    /// asserts against actual DOM `scrollTop`.
    #[wasm_bindgen_test]
    fn stack_scroll_snapshot_restores_on_browser_back() {
        use runtime_shared::{Length, StyleRules, Tokenized};

        fn tall(label: &'static str) -> runtime_scene::Element {
            runtime_vocabulary::view()
                .style(StyleRules {
                    height: Some(Tokenized::Literal(Length::Px(600.0))),
                    ..Default::default()
                })
                .child(runtime_vocabulary::text().content(label))
                .build()
        }

        let (mount, sim) = setup_sim("/");
        let handle: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let fill = handle.clone();
        start(move || {
            stack_navigator(&ROOT)
                .screen(ROOT, |_| tall("SCROLL-ROOT"))
                .screen(DETAIL, |_| tall("SCROLL-DETAIL"))
                .layout(|| {
                    navigator_outlet()
                        .style(StyleRules {
                            height: Some(Tokenized::Literal(Length::Px(40.0))),
                            overflow: Some(runtime_shared::Overflow::Hidden),
                            ..Default::default()
                        })
                        .build()
                })
                .on_handle(move |h| *fill.borrow_mut() = Some(h))
                .build()
        });
        let nav = { let h = handle.borrow_mut().take(); h.expect("handle") };
        assert!(text_of(&mount).contains("SCROLL-ROOT"));

        // The outlet is the element whose scroll the sync records.
        let outlet = outlet_element(&mount);
        assert!(
            outlet.scroll_height() > outlet.client_height(),
            "outlet must actually overflow for scroll to be observable \
             (scroll_height {} vs client_height {})",
            outlet.scroll_height(),
            outlet.client_height()
        );

        // The user scrolled the root screen before navigating away.
        outlet.set_scroll_top(120);
        assert_eq!(outlet.scroll_top(), 120, "browser accepted the scroll");

        nav.push(&DETAIL, ());
        crate::newcore::flush_sync();
        assert_eq!(
            outlet_element(&mount).scroll_top(),
            0,
            "a fresh screen starts at the top"
        );

        browser_back(&sim);
        assert_eq!(
            outlet_element(&mount).scroll_top(),
            120,
            "back restores the scroll the user left the root at"
        );
        stop();
    }

    /// The outlet element: the mount's nav root's first element child
    /// (the layout here is a bare `navigator_outlet`).
    fn outlet_element(mount: &web_sys::Element) -> web_sys::Element {
        mount
            .first_element_child()
            .expect("nav root")
            .first_element_child()
            .expect("outlet")
    }

    /// Regression: a cold start at a URL whose tail belongs to a NESTED
    /// navigator (`/docs/detail`) must mount the nested screen AND leave
    /// the full URL intact. The root's history claim originally replaced
    /// the entry with its OWN owned slice, clobbering the nested
    /// remainder — a cold `/alerts` load was rewritten to `/`.
    #[wasm_bindgen_test]
    fn regression_cold_start_nested_deep_link_preserves_full_url() {
        let (mount, sim) = setup_sim("/docs/detail");
        runtime_shared::primitives::navigator::set_initial_path(Some("/docs/detail".to_string()));

        start(move || {
            runtime_vocabulary::builders::swap_navigator(&ROOT)
                .screen(ROOT, |_| {
                    runtime_vocabulary::text().content("HOME CONTENT").build()
                })
                .screen(DOCS, move |_| {
                    stack_navigator(&NESTED_INDEX)
                        .screen(NESTED_INDEX, |_| {
                            runtime_vocabulary::text().content("DOCS INDEX").build()
                        })
                        .screen(NESTED_DETAIL, |_| {
                            runtime_vocabulary::text().content("NESTED DETAIL").build()
                        })
                        .layout(|| navigator_outlet().build())
                        .build()
                })
                .layout(|| navigator_outlet().build())
                .build()
        });

        let t = text_of(&mount);
        assert!(t.contains("NESTED DETAIL"), "cold deep link mounted the nested screen: {t}");
        assert_eq!(
            sim.borrow().current(),
            "/docs/detail",
            "root history-claim must not clobber the nested URL slice, log: {:?}",
            sim.borrow().log
        );
        runtime_shared::primitives::navigator::set_initial_path(None);
        stop();
    }

    /// Regression (docs-app catalog bug): selecting a DIFFERENT URL of
    /// the SAME parameterized route must swap the screen (the cache and
    /// no-op guard are URL-keyed, not name-keyed), and re-selecting the
    /// ACTIVE url must NOT push a duplicate history entry.
    #[wasm_bindgen_test]
    fn regression_swap_parameterized_route_selects_by_url_not_name() {
        use runtime_shared::primitives::navigator::RouteParams;
        use std::collections::HashMap as Map;

        #[derive(Clone)]
        struct EntryParams {
            name: String,
        }
        impl RouteParams for EntryParams {
            fn to_path(&self, pattern: &str) -> String {
                pattern.replace(":name", &self.name)
            }
            fn from_segments(segs: &Map<String, String>) -> Option<Self> {
                segs.get("name").map(|n| EntryParams { name: n.clone() })
            }
        }
        const ENTRY: Route<EntryParams> = Route::<EntryParams>::new("entry", "/entry/:name");

        let (mount, sim) = setup_sim("/");
        let handle: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
        let fill = handle.clone();
        start(move || {
            runtime_vocabulary::builders::swap_navigator(&ROOT)
                .screen(ROOT, |_| runtime_vocabulary::text().content("OVERVIEW").build())
                .screen(ENTRY, |p: EntryParams| {
                    runtime_vocabulary::text()
                        .content(format!("ENTRY {}", p.name))
                        .build()
                })
                .layout(|| navigator_outlet().build())
                .on_handle(move |h| *fill.borrow_mut() = Some(h))
                .build()
        });
        let nav = { let h = handle.borrow_mut().take(); h.expect("handle") };

        nav.select(&ENTRY, EntryParams { name: "button".into() });
        crate::newcore::flush_sync();
        assert!(text_of(&mount).contains("ENTRY button"));
        assert_eq!(sim.borrow().current(), "/entry/button");

        nav.select(&ENTRY, EntryParams { name: "card".into() });
        crate::newcore::flush_sync();
        let t = text_of(&mount);
        assert!(t.contains("ENTRY card"), "same-route select swapped: {t}");
        assert!(!t.contains("ENTRY button"), "stale entry swapped out: {t}");
        assert_eq!(sim.borrow().current(), "/entry/card");

        let pushes_before = sim.borrow().pushes();
        nav.select(&ENTRY, EntryParams { name: "card".into() });
        crate::newcore::flush_sync();
        assert_eq!(
            sim.borrow().pushes(),
            pushes_before,
            "re-selecting the active URL must not push history, log: {:?}",
            sim.borrow().log
        );

        browser_back(&sim);
        assert!(
            text_of(&mount).contains("ENTRY button"),
            "back re-selects the previous entry: {}",
            text_of(&mount)
        );
        stop();
    }

    /// Cold start at a deep-link URL mounts that screen and the root
    /// seed CLAIMS the history entry via replaceState (stray hash/state
    /// cleared) without rewriting the path.
    #[wasm_bindgen_test]
    fn cold_start_deep_link_mounts_url_screen_and_claims_the_entry() {
        let (mount, sim) = setup_sim("/settings");
        runtime_shared::primitives::navigator::set_initial_path(Some("/settings".to_string()));

        let _nav = boot_swap_app();

        let t = text_of(&mount);
        assert!(t.contains("SETTINGS CONTENT"), "deep link mounted the URL's screen: {t}");
        assert!(!t.contains("HOME CONTENT"), "home not mounted: {t}");
        assert!(
            sim.borrow().log.iter().any(|l| l == "replace:/settings"),
            "root seed claimed the history entry, log: {:?}",
            sim.borrow().log
        );
        runtime_shared::primitives::navigator::set_initial_path(None);
        stop();
    }
}
