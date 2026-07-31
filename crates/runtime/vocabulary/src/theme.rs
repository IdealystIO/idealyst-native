//! # Per-world theme / token / cohort state (P3c — the style-engine port)
//!
//! The old engine keeps its theme state in `runtime-core` thread-locals
//! (`TOKEN_REGISTRY`, `PENDING_TOKENS`, the cohort slab, the tokens
//! VERSION signal) and re-applies static-styled nodes through one
//! walker-installed driver Effect. This module is that machinery as
//! **vocabulary policy on the new kernel**, with one deliberate
//! re-homing decision:
//!
//! ## Where the state lives — per world, via world context
//!
//! All mutable theme state — the installed token table, the pending
//! delivery queues, the theme VERSION signal, the cohort slab, the
//! default text font, the host-surface slots — lives in a [`ThemeCtx`]
//! stored in the **owning world's context** (`runtime_world::provide`
//! keyed by type). Two worlds on one thread (app world + SSR
//! per-request worlds, the migration's flagship payoff) therefore have
//! fully independent themes; nothing here is a thread-local.
//!
//! The version signal and the driver effect are created via
//! [`runtime_world::unscoped`] so they are **world-root-owned**: the
//! first styled node to mount lives in some subtree's `collect_owned`
//! scope, and a collected driver would die with that subtree while the
//! rest of the cohort lives on — the exact regression class the old
//! core's `reactive::unscope` guard documents (runtime-core
//! `style.rs`, token-registry lifetime comment).
//!
//! Sheet **registration** (dedup + pregen + typeface registration +
//! dead-sheet sweep — the old `ensure_registered_with`) is ALSO
//! per-world here: the old function lives in runtime-core's private
//! `style` module, so the port reimplements it on public APIs
//! ([`ensure_sheet_registered`]), which happens to be the more coherent
//! home anyway — registration is backend-facing state and a world owns
//! its backend. What deliberately REMAINS thread-level (inside
//! runtime-core, which this phase does not modify): the **resolution
//! cache** behind `runtime_shared::resolve_style`. It is world-agnostic
//! by construction — keyed by token *names* (token-stable content
//! keys) — and [`update_tokens`] wipes it per swap exactly as the old
//! engine did (so `ComputedLayer` closures re-run against the new
//! theme). It migrates into a per-world home when the types leave
//! runtime-core (P7).
//!
//! ## Delivery model (mirrors the old cohort driver, `walker/theme_cohort.rs`)
//!
//! - [`install_tokens`] / [`update_tokens`] record into the world's
//!   `ThemeCtx` and bump the world VERSION signal. Nothing touches the
//!   backend yet (it may not exist).
//! - Every sheet-attach ([`crate::style_attach`]) and the per-world
//!   **theme driver** effect drain the pending queues into the backend
//!   (`StyleOps::install_tokens` / `update_tokens`) — tokens first,
//!   host-surface settings after, the same ordering invariant
//!   `style::flush_pending_host_state` documents (web emits
//!   `body { background: var(--x) }`; the var must exist first).
//! - On a version bump the driver re-fires: drain updates → if the
//!   backend cascades token updates (web `var()`), stop — the browser
//!   re-tints (the old driver's `BACKEND_CASCADE_TOKENS` fast path) —
//!   otherwise fan out every registered cohort entry's reapply closure
//!   (the old `THEME_COHORT` slab semantics, O(1) Effects for N static
//!   nodes).
//!
//! ## Known gaps, loud (see also `style_attach`)
//!
//! - **Old-core `Tokenized::resolve()` freshness on native**: native
//!   backends read token VALUES through runtime-core's thread-local
//!   registry, which this path does not write. Web is unaffected
//!   (values ship as CSS vars via `update_tokens`); the native value
//!   plumbing is re-pointed when the first native backend adopts the
//!   new core (P4).
//! - **Native breakpoint re-application**: the old driver subscribes to
//!   `current_breakpoint()` (an old-core signal); a world effect cannot
//!   subscribe to it. Values are still honored at apply time. The
//!   per-world reactive viewport/breakpoint source now exists
//!   ([`crate::viewport`], author-surface re-fire wired + web-sourced);
//!   re-pointing the native style-overlay MERGE onto it lands with the
//!   first native backend that wires a live viewport push (viewport
//!   module docs list the per-platform status). Web needs neither
//!   (`@media` CSS).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use runtime_shared::assets::TypefaceId;
use runtime_shared::{Color, FontFamily, StyleRules, StyleSheet, TokenEntry, TokenValue, Tokenized};
use runtime_world::{effect, inject, provide, signal, unscoped, Signal};

