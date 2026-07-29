//! Platform-URL synchronization for outlet-model navigators.
//!
//! One substrate-level implementation of what the legacy web navigator
//! helpers did per-SDK: mirror navigation into the browser URL
//! (`pushState`/`replaceState`), reconcile browser back/forward
//! (`popstate`) into ordinary [`NavCommand`]s, remember per-entry scroll
//! offsets so back restores them, and seed cold-start deep links. The
//! outlet-model handlers (`swap-navigator`, `stack-navigator`) are
//! backend-neutral and never touch the URL themselves — they opt in via
//! [`NavigatorControl::enable_url_sync`] and everything else happens
//! here, keyed off the commands already flowing through
//! [`NavigatorControl::dispatch`].
//!
//! # Provider model
//!
//! `runtime-core` cannot depend on `web-sys`, so the actual History API
//! is an installed provider ([`install_url_provider`]) — the same
//! pattern as `install_scheduler` / the wheel + file-drop channels. The
//! web backend installs one at startup; native/SSR backends install
//! nothing and every entry point below is a no-op, which is exactly the
//! legacy behavior (only web ever touched a URL).
//!
//! # Opt-in, not automatic
//!
//! While the legacy class-based navigators (stack/tab/drawer) coexist
//! with the outlet model, both generations dispatch through the same
//! `NavigatorControl`. The legacy web handlers already do their own
//! `pushState`/popstate work, so hooking every dispatch would
//! double-write history. `enable_url_sync()` is therefore called only
//! by handlers that delegate URL work to the substrate; controls that
//! never call it are invisible to this module.
//!
//! # Semantics (ported from `web-navigator-helpers`)
//!
//! - `Push`/`Select` with a URL → `pushState` + one history entry
//!   recording the URL being covered and its scroll offset. (`Select`
//!   pushes real history entries — on web the browser back button is
//!   expected to undo a tab/sidebar selection, exactly as the legacy
//!   drawer did.)
//! - `Replace` → `replaceState`, history untouched. `Reset` →
//!   `replaceState` + history cleared.
//! - Programmatic `Pop` → the handler commits the pop synchronously;
//!   this module calls `history.back()` and swallows the resulting
//!   `popstate` (a pending-self-pop counter — `pushState` never fires
//!   `popstate`, but `history.back()` does).
//! - Browser-initiated `popstate` → each enabled navigator compares the
//!   new path's slice it OWNS (`owned_of`, the full path minus any
//!   nested navigator's remainder) with its active owned slice.
//!   Unchanged ⇒ the change belongs to a nested navigator: skip —
//!   remounting here would tear the nested navigator down mid-pop (the
//!   teardown race the legacy helpers document). A match deeper in the
//!   recorded history ⇒ dispatch that many `Pop`s; anything else ⇒ a
//!   forward navigation, dispatched through the navigator's link
//!   activator (`Select` for swap, `Push` for stacks).
//! - Scroll: forward navigations reset the outlet to the top; back
//!   restores the recorded offset. Only meaningful when the outlet is
//!   itself a scroll surface — screens that own their scroll via
//!   `scroll_view` are unaffected, same as legacy.

use super::shared::{NavCommand, NavState, NavigatorControl};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// The platform History API surface, installed by the web backend.
pub struct UrlProvider {
    /// The platform's current path (`window.location.pathname`).
    pub current_path: Box<dyn Fn() -> String>,
    /// `history.pushState(null, "", url)`.
    pub push_state: Box<dyn Fn(&str)>,
    /// `history.replaceState(null, "", url)`.
    pub replace_state: Box<dyn Fn(&str)>,
    /// `history.back()`.
    pub history_back: Box<dyn Fn()>,
}

thread_local! {
    static PROVIDER: RefCell<Option<UrlProvider>> = const { RefCell::new(None) };
    /// Live per-navigator sync entries (weak controls; pruned lazily).
    static REGISTRY: RefCell<Vec<Rc<NavEntry>>> = const { RefCell::new(Vec::new()) };
    /// `history.back()` calls we initiated whose `popstate` hasn't
    /// arrived yet — those events are bookkeeping-only, never dispatch.
    static PENDING_SELF_POPS: Cell<u32> = const { Cell::new(0) };
    /// True while the popstate reconciler dispatches commands — the
    /// dispatch hooks must not write history for their own echoes.
    static RECONCILING: Cell<bool> = const { Cell::new(false) };
    static NEXT_ENTRY_ID: Cell<u64> = const { Cell::new(1) };
}

/// Install the platform URL provider. Called once by the web backend at
/// startup (never on native/SSR). The provided `on_popstate`
/// registration is the caller's job: wire the platform `popstate` event
/// to [`handle_popstate`].
pub fn install_url_provider(provider: UrlProvider) {
    PROVIDER.with(|p| *p.borrow_mut() = Some(provider));
}

/// True when a provider is installed (i.e. running on a URL-bearing
/// platform). Exposed for tests.
pub fn url_provider_installed() -> bool {
    PROVIDER.with(|p| p.borrow().is_some())
}

/// Remove the installed provider and all sync state. Test hook — lets a
/// test install a fake provider without leaking into the next test on
/// the same thread.
pub fn reset_url_sync_for_tests() {
    PROVIDER.with(|p| *p.borrow_mut() = None);
    REGISTRY.with(|r| r.borrow_mut().clear());
    PENDING_SELF_POPS.with(|c| c.set(0));
    RECONCILING.with(|c| c.set(false));
}

fn with_provider<R>(f: impl FnOnce(&UrlProvider) -> R) -> Option<R> {
    PROVIDER.with(|p| p.borrow().as_ref().map(f))
}

// ---------------------------------------------------------------------------
// Per-navigator entry
// ---------------------------------------------------------------------------

/// One recorded back-history step: the owned URL of the screen a
/// forward navigation covered, plus the outlet scroll at that moment.
struct HistoryEntry {
    /// Full hierarchical path (this navigator's owned slice).
    owned_url: String,
    scroll: (f32, f32),
}

/// The URL-sync context the walker parks on every `NavigatorControl`
/// (kind-agnostically); it becomes a live registry entry only when a
/// handler opts in via `enable_url_sync`.
#[doc(hidden)]
pub struct UrlSyncContext {
    /// Hierarchical PREFIX resolver — same closure the `NavigatorHost`
    /// carries. Returns `(route, params, remainder)` for a full path.
    pub resolve_entry: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>, String)>>,
    /// Reactive nav-state mirror (depth read untracked for the initial
    /// browser-history seed; active_path for the same).
    pub nav_state: NavState,
    /// This navigator's base prefix ("" for the root).
    pub base: String,
    /// Full hierarchical path of the configured initial screen.
    pub initial_full_path: String,
}

struct NavEntry {
    id: u64,
    control: Weak<NavigatorControl>,
    ctx: UrlSyncContext,
    /// The slice of the platform URL this navigator currently owns.
    active_owned: RefCell<String>,
    history: RefCell<Vec<HistoryEntry>>,
    /// Outlet scroll accessors, installed by the handler once its
    /// outlet exists (`NavigatorControl::install_scroll_accessor`).
    scroll_get: RefCell<Option<Rc<dyn Fn() -> (f32, f32)>>>,
    scroll_set: RefCell<Option<Rc<dyn Fn(f32, f32)>>>,
}

impl NavEntry {
    /// The portion of `url` THIS navigator owns: the full URL minus the
    /// unconsumed remainder a nested navigator resolves. Ported from
    /// the legacy helpers (`NavigatorInstance::owned_of`).
    fn owned_of(&self, url: &str) -> String {
        match (self.ctx.resolve_entry)(url) {
            Some((_, _, remainder)) if !remainder.is_empty() => url
                .strip_suffix(&remainder)
                .unwrap_or(url)
                .trim_end_matches('/')
                .to_string(),
            _ => url.trim_end_matches('/').to_string(),
        }
    }

    fn current_scroll(&self) -> (f32, f32) {
        self.scroll_get
            .borrow()
            .as_ref()
            .map(|f| f())
            .unwrap_or((0.0, 0.0))
    }