use crate::caps::{AppEnvOps, AssetOps, StyleOps};

/// Opaque id of a cohort entry (index into the per-world slab).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct CohortId(u32);

struct ThemeState {
    /// Latest full token install, pending backend delivery. Single slot
    /// (latest wins) — same contract as the old `PENDING_TOKENS`.
    pending_install: Option<Vec<TokenEntry>>,
    /// Token-update batches pending backend delivery. Accumulate — every
    /// batch reaches the backend (old `PENDING_TOKEN_UPDATES`).
    pending_updates: Vec<Vec<TokenEntry>>,
    /// The authoritative per-world token VALUE table (`install` seeds,
    /// `update` overwrites). The new-core analogue of the old
    /// `TOKEN_REGISTRY`'s values, minus the per-token signals — the
    /// version signal + cohort fan-out carry re-application instead.
    values: HashMap<&'static str, TokenValue>,
    /// The theme's default text font — filled into a resolved rule's
    /// absent `font_family` at apply time (old `DEFAULT_TEXT_FONT`,
    /// per-world now).
    default_text_font: Option<FontFamily>,
    /// Host-surface background pending delivery (old `PENDING_APP_BG`).
    pending_app_background: Option<Tokenized<Color>>,
    /// Scrollbar theme pending delivery (old `PENDING_SCROLLBAR`).
    pending_scrollbar: Option<(Tokenized<Color>, Tokenized<Color>)>,
    /// The static-style cohort: one reapply closure per registered node
    /// (or node batch). `Rc` so the fan-out can clone the list out of
    /// the borrow before running entries (entries re-enter this
    /// RefCell via `default_text_font()` / pending drains).
    cohort: Vec<Option<Rc<dyn Fn()>>>,
    cohort_free: Vec<u32>,
    /// One driver per world (cleared by the driver effect's cleanup on
    /// world teardown, mirroring the old `DriverGuard`).
    driver_installed: bool,
    /// Set when a `StyleProp::Preminted` attach happened in this world:
    /// the driver then also (re)publishes the default text font at the
    /// document level on every version bump — the old premint host
    /// driver's `apply_default_text_font` contract (preminted classes
    /// reference `var(--iy-default-font)`).
    premint_used: bool,
    /// Per-world sheet registrations (old `REGISTRATIONS`): sheet
    /// pointer → (liveness Weak, the pregenerated rules the backend was
    /// handed — kept so `unregister_stylesheet` can free them).
    registrations: HashMap<usize, (Weak<StyleSheet>, Rc<Vec<Rc<StyleRules>>>)>,
    /// Rule sets queued for `unregister_stylesheet` (old
    /// `PENDING_UNREGISTER`) — populated by the dead-Weak sweep,
    /// drained on the next [`ensure_sheet_registered`].
    pending_unregister: Vec<Rc<Vec<Rc<StyleRules>>>>,
    /// Typefaces already registered with this world's backend (old
    /// `REGISTERED_TYPEFACES`): `register_asset` + `register_typeface`
    /// fire once per `TypefaceId` no matter how many sheets reference
    /// it.
    registered_typefaces: HashSet<TypefaceId>,
}