    fn set_scroll(&self, x: f32, y: f32) {
        if let Some(f) = self.scroll_set.borrow().as_ref() {
            f(x, y);
        }
    }
}

fn entry_by_id(id: u64) -> Option<Rc<NavEntry>> {
    REGISTRY.with(|r| r.borrow().iter().find(|e| e.id == id).cloned())
}

// ---------------------------------------------------------------------------
// Registration (called from NavigatorControl)
// ---------------------------------------------------------------------------

/// Activate URL sync for one navigator. Called by
/// `NavigatorControl::enable_url_sync` when a handler opts in; no-op
/// without an installed provider. Returns the registry id the control
/// stores so dispatch hooks can find the entry.
pub(crate) fn register(control: &Rc<NavigatorControl>, ctx: UrlSyncContext) -> Option<u64> {
    if !url_provider_installed() {
        return None;
    }
    let id = NEXT_ENTRY_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    // Resolve the active owned slice from the platform URL when this
    // navigator's routes match it (cold-start deep link already mounted
    // by the walker's initial-path resolution), else from the
    // configured initial path.
    let entry = Rc::new(NavEntry {
        id,
        control: Rc::downgrade(control),
        active_owned: RefCell::new(String::new()),
        history: RefCell::new(Vec::new()),
        scroll_get: RefCell::new(None),
        scroll_set: RefCell::new(None),
        ctx,
    });
    let active_path = crate::reactive::untrack(|| entry.ctx.nav_state.active_path.get());
    *entry.active_owned.borrow_mut() = entry.owned_of(&active_path);

    // Prune dead controls while we're here.
    REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        reg.retain(|e| e.control.strong_count() > 0);
        reg.push(entry.clone());
    });

    // Seed the browser history for this navigator's cold start — but
    // only for the ROOT navigator (a nested navigator must not stomp
    // the entry its parent owns). Deferred a microtask so the handler's
    // deferred initial mount (and any deep-link back-stack synthesis,
    // which raises `depth`) has committed first.
    if entry.ctx.base.is_empty() {
        let entry = entry.clone();
        crate::schedule_microtask(move || {
            let depth = crate::reactive::untrack(|| entry.ctx.nav_state.depth.get());
            let active = crate::reactive::untrack(|| entry.ctx.nav_state.active_path.get());
            let _ = with_provider(|p| {
                if depth > 1 && active != entry.ctx.initial_full_path {
                    // Deep link with a synthesized entry below (a stack
                    // root that reconstructed its index): make the browser
                    // back button work immediately — index entry under the
                    // deep-link entry, mirroring the legacy cold-start
                    // flow. Capture the FULL platform path BEFORE the
                    // replace (so a deeper nested remainder survives),
                    // then re-push it above the index entry.
                    let full = (p.current_path)();
                    (p.replace_state)(&entry.ctx.initial_full_path);
                    (p.push_state)(&full);
                    entry.history.borrow_mut().push(HistoryEntry {
                        owned_url: entry.owned_of(&entry.ctx.initial_full_path),
                        scroll: (0.0, 0.0),
                    });
                } else {
                    // Plain mount: claim the current history entry
                    // (clears stray hash/state) WITHOUT rewriting the
                    // path — replacing with our own slice would clobber
                    // a nested navigator's remainder on a cold deep link
                    // (e.g. `/alerts` under a root whose slice is `/`).
                    (p.replace_state)(&(p.current_path)());
                }
            });
            *entry.active_owned.borrow_mut() =
                entry.owned_of(&crate::reactive::untrack(|| entry.ctx.nav_state.active_path.get()));
        });
    }

    Some(id)
}

/// Remove a navigator's sync entry. Called from `NavigatorControl`'s
/// `Drop` at navigator teardown, so entries (which own author-reachable
/// `Rc`s: the resolver, scroll accessors over handler state) never
/// outlive their navigator into thread-death TLS destruction — dropping
/// them there runs author/scope destructors against an already-destroyed
/// reactive arena ("cannot access a TLS value during destruction"
/// abort). `try_with` keeps the drop safe even if the registry's own
/// TLS slot is already gone.
pub(crate) fn deregister(id: u64) {
    let _ = REGISTRY.try_with(|r| {
        r.borrow_mut().retain(|e| e.id != id);
    });
}