impl Default for ThemeState {
    fn default() -> Self {
        ThemeState {
            pending_install: None,
            pending_updates: Vec::new(),
            values: HashMap::new(),
            default_text_font: None,
            pending_app_background: None,
            pending_scrollbar: None,
            cohort: Vec::new(),
            cohort_free: Vec::new(),
            driver_installed: false,
            premint_used: false,
            registrations: HashMap::new(),
            pending_unregister: Vec::new(),
            registered_typefaces: HashSet::new(),
        }
    }
}

/// The per-world theme context: interior-mutable state + the world
/// VERSION signal. Cloneable (context values must be) — clones share
/// the same state.
#[derive(Clone)]
pub struct ThemeCtx {
    state: Rc<RefCell<ThemeState>>,
    /// Bumped once per install/update batch. World-root-owned. Style
    /// effects and the theme driver subscribe to it — the new-kernel
    /// analogue of the old `tokens_version_signal()` (which exists
    /// precisely because per-token subscriptions go deaf behind the
    /// resolution cache; the version signal never does).
    version: Signal<u64>,
}

thread_local! {
    /// The most recent ambient world's `ThemeCtx` — the HANDLER fallback.
    /// Platform event handlers run OUTSIDE `World::enter` (the flush
    /// driver commits afterwards), so the ambient free fns below cannot
    /// `inject` there; they fall back to this capture instead — capture,
    /// don't inject. Refreshed on every ambient [`theme_ctx`] call, so it
    /// tracks whichever world was ambient last; entered callers never
    /// consult it (per-world isolation is preserved wherever a world IS
    /// ambient), and a stale capture is inert, not dangerous — the ctx
    /// state is plain `Rc` memory and its version signal's writes no-op
    /// on a dead world. Found live: idea-theme's `set_theme` from the
    /// docs-shell dark-theme button panicked "outside World::enter"
    /// through the ambient `inject`.
    static LAST_CTX: RefCell<Option<ThemeCtx>> = const { RefCell::new(None) };
}

/// The ambient world's theme context, created (and `provide`d) on first
/// use. Must run inside `World::enter` — same contract as every
/// creation-side kernel API. (Handler-side callers go through the free
/// fns below, which fall back to the last ambient world's ctx.)
pub fn theme_ctx() -> ThemeCtx {
    if let Some(ctx) = inject::<ThemeCtx>() {
        LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
        return ctx;
    }
    let ctx = ThemeCtx {
        state: Rc::new(RefCell::new(ThemeState::default())),
        // World-root-owned: the version signal must outlive whatever
        // mount scope first touched the theme (module docs).
        version: unscoped(|| signal(0u64)),
    };
    // The PROVISION is world-root too, not just the signal: `provide` binds
    // an entry to the ambient collector, and first-touch happens inside
    // whatever subtree's mount got here first. Scope-owned, the ctx would
    // stop being injectable the moment that subtree unmounted and the next
    // `theme_ctx()` would build a SECOND ctx around a fresh version signal
    // — silently orphaning every effect subscribed to the first.
    unscoped(|| provide(ctx.clone()));
    LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
    ctx
}

/// The handler-safe ctx resolution every ambient free fn in this module
/// uses: the ambient world's ctx when entered, else the thread's last
/// ambient ctx. Only when NO world ever created a theme ctx does this
/// fall through to the ambient path's canonical outside-enter panic
/// (there is genuinely nothing to stage into).
fn theme_ctx_handler_safe() -> ThemeCtx {
    if !runtime_world::is_entered() {
        if let Some(ctx) = LAST_CTX.with(|c| c.borrow().clone()) {
            return ctx;
        }
    }
    theme_ctx()
}

/// The ambient world's theme VERSION signal. Reading it inside an
/// effect subscribes that effect to every subsequent theme/token change.
pub(crate) fn version_signal() -> Signal<u64> {
    theme_ctx().version
}

// ---------------------------------------------------------------------------
// The theme surface — METHODS on the capturable ctx, plus ambient
// free-fn conveniences.
//
// The free fns require the ambient world (`World::enter`), which holds
// during component builds, effects, and flushes — but NOT inside
// platform event handlers (a DOM `click` closure fires straight from
// the browser; the flush driver commits afterwards). A handler that
// swaps the theme must therefore CAPTURE the `ThemeCtx` at build time
// and call the method — the same handle-capture discipline world
// signals already impose. (Found live: the web-smoke "Swap theme"
// button panicked "outside World::enter" through the free fn.)
// ---------------------------------------------------------------------------

impl ThemeCtx {
    /// See [`install_tokens`]. Handler-safe (no ambient world needed).
    pub fn install_tokens(&self, tokens: &[TokenEntry]) {
        {
            let mut s = self.state.borrow_mut();
            for t in tokens {
                s.values.insert(t.name, t.value.clone());
            }
            s.pending_install = Some(tokens.to_vec());
        }
        self.version.update(|v| v + 1);
    }

    /// See [`update_tokens`]. Handler-safe (no ambient world needed).
    pub fn update_tokens(&self, tokens: &[TokenEntry]) {
        {
            let mut s = self.state.borrow_mut();
            for t in tokens {
                s.values.insert(t.name, t.value.clone());
            }
            s.pending_updates.push(tokens.to_vec());
        }
        // Wipe runtime-core's memoized resolution so subsequent resolves
        // re-run `ComputedLayer` closures against the new theme — the old
        // `update_tokens` contract. `reset_for_ssg_render` is the one
        // PUBLIC entry that clears that cache; its other clears are inert
        // on the new path (core's REGISTRATIONS / pending queues / typeface
        // dedup are unused here — the per-world tables in this module own
        // those roles).
        runtime_shared::reset_for_ssg_render();
        self.version.update(|v| v + 1);
    }

    /// See [`set_default_text_font`]. Handler-safe.
    pub fn set_default_text_font(&self, font: Option<FontFamily>) {
        self.state.borrow_mut().default_text_font = font;
        self.version.update(|v| v + 1);
    }

    /// See [`set_app_background`]. Handler-safe.
    pub fn set_app_background(&self, color: Tokenized<Color>) {
        self.state.borrow_mut().pending_app_background = Some(color);
        self.version.update(|v| v + 1);
    }

    /// See [`set_scrollbar_theme`]. Handler-safe.
    pub fn set_scrollbar_theme(&self, thumb: Tokenized<Color>, track: Tokenized<Color>) {
        self.state.borrow_mut().pending_scrollbar = Some((thumb, track));
        self.version.update(|v| v + 1);
    }

    /// See [`token_value`]. Handler-safe.
    pub fn token_value(&self, name: &str) -> Option<TokenValue> {
        self.state.borrow().values.get(name).cloned()
    }

    /// The world's theme VERSION signal (Copy handle) — for per-fire
    /// hot paths that captured the ctx at attach time (same rationale
    /// as [`Self::sheet_is_registered`]). Equivalent to the free
    /// `version_signal()` minus the per-call `inject`.
    pub(crate) fn version(&self) -> Signal<u64> {
        self.version
    }

    /// Ctx-based [`sheet_is_registered`] — for per-fire hot paths
    /// (style binding effects) that captured the ctx at attach time:
    /// skips the per-call `inject` world-context lookup the free fn
    /// pays (measured: 3 context lookups per node per fire on the
    /// dynamic style path — the js-framework-bench gate work).
    pub(crate) fn sheet_is_registered(&self, sheet: &Rc<StyleSheet>) -> bool {
        self.state
            .borrow()
            .registrations
            .contains_key(&(Rc::as_ptr(sheet) as usize))
    }

    /// Ctx-based [`default_text_font`] — same hot-path rationale as
    /// [`Self::sheet_is_registered`].
    pub(crate) fn default_text_font(&self) -> Option<FontFamily> {
        self.state.borrow().default_text_font.clone()
    }
}