/// Install the outlet scroll accessors for an enabled navigator.
pub(crate) fn install_scroll_accessor(
    id: u64,
    get: Rc<dyn Fn() -> (f32, f32)>,
    set: Rc<dyn Fn(f32, f32)>,
) {
    if let Some(e) = entry_by_id(id) {
        *e.scroll_get.borrow_mut() = Some(get);
        *e.scroll_set.borrow_mut() = Some(set);
    }
}

// ---------------------------------------------------------------------------
// Dispatch hooks (called from NavigatorControl::dispatch)
// ---------------------------------------------------------------------------

/// Runs BEFORE the handler commits `cmd` (URLs already base-composed).
/// Writes browser history and snapshots the outgoing screen's scroll —
/// the outlet still shows it here.
pub(crate) fn before_command(id: u64, cmd: &NavCommand) {
    if RECONCILING.with(|c| c.get()) {
        return;
    }
    let Some(entry) = entry_by_id(id) else { return };
    match cmd {
        NavCommand::Push { url, .. } | NavCommand::Select { url, .. } => {
            // Re-selecting the already-active URL is a handler no-op
            // (the swap ignores it) — don't push a duplicate history
            // entry for it. Applies to Select's tab-reclick shape;
            // pushing the same URL onto a STACK is legitimate depth
            // growth, so Push is exempted.
            let owned = entry.owned_of(url);
            if matches!(cmd, NavCommand::Select { .. })
                && owned == *entry.active_owned.borrow()
            {
                return;
            }
            let covered = entry.active_owned.borrow().clone();
            let scroll = entry.current_scroll();
            entry
                .history
                .borrow_mut()
                .push(HistoryEntry { owned_url: covered, scroll });
            *entry.active_owned.borrow_mut() = owned;
            let _ = with_provider(|p| (p.push_state)(url));
        }
        NavCommand::Replace { url, .. } => {
            *entry.active_owned.borrow_mut() = entry.owned_of(url);
            let _ = with_provider(|p| (p.replace_state)(url));
        }
        NavCommand::Reset { url, .. } => {
            entry.history.borrow_mut().clear();
            *entry.active_owned.borrow_mut() = entry.owned_of(url);
            let _ = with_provider(|p| (p.replace_state)(url));
        }
        NavCommand::Pop => {
            // The handler commits the pop synchronously; we move the
            // browser back and swallow the echoed popstate. Guarded on
            // our own recorded history so a root pop (handler no-op)
            // never backs out of the app.
            if !entry.history.borrow().is_empty() {
                PENDING_SELF_POPS.with(|c| c.set(c.get() + 1));
                let _ = with_provider(|p| (p.history_back)());
            }
        }
        NavCommand::Custom(_) => {}
    }
}

/// Coarse command classification for [`after_command`] — computed from
/// the command BEFORE dispatch moves it into the handler closure.
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum CommandKind {
    /// Push / Select / Replace / Reset — a fresh screen is now showing.
    Forward,
    Pop,
    Other,
}

impl CommandKind {
    pub(crate) fn of(cmd: &NavCommand) -> Self {
        match cmd {
            NavCommand::Push { .. }
            | NavCommand::Select { .. }
            | NavCommand::Replace { .. }
            | NavCommand::Reset { .. } => CommandKind::Forward,
            NavCommand::Pop => CommandKind::Pop,
            NavCommand::Custom(_) => CommandKind::Other,
        }
    }
}