/// Install the initial token set for the ambient world. Values are
/// recorded per-world and queued for the backend; the next sheet attach
/// or driver run delivers them (`StyleOps::install_tokens`). Re-install
/// counts as a theme change (version bump) — same as the old core.
/// Ambient convenience — from an event handler, capture [`theme_ctx`]
/// at build time and use [`ThemeCtx::install_tokens`].
pub fn install_tokens(tokens: &[TokenEntry]) {
    theme_ctx_handler_safe().install_tokens(tokens);
}

/// Push updated token values (a theme swap). Recorded per-world, queued
/// as an update batch, version bumped — on the owning world's next
/// flush the driver delivers the batch to the backend and (on
/// non-cascade backends) re-applies every cohort node; dynamic style
/// effects re-fire through their own version subscription. Ambient
/// convenience — from an event handler, capture [`theme_ctx`] at build
/// time and use [`ThemeCtx::update_tokens`].
pub fn update_tokens(tokens: &[TokenEntry]) {
    theme_ctx_handler_safe().update_tokens(tokens);
}

/// Install the theme's default text [`FontFamily`] for the ambient
/// world (`None` clears). Filled into a resolved rule's absent
/// `font_family` at apply time — the cross-platform theme-font default
/// (old `set_default_text_font`, per-world now). Bumps the version so
/// already-applied nodes re-fill.
pub fn set_default_text_font(font: Option<FontFamily>) {
    theme_ctx_handler_safe().set_default_text_font(font);
}

/// The ambient world's default text font, if any.
pub fn default_text_font() -> Option<FontFamily> {
    theme_ctx_handler_safe().state.borrow().default_text_font.clone()
}

/// Theme the host surface behind the rendered tree (old
/// `set_app_background`): single pending slot, delivered on the next
/// sheet attach / driver run, after tokens.
pub fn set_app_background(color: Tokenized<Color>) {
    theme_ctx_handler_safe().set_app_background(color);
}

/// Theme the platform scrollbar (old `set_scrollbar_theme`): single
/// pending slot, same delivery as [`set_app_background`].
pub fn set_scrollbar_theme(thumb: Tokenized<Color>, track: Tokenized<Color>) {
    theme_ctx_handler_safe().set_scrollbar_theme(thumb, track);
}

/// The ambient world's current value for a token name, if installed.
/// (The per-world table; native `Tokenized::resolve` re-pointing lands
/// at P4 — module docs.)
pub fn token_value(name: &str) -> Option<TokenValue> {
    theme_ctx_handler_safe().token_value(name)
}

/// Drain the pending host-level state into `backend` — tokens first
/// (install, then update batches), host-surface settings after. Shared
/// by every sheet attach (so the first styled mount delivers the
/// pre-mount `install_tokens`, in the same relative position the old
/// `ensure_registered_with` prologue produced: install_tokens BEFORE
/// the first `register_stylesheet`) and by the driver (so swaps reach
/// backends even when no new sheet registers).
pub(crate) fn flush_pending_host_state<H: StyleOps + crate::caps::AppEnvOps>(
    backend: &Rc<RefCell<H>>,
) {
    let ctx = theme_ctx();
    // Take everything out of the borrow first — backend calls may
    // re-enter theme state (e.g. a recorder that styles something).
    let (install, updates, app_bg, scrollbar) = {
        let mut s = ctx.state.borrow_mut();
        (
            s.pending_install.take(),
            std::mem::take(&mut s.pending_updates),
            s.pending_app_background.take(),
            s.pending_scrollbar.take(),
        )
    };
    if let Some(tokens) = install {
        backend.borrow_mut().install_tokens(&tokens);
    }
    for batch in &updates {
        backend.borrow_mut().update_tokens(batch);
    }
    if let Some(color) = app_bg {
        backend.borrow_mut().set_app_background(&color);
    }
    if let Some((thumb, track)) = scrollbar {
        backend.borrow_mut().set_scrollbar_theme(&thumb, &track);
    }
}

/// Register a static-styled node's reapply closure with the ambient
/// world's cohort. Slab + freelist, same shape (and same O(1)-Effects
/// rationale) as the old `theme_cohort_register`.
pub(crate) fn cohort_register(reapply: Rc<dyn Fn()>) -> CohortId {
    let ctx = theme_ctx();
    let mut s = ctx.state.borrow_mut();
    if let Some(idx) = s.cohort_free.pop() {
        s.cohort[idx as usize] = Some(reapply);
        CohortId(idx)
    } else {
        let idx = s.cohort.len() as u32;
        s.cohort.push(Some(reapply));
        CohortId(idx)
    }
}

impl ThemeCtx {
    /// Remove a cohort entry. A METHOD (not an ambient-world free fn) on
    /// purpose: teardown runs from `Owned`/`Realized` drops, which
    /// happen OUTSIDE `World::enter` — the attach path captures the ctx
    /// at mount time so unregistration never consults the ambient world
    /// (whose absence would panic in a plain drop; regression:
    /// `sheet_static_cohort_reapplies_on_theme_swap`'s unmount step).
    pub(crate) fn cohort_unregister(&self, id: CohortId) {
        let mut s = self.state.borrow_mut();
        if let Some(slot) = s.cohort.get_mut(id.0 as usize) {
            if slot.take().is_some() {
                s.cohort_free.push(id.0);
            }
        }
    }
}

/// Mark that a preminted style was attached in this world — the driver
/// then also publishes the default text font at the document level
/// (old premint host driver contract).
pub(crate) fn mark_premint_used() {
    theme_ctx().state.borrow_mut().premint_used = true;
}

/// Pointer-keyed fast path (old `is_registered`): `true` iff this exact
/// `StyleSheet` instance is live-registered in the ambient world.
/// Steady-state style-effect re-fires use it to skip
/// [`ensure_sheet_registered`]'s sweep/flush prologue.
pub(crate) fn sheet_is_registered(sheet: &Rc<StyleSheet>) -> bool {
    let ctx = theme_ctx();
    let s = ctx.state.borrow();
    s.registrations.contains_key(&(Rc::as_ptr(sheet) as usize))
}

/// Ensure the backend has pre-generated state for `sheet` — the
/// per-world port of the old `ensure_registered_with` prologue + body,
/// on public runtime-core APIs:
///
/// 1. Drain the pending host state (tokens FIRST — web rules reference
///    `var(--…)`; the variable must exist before the rule parses).
/// 2. Sweep registrations whose `Weak<StyleSheet>` died into the
///    unregister queue; drain the queue (`unregister_stylesheet`).
/// 3. Already registered? Done.
/// 4. Pregenerate the sheet's resolvable rule set
///    (`runtime_shared::pregenerate`), register any typefaces the rules
///    reference (`register_asset` per face + `register_typeface`,
///    deduped per world), then `register_stylesheet`.
///
pub(crate) fn ensure_sheet_registered<H: StyleOps + AssetOps + AppEnvOps>(
    backend: &Rc<RefCell<H>>,
    sheet: &Rc<StyleSheet>,
) {
    flush_pending_host_state(backend);
    let ctx = theme_ctx();
    let key = Rc::as_ptr(sheet) as usize;

    // Sweep + collect the drain OUTSIDE the borrow (backend calls may
    // re-enter theme state).
    let (pending, already) = {
        let mut s = ctx.state.borrow_mut();
        let dead: Vec<usize> = s
            .registrations
            .iter()
            .filter(|(_, (weak, _))| weak.upgrade().is_none())
            .map(|(k, _)| *k)
            .collect();
        for k in dead {
            if let Some((_, rules)) = s.registrations.remove(&k) {
                s.pending_unregister.push(rules);
            }
        }
        (
            std::mem::take(&mut s.pending_unregister),
            s.registrations.contains_key(&key),
        )
    };
    for rules in &pending {
        backend.borrow_mut().unregister_stylesheet(&rules[..]);
    }
    if already {
        return;
    }

    // Pregenerate outside any borrow (sheet closures are arbitrary
    // author code). The SEEDING variant also populates the sheet's
    // pointer-keyed resolve fast path — without it every
    // `resolve_style` on this sheet takes the slow ResolutionKey path
    // and returns pointer-distinct rules, defeating backends'
    // pointer-keyed class caches (the create_10k bench-gate residual).
    let rules: Rc<Vec<Rc<StyleRules>>> = Rc::new(runtime_shared::pregenerate_and_seed(sheet));

    // Typeface registration BEFORE the sheet ships (old
    // `ensure_typefaces_registered_with` contract: every `apply_style`
    // referencing a typeface finds the family registered).
    for r in rules.iter() {
        if let Some(FontFamily::Typeface(tf)) = &r.font_family {
            let fresh = ctx.state.borrow_mut().registered_typefaces.insert(tf.id);
            if fresh {
                let mut b = backend.borrow_mut();
                for face in tf.faces {
                    b.register_asset(face.asset, runtime_shared::assets::AssetTag::Font, &face.source);
                }
                b.register_typeface(tf.id, tf.family_name, tf.faces, tf.fallback);
            }
        }
    }

    backend.borrow_mut().register_stylesheet(&rules[..]);
    ctx.state
        .borrow_mut()
        .registrations
        .insert(key, (Rc::downgrade(sheet), rules));
}