/// Runs AFTER the handler committed the command: the outlet now shows
/// the new screen, so scroll adjustments land on the right surface.
pub(crate) fn after_command(id: u64, kind: CommandKind) {
    if RECONCILING.with(|c| c.get()) {
        return;
    }
    let Some(entry) = entry_by_id(id) else { return };
    match kind {
        CommandKind::Forward => {
            // Fresh screen starts at the top (legacy `mount_internal`
            // behavior). No-op when the outlet isn't a scroll surface.
            entry.set_scroll(0.0, 0.0);
        }
        CommandKind::Pop => {
            // Reveal bookkeeping: the popped-to entry's URL becomes the
            // active owned slice and its scroll is restored.
            let revealed = entry.history.borrow_mut().pop();
            if let Some(h) = revealed {
                *entry.active_owned.borrow_mut() = h.owned_url.clone();
                entry.set_scroll(h.scroll.0, h.scroll.1);
            }
        }
        CommandKind::Other => {}
    }
}

// ---------------------------------------------------------------------------
// Popstate reconciliation
// ---------------------------------------------------------------------------

/// Handle a platform `popstate`: the URL already changed to
/// `new_path`; translate the delta into ordinary `NavCommand`s on the
/// navigator(s) whose owned slice changed. Called by the installed
/// provider's event wiring (and directly by tests).
pub fn handle_popstate(new_path: &str) {
    // Echo of our own `history.back()` — the dispatch hooks already did
    // the bookkeeping when the handler committed the pop.
    if PENDING_SELF_POPS.with(|c| c.get()) > 0 {
        PENDING_SELF_POPS.with(|c| c.set(c.get() - 1));
        return;
    }

    let entries: Vec<Rc<NavEntry>> = REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        reg.retain(|e| e.control.strong_count() > 0);
        reg.iter().cloned().collect()
    });

    for entry in entries {
        let Some(control) = entry.control.upgrade() else { continue };
        // Not under this navigator's routes at all → not ours.
        let Some((name, params, _remainder)) = (entry.ctx.resolve_entry)(new_path) else {
            continue;
        };
        let owned = entry.owned_of(new_path);
        // Our slice is unchanged → the change belongs to a NESTED
        // navigator. Touching our screen here would tear the nested
        // navigator's subtree down mid-transition (the legacy teardown
        // race), so skip.
        if owned == *entry.active_owned.borrow() {
            continue;
        }

        // Backward: the new owned slice matches an entry in our
        // recorded history. HOW to go back depends on the navigator's
        // kind, which its link activator encodes: a stack (activator →
        // `Push`) goes back by POPPING — that many pops, since the
        // browser collapses a multi-entry jump into one popstate — while
        // a depth-less swap (activator → `Select`) has no Pop and goes
        // back by re-SELECTING the previous route.
        let match_idx = entry
            .history
            .borrow()
            .iter()
            .rposition(|h| h.owned_url == owned);
        if let Some(idx) = match_idx {
            let relative = owned
                .strip_prefix(entry.ctx.base.as_str())
                .unwrap_or(&owned)
                .to_string();
            let cmd = control.build_link_command(name, relative, params);
            RECONCILING.with(|c| c.set(true));
            match cmd {
                NavCommand::Select { .. } => control.dispatch(cmd),
                _ => {
                    let pops = entry.history.borrow().len() - idx;
                    for _ in 0..pops {
                        control.dispatch(NavCommand::Pop);
                    }
                }
            }
            RECONCILING.with(|c| c.set(false));
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
            // Forward (or unknown) navigation: dispatch through the
            // navigator's link activator so the verb matches the kind
            // (`Select` for swap, `Push` for stacks). The activator
            // takes a navigator-RELATIVE url (dispatch re-composes the
            // base).
            let relative = owned
                .strip_prefix(entry.ctx.base.as_str())
                .unwrap_or(&owned)
                .to_string();
            let covered = entry.active_owned.borrow().clone();
            let scroll = entry.current_scroll();
            let cmd = control.build_link_command(name, relative, params);
            RECONCILING.with(|c| c.set(true));
            control.dispatch(cmd);
            RECONCILING.with(|c| c.set(false));
            entry
                .history
                .borrow_mut()
                .push(HistoryEntry { owned_url: covered, scroll });
            *entry.active_owned.borrow_mut() = owned;
            entry.set_scroll(0.0, 0.0);
        }
    }
}