/// Install (idempotently, per world) the theme driver: ONE world-root
/// effect that subscribes to the version signal and, on each theme
/// change, drains pending token/host state into the backend and — on
/// backends whose token updates don't cascade — fans out every cohort
/// entry's reapply closure. Port of `install_theme_cohort_driver` +
/// `install_premint_host_driver` (they were split in the old core only
/// because premint apps register no sheets; here both drains share the
/// driver).
pub(crate) fn ensure_theme_driver<H: StyleOps + crate::caps::AppEnvOps>(
    backend: &Rc<RefCell<H>>,
) {
    let ctx = theme_ctx();
    {
        let mut s = ctx.state.borrow_mut();
        if s.driver_installed {
            return;
        }
        s.driver_installed = true;
    }

    let backend = backend.clone();
    let version = ctx.version;
    let state = Rc::downgrade(&ctx.state);
    // World-root-owned: the driver must outlive the mounting subtree
    // (module docs). Its cleanup clears the installed flag so a
    // hypothetical re-init after teardown reinstalls — mirroring the
    // old DriverGuard; with per-world state this only matters if the
    // ctx outlives the effect, which the Weak makes safe either way.
    let _driver = unscoped(|| {
        effect(move || {
            // Subscribe: every install/update/font/host-surface change
            // bumps the version.
            let _ = version.get();
            flush_pending_host_state(&backend);
            let Some(state) = state.upgrade() else {
                return;
            };
            // Premint worlds publish the theme's default font at the
            // document level on every theme change (the var preminted
            // classes reference).
            let (premint_used, font) = {
                let s = state.borrow();
                (s.premint_used, s.default_text_font.clone())
            };
            if premint_used {
                backend.borrow_mut().apply_default_text_font(font.as_ref());
            }
            // Cascade fast path (old BACKEND_CASCADE_TOKENS): web's
            // `var()` cascade re-tints without per-node fan-out.
            if backend.borrow().token_updates_propagate_via_cascade() {
                return;
            }
            // Fan out. Clone the entries out of the borrow first — the
            // reapply closures re-enter theme state (default-font fill,
            // pending drains inside ensure_registered_with).
            let entries: Vec<Rc<dyn Fn()>> = state
                .borrow()
                .cohort
                .iter()
                .filter_map(|e| e.clone())
                .collect();
            for entry in entries {
                entry();
            }
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_world::World;

    #[test]
    fn theme_state_is_per_world() {
        let a = World::new();
        let b = World::new();
        a.enter(|| {
            install_tokens(&[TokenEntry {
                name: "color-surface",
                value: TokenValue::Color(Color("#111".into())),
            }]);
        });
        b.enter(|| {
            install_tokens(&[TokenEntry {
                name: "color-surface",
                value: TokenValue::Color(Color("#fff".into())),
            }]);
        });
        a.flush();
        b.flush();
        let va = a.enter(|| token_value("color-surface"));
        let vb = b.enter(|| token_value("color-surface"));
        assert_eq!(va, Some(TokenValue::Color(Color("#111".into()))));
        assert_eq!(vb, Some(TokenValue::Color(Color("#fff".into()))));
    }

    #[test]
    fn version_bumps_once_per_batch() {
        let world = World::new();
        world.enter(|| {
            let ctx = theme_ctx();
            let v0 = ctx.version.get();
            update_tokens(&[
                TokenEntry { name: "a", value: TokenValue::Number(1.0) },
                TokenEntry { name: "b", value: TokenValue::Number(2.0) },
            ]);
            world.flush();
            assert_eq!(ctx.version.get(), v0 + 1, "one bump per update batch");
        });
    }

    /// The idea-ui-docs dark-theme-button crash: `set_theme` →
    /// `update_tokens` runs from a platform event handler, OUTSIDE
    /// `World::enter` — the ambient `inject` used to panic "outside
    /// World::enter". The swap must instead stage into the last ambient
    /// world's ctx (capture, don't inject), notify the version
    /// subscribers on that world's next flush, and keep per-world
    /// isolation for entered callers.
    #[test]
    fn regression_theme_swap_outside_enter_stages_and_commits_on_flush() {
        let world = World::new();
        let (ctx, fires) = world.enter(|| {
            install_tokens(&[TokenEntry {
                name: "color-surface",
                value: TokenValue::Color(Color("#fff".into())),
            }]);
            let ctx = theme_ctx();
            let version = ctx.version;
            let fires = Rc::new(std::cell::Cell::new(0usize));
            let fires_c = fires.clone();
            // Stand-in for the theme driver: subscribes to the version.
            let _d = unscoped(|| {
                effect(move || {
                    let _ = version.get();
                    fires_c.set(fires_c.get() + 1);
                })
            });
            (ctx, fires)
        });
        world.flush();
        let baseline = fires.get();

        // The handler: no ambient world. Must not panic, must stage.
        update_tokens(&[TokenEntry {
            name: "color-surface",
            value: TokenValue::Color(Color("#0a0e17".into())),
        }]);
        assert_eq!(
            world.enter(|| token_value("color-surface")),
            Some(TokenValue::Color(Color("#0a0e17".into()))),
            "the value table updates immediately (Rc state, not staged)"
        );

        // Commit on flush: the version subscribers (driver) re-fire once.
        world.flush();
        assert_eq!(fires.get(), baseline + 1, "swap committed on the owning world's flush");

        // Handler-safe pending delivery: the update batch is queued for
        // the backend drain.
        assert_eq!(ctx.state.borrow().pending_updates.len(), 1);

        // Entered callers still resolve per-world: a second world's
        // install lands in ITS ctx, not the captured one.
        let other = World::new();
        other.enter(|| {
            install_tokens(&[TokenEntry {
                name: "color-surface",
                value: TokenValue::Color(Color("#123".into())),
            }]);
        });
        assert_eq!(
            world.enter(|| token_value("color-surface")),
            Some(TokenValue::Color(Color("#0a0e17".into()))),
            "per-world isolation preserved for entered callers"
        );
    }

    #[test]
    fn cohort_slab_recycles_slots() {
        let world = World::new();
        world.enter(|| {
            let a = cohort_register(Rc::new(|| {}));
            let b = cohort_register(Rc::new(|| {}));
            assert_ne!(a, b);
            let ctx = theme_ctx();
            ctx.cohort_unregister(a);
            let c = cohort_register(Rc::new(|| {}));
            assert_eq!(a, c, "freed slot is reused");
            ctx.cohort_unregister(b);
            ctx.cohort_unregister(c);
        });
    }
}
