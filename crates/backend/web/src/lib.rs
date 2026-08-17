//! Web backend: drives DOM nodes via web-sys/wasm-bindgen.
//!
//! # File layout
//!
//! - `style.rs` — CSS converters (`rules_to_css` + per-enum helpers),
//!   stylesheet rule-index bookkeeping (`insert_rule` / `delete_rule`
//!   on `WebBackend`), and the register/apply Backend methods that
//!   live next to the data they mutate.
//! - `defaults.rs` — global baselines: `.ui-default` class, spinner
//!   keyframes, virtualizer JS shim, dynamic-slot teardown.
//! - `primitives/` — one module per `Element` kind. Each owns its
//!   create/update functions, any `Ops` impl, and the `make_*_handle`
//!   builder where applicable. The `impl Backend for WebBackend`
//!   block at the bottom of this file is a thin delegation layer.
//!
//! # Style architecture
//!
//! Two distinct caches:
//!
//! - **Pre-generated cache.** Holds classes minted via
//!   `register_stylesheet` — variant combinations × theme. Content-keyed
//!   and shared across nodes. Lifecycle is anchored by the framework's
//!   `register_stylesheet` / `unregister_stylesheet` calls.
//!
//! - **Dynamic slots, one per styled node.** When a node's resolved
//!   style doesn't match any pre-generated class, the backend mints a
//!   per-node class for it. Each styled node owns at most one dynamic
//!   class. When the node's resolved style changes:
//!   1. Mint the new class (insert a CSS rule).
//!   2. Swap the node's `className`.
//!   3. Remove the old class's CSS rule.
//!
//! Dynamic classes are not shared across nodes — two nodes with the
//! same dynamic style get separate classes. The cost (slight CSS
//! duplication) is intentional: it eliminates content-keyed cache
//! contention for per-instance values and keeps dynamic-class lifecycle
//! simple (one class per node, replaced atomically).

mod a11y;
mod animated;
mod keyframes;
mod batch_queue;
#[cfg(feature = "robot")]
mod introspect;
pub mod newcore;
mod newcore_url_sync;
#[cfg(idealyst_premint)]
mod premint_guard;
#[cfg(test)]
mod tests;
#[cfg(feature = "async-driver")]
pub mod async_executor;
mod assets;
mod defaults;
#[cfg(feature = "runtime-server")]
pub mod dev_transport;
#[cfg(feature = "robot")]
#[cfg(feature = "robot")]
pub mod robot_transport;
#[cfg(feature = "robot")]
#[cfg(feature = "robot")]
mod robot_screenshot;
pub mod dispatch_hook;
pub mod drop_deferral;
pub mod logger;
mod phase_timer;
mod primitives;
#[cfg(feature = "async-driver")]
pub mod render_loop;
pub mod scheduler;
mod style;
pub mod time_source;
pub mod url_provider;
mod viewport_observer;

#[cfg(feature = "async-driver")]
pub use async_executor::install_async_executor;
#[cfg(feature = "runtime-server")]
pub use dev_transport::{connect_web, WebClientHandle};
#[cfg(feature = "robot")]
pub use robot_transport::install_robot_relay_client;
pub use drop_deferral::install_drop_deferral;
pub use logger::install_logger;
#[cfg(feature = "async-driver")]
pub use render_loop::install_render_loop;
pub use scheduler::install_scheduler;
pub use time_source::{install_time_source, install_wall_clock_source};
pub use viewport_observer::{install_viewport_observer, page_is_prerendered, ssr_viewport};

/// Install a `Weak` self-handle for the active `WebBackend`. Required
/// by any code path that needs `&mut WebBackend` from outside the
/// build walker:
///  - [`AnimatedValue::bind`](runtime_shared::animation::AnimatedValue::bind)
///    and friends (per-frame animation writes from author closures).
///  - The batched text-update microtask flush
///    ([`Backend::create_text_with_id`] / [`Backend::update_text_by_id`]).
///  - Future per-frame writers that fire outside a backend borrow.
///
/// Call once after constructing the backend `Rc<RefCell<>>`. Idempotent
/// — re-installing overwrites the previous handle.
///
/// The handle is held as a `Weak` so the backend `Rc` still drops
/// cleanly on app teardown; queued callbacks upgrade to `None` and
/// become silent no-ops once the backend is gone.
///
/// Same shape as `backend_ios_mobile::install_global_self` /
/// `backend_android_mobile::install_global_self` — keeps the wrapper
/// boilerplate uniform across platforms.
pub fn install_global_self(backend: &std::rc::Rc<std::cell::RefCell<WebBackend>>) {
    WEB_BACKEND_HANDLE.with(|s| *s.borrow_mut() = Some(std::rc::Rc::downgrade(backend)));
}

/// Push a scalar animation property update to `node` on the installed
/// global backend. Same shape as `backend_ios_mobile::set_animated_f32`
/// / `backend_android_mobile::set_animated_f32`; the framework's
/// `ViewOps::set_animated_f32` dispatch routes here for the web
/// backend so author code never needs to call it directly.
///
/// No-ops cleanly if [`install_global_self`] hasn't been called yet,
/// the install has been dropped, or the backend is currently
/// borrowed (an in-flight call will pick the new value up on its
/// next frame).
pub fn set_animated_f32(
    node: &web_sys::Node,
    prop: runtime_shared::animation::AnimProp,
    value: f32,
) {
    // Clone the `Weak` inside the closure so the thread-local borrow
    // drops before we upgrade — same pattern as
    // `backend_ios::set_animated_f32`. Holding the borrow across the
    // upgrade would extend the Ref's lifetime past the `with` block
    // and trip a borrow-checker error.
    let weak = WEB_BACKEND_HANDLE.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        b.set_animated_f32_impl(node, prop, value);
    };
}

/// Color-family counterpart of [`set_animated_f32`]. Routes through
/// the global backend's `set_animated_color`. `value` is sRGB
/// `[r, g, b, a]` with channels in `0..=1`.
pub fn set_animated_color(
    node: &web_sys::Node,
    prop: runtime_shared::animation::AnimProp,
    value: [f32; 4],
) {
    let weak = WEB_BACKEND_HANDLE.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        b.set_animated_color_impl(node, prop, value);
    };
}

/// `true` if `el`'s `class` attribute contains `class` as a whole token.
fn element_has_class(el: &web_sys::Element, class: &str) -> bool {
    el.class_name().split_whitespace().any(|c| c == class)
}

/// During hydration, point the adoption cursor at the first element child
/// of `region` (an adopted frame slot / body outlet) so the next walker
/// `create_*` calls adopt the server content inside it. No-op off
/// hydration.
#[cfg(feature = "hydrate")]
pub fn hydrate_enter(region: &web_sys::Node) {
    let weak = WEB_BACKEND_HANDLE.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        if !b.hydrating {
            return;
        }
        let first = region
            .dyn_ref::<web_sys::Element>()
            .and_then(|el| el.first_element_child());
        b.hydration_cursor = first.map(|e| e.unchecked_into::<web_sys::Node>());
        b.hydration_suppress = false;
        b.hydration_pending_fresh = false;
    };
}

/// Off-feature stub — `hydrate_enter` is a no-op when the `hydrate`
/// feature is disabled. SDK navigator helpers call this on every region
/// entry; this stub keeps them callable without `#[cfg]` plumbing in
/// the SDK crates.
#[cfg(not(feature = "hydrate"))]
pub fn hydrate_enter(_region: &web_sys::Node) {}

/// Whether an SSR-hydration pass is currently in progress. Borrow-free
/// (reads the scheduler's hydration-buffer thread-local, not the backend),
/// so navigator SDK code can call it while holding `&mut WebBackend`. Used
/// to make the navigator's INITIAL screen mount authoritative-and-adopting
/// during hydration (via the walker's `attach_initial`) while the deferred
/// create-time auto-mount microtask skips — otherwise the initial screen is
/// built twice (walker adopts the SSR screen, microtask builds a fresh one)
/// and the whole screen duplicates.
#[cfg(feature = "hydrate")]
pub fn is_hydrating() -> bool {
    crate::scheduler::is_hydration_active()
}

/// Off-feature stub — never hydrating without the `hydrate` feature.
#[cfg(not(feature = "hydrate"))]
pub fn is_hydrating() -> bool {
    false
}

/// Install a self-handle so the batched text-update path
/// ([`Backend::create_text_with_id`] / [`Backend::update_text_by_id`])
/// can schedule its microtask flush. Must be called once after the
/// app's `Rc<RefCell<WebBackend>>` is constructed; if it's never
/// called, `create_text_with_id` returns `None` and the framework
/// falls back to the unbatched `update_text` path automatically.
///
/// Superset of [`install_global_self`] — installs the same handle
/// plus pre-injects the JS-side text/class binding shims. Apps that
/// only need animation routing (no reactive text bindings) can call
/// `install_global_self` alone and skip the shim injection cost.
pub fn install_text_batcher(backend: &std::rc::Rc<std::cell::RefCell<WebBackend>>) {
    install_global_self(backend);
    // Pre-inject the JS-side reactive-binding shim so it's
    // available for console-driven smoke tests (`__idealystBindingsSmokeTest()`)
    // before any text binding is actually registered through the
    // framework. Cheap (~0.5 ms for the eval); same pattern as
    // the batched-text shim's lazy injection on first use, just
    // pulled forward.
    backend.borrow_mut().ensure_text_bindings_shim();
    // Pre-inject the class-batch shim so the first style apply at
    // mount doesn't pay an injection round-trip mid-apply. Same
    // shape as the text-bindings pre-inject above.
    backend.borrow_mut().ensure_class_batch_shim();
    // Pre-inject the class-bindings shim (the JS-side dispatcher
    // for `StyleSource::SignalClass`). Tapping the existing
    // signal-changed handler in `text_bindings.js`, so the order
    // of injection matters — `ensure_class_bindings_shim`
    // internally re-ensures its deps before injecting.
    backend.borrow_mut().ensure_class_bindings_shim();
}

std::thread_local! {
    /// `Weak` self-handle to the active `WebBackend` so the
    /// microtask scheduled inside `update_text_by_id` /
    /// `release_text_id` can find its way back to a `&mut self`
    /// borrow without cyclic Rcs.
    ///
    /// Set by [`install_text_batcher`]. Single-threaded by virtue
    /// of being a thread_local in wasm32 (single-threaded by
    /// platform). For multi-backend pages the handle gets
    /// overwritten — `create_text_with_id` always reads back the
    /// most-recently-installed one.
    static WEB_BACKEND_HANDLE: std::cell::RefCell<Option<std::rc::Weak<std::cell::RefCell<WebBackend>>>> =
        const { std::cell::RefCell::new(None) };

    /// `@font-face` rules already present in the document, keyed by the
    /// exact rule text (`css::font_face_css`). Shared across EVERY
    /// `WebBackend` on the (single wasm) thread — the main page backend,
    /// the SSR `<head>` it adopts, AND each lazy chunk's own
    /// `mount_chunk` backend. A face must be injected at most once:
    /// otherwise a second `@font-face` for the same URL makes the browser
    /// fetch the font file AGAIN (the lazy-chunk double-download bug). The
    /// SSR/hydration case seeds this set without injecting (the rule is
    /// already in the server `<head>`); the live page injects on first
    /// sight; chunks then find it present and skip.
    static FONT_FACES_PRESENT: std::cell::RefCell<FxHashSet<String>> =
        std::cell::RefCell::new(FxHashSet::default());
}

/// EXPERIMENT (External-anchoring probe): run `f` with the ambient WebBackend —
/// the most-recently-installed one, reached weakly. This lets a lazy chunk build
/// an external's node ITSELF (running the SDK handler inside the chunk) instead
/// of routing through the main-resident `ExternalRegistry` + `call_indirect`,
/// which anchors the handler's code (and its wgpu/vello deps) in `main.wasm`.
/// Returns `None` if no backend is installed or it's currently borrowed.
pub fn with_ambient_backend<R>(f: impl FnOnce(&mut WebBackend) -> R) -> Option<R> {
    let weak = WEB_BACKEND_HANDLE.with(|h| h.borrow().clone())?;
    let rc = weak.upgrade()?;
    let mut b = rc.try_borrow_mut().ok()?;
    Some(f(&mut b))
}

use runtime_shared::{
    AssetId, AssetSource, AssetTag, ButtonHandle, StyleRules, SystemFallback,
    TypefaceFace, TypefaceId,
};
use runtime_shared::{FxHashMap, FxHashSet};
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Document, Node};

/// Read the `data-navigator-id` attribute the SDK helpers crate stamps
/// on each navigator container. Returns `None` when `node` isn't an
/// Element or the attribute isn't present — every Backend trait nav
/// method gracefully no-ops in that case.
fn nav_id_from_node(node: &Node) -> Option<u32> {
    let elem: web_sys::Element = node.clone().dyn_into().ok()?;
    elem.get_attribute("data-navigator-id")?.parse().ok()
}

/// No-op `NavigatorOps` returned by `make_navigator_handle` when no
/// SDK handler is registered for the given node. Keeps the
/// fallback handle inert without depending on the helpers crate
/// (which would be circular: helpers depends on backend-web).
struct NoopNavOps;
impl runtime_shared::primitives::navigator::NavigatorOps for NoopNavOps {}
static NOOP_NAV_OPS: NoopNavOps = NoopNavOps;

// NOTE: the `BACKEND_NAV_ID` counter and `next_backend_nav_id()` lived here
// to keep backend-assigned navigator ids from colliding with the low ids the
// `web-navigator-helpers` crate stamped for the legacy handlers. Both that
// crate and those handlers went with the runtime-v2 deletion, leaving the
// counter with nothing to avoid and no call sites, so it was removed.

pub struct WebBackend {
    pub(crate) doc: Document,
    pub(crate) mount: web_sys::Element,
    /// HYDRATION (prototype): when `true`, `create_*` adopts the
    /// pre-rendered SSR DOM node at [`hydration_cursor`] instead of
    /// creating a fresh element — so the booting bundle reuses the
    /// server's DOM (and its layout) and just wires handlers/reactivity
    /// onto it. The cursor walks the SSR tree in pre-order, matching the
    /// walker's pre-order `create_*` calls. Turned off in `finish` once
    /// the initial adoption pass completes (later reactive rebuilds
    /// create fresh nodes normally).
    #[cfg(feature = "hydrate")]
    pub(crate) hydrating: bool,
    /// Next SSR node to adopt (pre-order). `None` once exhausted.
    #[cfg(feature = "hydrate")]
    pub(crate) hydration_cursor: Option<web_sys::Node>,
    /// SUBTREE-LOCAL REMOUNT: when the walker's node doesn't match the
    /// SSR node at the cursor, we don't fail the whole hydration — we
    /// build *that one subtree* fresh, replace the stale SSR node in
    /// place, and resume adopting its siblings. These four fields track
    /// the single in-flight remount (only the OUTERMOST mismatch needs
    /// tracking — everything nested under it is fresh via `suppress`):
    ///
    /// `suppress` — inside the fresh remount subtree; `create_*` builds
    /// fresh and `hydrate_next` doesn't touch the cursor.
    #[cfg(feature = "hydrate")]
    pub(crate) hydration_suppress: bool,
    /// The last `hydrate_next` mismatched; the next fresh node a
    /// `create_*` makes IS the remount root (recorded via
    /// [`Self::hydrate_note_fresh`]).
    #[cfg(feature = "hydrate")]
    pub(crate) hydration_pending_fresh: bool,
    /// The tag the walker asked for on the mismatching `hydrate_next` —
    /// carried into [`Self::hydrate_note_fresh`]'s diagnostic so the
    /// console warning says WHAT the client wanted, not just what the
    /// server had.
    #[cfg(feature = "hydrate")]
    pub(crate) hydration_pending_tag: Option<String>,
    /// The fresh subtree root being built; when the walker `insert`s it,
    /// the remount completes (replace the stale node, resume cursor).
    #[cfg(feature = "hydrate")]
    pub(crate) hydration_remount_root: Option<web_sys::Node>,
    /// The stale SSR node the remount root replaces (removed on resync).
    #[cfg(feature = "hydrate")]
    pub(crate) hydration_remount_stale: Option<web_sys::Node>,
    /// Cursor to restore once the remount subtree completes (the stale
    /// node's next sibling — so the remounted node's siblings adopt).
    #[cfg(feature = "hydrate")]
    pub(crate) hydration_remount_resume: Option<web_sys::Node>,
    /// NAVIGATOR cursor steering (`hydrate_nav_screen_begin`/`_end`):
    /// LIFO stack of saved cursors, one frame per in-flight navigator
    /// initial-screen build. `(true, cursor)` = steering active, restore
    /// on end; `(false, None)` = the begin ran suppressed/unmatched and
    /// end must pop without touching the cursor.
    #[cfg(feature = "hydrate")]
    pub(crate) hydration_nav_saved: Vec<(bool, Option<web_sys::Node>)>,
    /// Outlet nodes whose SSR subtree was already consumed by a steered
    /// screen build. When the author-layout build later adopts one of
    /// these, the cursor must skip its subtree instead of descending
    /// into it (the children belong to the screen, already adopted).
    #[cfg(feature = "hydrate")]
    pub(crate) hydration_consumed_outlets: Vec<web_sys::Node>,
    pub(crate) _click_closures: Vec<Closure<dyn FnMut()>>,
    /// Keyboard handlers for `Element::Pressable` (Enter/Space →
    /// click). Held so JS doesn't drop them while the element is in
    /// the layout tree. The click handler itself lives in
    /// `_click_closures` (shared shape: `FnMut()` no-arg).
    pub(crate) _pressable_key_closures: Vec<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
    /// The single APP-LEVEL `keydown` listener installed on `document` by
    /// `set_app_key_handler` (fires regardless of focus). Held so JS keeps it
    /// alive; removing + dropping it tears the listener down.
    pub(crate) _app_key_closure: Option<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
    /// Closures attached to `<a>` elements for `Element::Link`.
    /// Held so JS doesn't drop them while the anchor is still in
    /// the layout tree. Same posture as `_click_closures`.
    pub(crate) _link_click_closures: Vec<Closure<dyn FnMut(web_sys::MouseEvent)>>,
    /// Per-node interaction-event closures. Keyed by node-id so we
    /// can drop them when `on_node_unstyled` fires. Each entry holds
    /// the listeners for one node (pointerenter, pointerleave,
    /// pointerdown, pointerup, focusin, focusout) plus the
    /// pointer-event-type closures so the JS side keeps them alive.
    pub(crate) state_listeners: FxHashMap<u32, Vec<Closure<dyn FnMut(web_sys::Event)>>>,
    /// Per-node CSS properties the LAST inline-layer application set
    /// (`apply_inline_style`). CSSOM has no "replace layer" primitive —
    /// only per-property set/remove — so without this record a property
    /// the previous layer named but the current one doesn't (a cleared
    /// `with_inline`) lingered on the node forever.
    pub(crate) inline_props: FxHashMap<u32, Vec<String>>,
    /// Has the `@keyframes ui-spin` rule been injected? First
    /// ActivityIndicator creation injects it; later creations skip
    /// the work.
    pub(crate) spinner_keyframes_injected: bool,
    /// Has the virtualizer JS shim been injected? First Virtualizer
    /// creation injects `runtime/js/virtualizer.js` into a
    /// `<script>` tag in the document head.
    pub(crate) virtualizer_shim_injected: bool,
    pub(crate) virtual_grid_shim_injected: bool,
    /// Has the local-render batch executor (`runtime/js/batch.js`)
    /// been injected? First batched `Element::Repeat` triggers
    /// injection, subsequent calls reuse the cached
    /// `window.__idealystExecuteBatch` function.
    pub(crate) batch_shim_injected: bool,
    /// Cached handle to `window.__idealystExecuteBatch` after the
    /// shim is injected. Avoids a per-batch `Reflect::get` lookup
    /// off `window` — the function reference is stable for the
    /// page's lifetime.
    pub(crate) batch_fn: Option<js_sys::Function>,
    /// Has the batched-text-update shim
    /// (`runtime/js/text_batch.js`) been injected? Mirrors
    /// `batch_shim_injected` for the reactive-text fast path.
    pub(crate) text_batch_shim_injected: bool,
    /// Has the JS-side reactive-binding shim
    /// (`runtime/js/text_bindings.js`) been injected? Companion
    /// to `text_batch_shim_injected` — the binding shim shares
    /// the text-id space with the batched-text shim, so any node
    /// that owns a batched-text id can ALSO carry a JS-side
    /// binding without conflict. Lazy: flipped true on first
    /// `ensure_text_bindings_shim()` call.
    pub(crate) text_bindings_shim_injected: bool,
    /// Cached handle to `window.__idealystRegisterText`. Set on first
    /// `create_text_with_id` call.
    pub(crate) text_register_fn: Option<js_sys::Function>,
    /// Cached handle to `window.__idealystOnSignalChanged`. Set
    /// the first time a JS-registered signal fires.
    pub(crate) signal_changed_fn: Option<js_sys::Function>,
    /// Cached handle to `window.__idealystRegisterBinding`. Set on
    /// first call to `register_reactive_text_binding`.
    pub(crate) binding_register_fn: Option<js_sys::Function>,
    /// Cached handle to `window.__idealystReleaseBinding`. Set on
    /// first call to `release_reactive_text_binding`.
    pub(crate) binding_release_fn: Option<js_sys::Function>,
    /// Monotonically-assigned text id counter. NEVER reused — a stale
    /// `update_text_by_id` queued before a release but flushed after
    /// would otherwise race against a re-assigned slot.
    pub(crate) next_text_id: u32,
    /// Per-microtask buffer of `(text_id, new_content)` updates,
    /// flushed via one FFI call to `__idealystUpdateTextBatch`.
    /// Same shared `StringBatchQueue` infrastructure the class-batch
    /// path uses — the only thing that differs is the JS function
    /// name (`__idealystUpdateTextBatch` vs `__idealystApplyClassesBatch`).
    pub(crate) text_queue: crate::batch_queue::StringBatchQueue,
    /// Pending text-registry releases. Flushed via one FFI call to
    /// `__idealystReleaseTextBatch` ahead of the update batch.
    pub(crate) text_release_batch: crate::batch_queue::IdBatch,

    // ---------------------------------------------------------
    // Batched class-attribute updates. The style apply paths
    // queue `(node_id, class_name)` pairs here and schedule a
    // microtask flush; the flush ships a single FFI call to
    // `__idealystApplyClassesBatch` and the JS shim does the
    // per-element `setAttribute` in pure JS. Each unique styled
    // node pays ONE FFI hop in its lifetime (registration on
    // first apply); subsequent updates cost only their share of
    // a batch flush.
    // ---------------------------------------------------------
    /// Has `runtime/js/class_batch.js` been injected? First apply
    /// triggers the injection; subsequent applies reuse the cached
    /// function handles.
    pub(crate) class_batch_shim_injected: bool,
    /// Cached `window.__idealystRegisterStyledNode`. Looked up once
    /// after first registration.
    pub(crate) class_register_fn: Option<js_sys::Function>,
    /// Set of node ids the JS side has been told about. We register
    /// each styled node ONCE on its first apply (1 FFI hop /
    /// node-lifetime); subsequent applies hit the batched path.
    pub(crate) class_nodes_registered: FxHashSet<u32>,
    /// Per-microtask buffer of (node_id, class_name) updates.
    /// Flushed via one FFI call to `__idealystApplyClassesBatch`.
    /// All bookkeeping (lengths, scheduling, FFI shipping) lives in
    /// the shared `StringBatchQueue` type — every batched surface
    /// (text, class, future attribute, …) owns one of these.
    pub(crate) class_queue: crate::batch_queue::StringBatchQueue,
    /// Pending styled-node releases. Flushed via one FFI call to
    /// `__idealystReleaseStyledNodesBatch` (collapses N per-id
    /// calls to one — material at switch-arm teardown of 10k+
    /// rows).
    pub(crate) class_release_batch: crate::batch_queue::IdBatch,

    // ---------------------------------------------------------
    // JS-side reactive class bindings (`StyleSource::SignalClass`).
    // Pre-resolves a value→class table at mount; signal writes
    // fan out entirely in JS via the shared signal-changed
    // dispatcher in `text_bindings.js`. Eliminates per-row Rust
    // Effect dispatch for SHARED cohorts at hierarchy scale.
    // ---------------------------------------------------------
    /// Has `runtime/js/class_bindings.js` been injected?
    pub(crate) class_bindings_shim_injected: bool,
    /// Has `runtime/js/node_ids.js` been injected? Hosts the
    /// `WeakMap<Node, u32>` that backs [`WebBackend::node_id`].
    pub(crate) node_id_shim_injected: bool,
    /// Cached `window.__idealystNodeId` after first lookup —
    /// subsequent `node_id` cache misses skip the `Reflect::get` round-trip.
    pub(crate) node_id_fn: Option<js_sys::Function>,
    /// Cached `window.__idealystRegisterClassBinding`.
    pub(crate) class_binding_register_fn: Option<js_sys::Function>,
    /// Pending class-binding releases. Flushed via one FFI call to
    /// `__idealystReleaseClassBindingsBatch`. Shares the same
    /// `IdBatch` infrastructure the styled-node release path uses.
    pub(crate) class_binding_release_batch: crate::batch_queue::IdBatch,
    /// Monotonic id counter for active class bindings.
    pub(crate) next_class_binding_id: u32,
    /// Per-virtualizer instance state — keyed by node id so we can
    /// route `virtualizer_data_changed` to the right instance AND
    /// drop its closures on `release_virtualizer`. The wrapped
    /// `VirtualizerInstance` owns the wasm-bindgen `Closure`s
    /// handed to the JS shim; dropping it destroys them via
    /// `__wbindgen_destroy_closure`, which is what prevents
    /// queued-but-not-yet-fired JS callbacks from reaching a
    /// freed-Signal arena slot after the surrounding scope has
    /// dropped.
    pub(crate) virtualizer_instances: FxHashMap<u32, primitives::virtualizer::VirtualizerInstance>,
    pub(crate) virtual_grid_instances: FxHashMap<u32, primitives::virtual_grid::VirtualGridInstance>,
    /// Monotonic id counter for virtualizer containers, written as
    /// `data-virtualizer-id` on the container `<div>`. Same trick as
    /// `data-graphics-id`: lets `release_virtualizer` look up the
    /// instance from a `&Node` without going through `node_ids`,
    /// which gets cleared by `on_node_unstyled` before our cleanup
    /// hook runs (style effects drop before the virtualizer cleanup
    /// effect within a single `Scope::drop` batch).
    pub(crate) next_virtualizer_id: u32,
    pub(crate) next_virtual_grid_id: u32,
    /// Per-Graphics-canvas runtime state — wgpu device, user closures,
    /// pending-paint flag, etc. Keyed by node id so `make_handle` can
    /// look up the matching instance after `create`. The `Rc` is the
    /// shared owner; the handle wraps the same Rc so `request_redraw`
    /// reaches the scheduler with no backend round-trip.
    pub(crate) graphics_instances:
        FxHashMap<u32, std::rc::Rc<std::cell::RefCell<primitives::graphics::GraphicsInstance>>>,
    /// Monotonic id counter for Graphics canvases. Written as the
    /// `data-graphics-id` attribute on each `<canvas>` so
    /// `make_handle` / `release` can look the instance up from a
    /// fresh `&Node` after the create call returned. Distinct from
    /// per-Node ids (those live in a JS-side `WeakMap` keyed by
    /// DOM identity; see [`WebBackend::node_id`]).
    pub(crate) next_graphics_id: u32,
    /// Shared `<style>` element holding every active CSS rule.
    pub(crate) style_element: Option<web_sys::HtmlStyleElement>,
    /// Pre-generated classes from `register_stylesheet`. Content-keyed,
    /// shared, refcounted (refcount tracks how many active
    /// registrations hold them — not how many nodes apply them).
    pub(crate) pregen: FxHashMap<String, PregenEntry>,
    /// Pointer-keyed mirror of `pregen` for the hot apply path. When
    /// the framework's resolution cache returns the same
    /// `Rc<StyleRules>` instance for many nodes (e.g. 10000 rows of
    /// the same variant), we look up the class name by `Rc::as_ptr`
    /// in O(1) — without paying for `content_key()` to format a
    /// 300-byte hex string per row.
    ///
    /// Populated by `register_stylesheet` alongside the content-keyed
    /// `pregen` map. Cleared on `unregister_stylesheet` /
    /// theme change.
    pub(crate) pregen_by_ptr: FxHashMap<*const runtime_shared::StyleRules, String>,
    /// Per-node dynamic class slot — `node_id -> (class_name, content_key)`.
    /// At most one dynamic class per node. Replaced atomically when
    /// the node's resolved style changes.
    pub(crate) dynamic: FxHashMap<u32, DynamicSlot>,
    /// Content-keyed pool of dynamic CSS rules, refcounted across the
    /// cohort of nodes that resolved to the same `(base + overlays)`
    /// content. Populated lazily on `apply_styled_states` slow-path
    /// misses; collapsed when the last `DynamicSlot` referencing a
    /// key drops. The reactive-style cohort (one signal fanning out
    /// to N styled nodes) is the canonical user — pre-dedupe, every
    /// fan-out minted N identical rules + did N `insert_rule` / N
    /// `delete_rule` calls; deduped, the first node mints and the
    /// rest just bump the refcount.
    pub(crate) dynamic_by_content: FxHashMap<String, DynamicRule>,
    /// Pointer-keyed mirror of `dynamic_by_content` for the hot apply
    /// path. The framework's `RESOLUTION_CACHE` hands us the SAME
    /// `Rc<StyleRules>` for repeated `(sheet, variants, overrides)`
    /// resolutions — so a cohort of N reactive-styled rows all
    /// receive the same `Rc::as_ptr(base)`. This lets us skip
    /// `content_key()` (a ~300-byte string format) entirely on the
    /// second-and-later applies of any given resolved style.
    ///
    /// Value is `Rc<DynamicPtrEntry>` so both `dynamic_by_content`
    /// (keyed by content) and per-node `DynamicSlot`s can share the
    /// same `class_name` + `content_key` strings without per-call
    /// allocation. On a fast-path hit we just `Rc::clone` the entry
    /// (atomic refcount bump) instead of cloning two `String`s.
    ///
    /// Populated when `dynamic_by_content` gets a new entry;
    /// invalidated when the entry is removed. `*const` is safe to
    /// use as a key because: (a) we only ever compare it, never
    /// dereference; (b) the `RESOLUTION_CACHE` keeps the Rc alive
    /// for as long as its content is reachable, which is at least
    /// as long as we hold any `DynamicSlot` referencing it.
    pub(crate) dynamic_by_ptr: FxHashMap<*const runtime_shared::StyleRules, std::rc::Rc<DynamicPtrEntry>>,
    /// Indices in the shared `<style>` sheet that previously held a
    /// dynamic rule and are now available for re-use. See
    /// `insert_rule` / `delete_rule` in [`crate::style`] — instead
    /// of `deleteRule(idx)`-then-shifting-everything (O(N) per
    /// op), `delete_rule` records `idx` here and `insert_rule`
    /// recycles via an `insertRule(rule, idx)` after the matching
    /// `deleteRule(idx)`. The pair leaves all other indices
    /// unchanged, so insert+delete are both O(1) regardless of how
    /// many rules are live.
    pub(crate) free_rule_indices: Vec<u32>,
    /// CSS rule index of the `:root { --token: value; ... }` block
    /// that holds the active theme's token variables. `None` until the
    /// first `install_theme_variables` call. On theme swap we reach
    /// into the existing rule's `CSSStyleDeclaration` and `setProperty`
    /// each token in place — the rule itself is never deleted, so no
    /// other rule indices shift and no minted class re-emits.
    pub(crate) theme_root_rule_index: Option<u32>,
    /// Indices of the `html,body { background: var(--…); … }` rule
    /// (`Some(idx)` once `set_app_background` has been called). Stored
    /// so re-calls with a different token swap the rule's body in place
    /// — we DELETE + re-insert at the same index rather than
    /// `setProperty`-mutating, because the rule's `background` value
    /// is the `var(--…)` reference itself (not the resolved color),
    /// and only the reference changes when the SDK re-targets.
    pub(crate) app_bg_rule_index: Option<u32>,
    /// Indices of the scrollbar rules — one entry per
    /// `BODY_SCROLLBAR_RULE_COUNT` rule inserted by
    /// `set_scrollbar_theme`. Same delete-and-reinsert-in-place
    /// pattern as `app_bg_rule_index`.
    pub(crate) scrollbar_rule_indices: Vec<u32>,
    /// Per-portal state, keyed by the `data-portal-id` attribute
    /// stamped on the portal root. Holds the wasm-bindgen `Closure`
    /// handles wired to dismiss / reposition / focus-trap events so
    /// they stay alive while the portal is mounted; dropping the
    /// instance entry in `release_portal` is what frees the
    /// JS-side closures and prevents late-firing events from
    /// reaching a freed `Signal` slot.
    pub(crate) portal_instances: primitives::portal::PortalInstances,
    /// Monotonic id counter for portals. Same pattern as
    /// `next_navigator_id` — stamped as `data-portal-id` on the
    /// portal root.
    pub(crate) next_portal_id: u32,
    /// Asset id → resolved URL. Filled by `register_asset`; queried
    /// by `register_typeface` (for the `@font-face` `src: url(...)`)
    /// and, in a follow-up, by the `Image` primitive's `<img src>`.
    pub(crate) asset_urls: FxHashMap<AssetId, String>,
    /// Ids whose `asset_urls` entry is a `blob:` URL backed by
    /// `URL.createObjectURL` (i.e. `AssetSource::Embedded`). Used by
    /// `unregister_asset` to call `URL.revokeObjectURL` and free the
    /// Blob's backing storage. Bundled / Remote URLs are owned by
    /// the page / CDN — not in this set, never revoked.
    pub(crate) blob_asset_urls: FxHashSet<AssetId>,
    /// Typeface id → indices into the shared `<style>` sheet for the
    /// `@font-face` rules emitted at registration. Lets
    /// `unregister_typeface` reclaim the slots through the regular
    /// `delete_rule` recycle path.
    pub(crate) font_face_rule_indices: runtime_shared::collections::SmallIdMap<TypefaceId, Vec<u32>>,
    /// Registry of `Element::Navigator` handler factories,
    /// populated by `register_navigator::<P, _>(...)` calls from
    /// SDK leaf crates (e.g. `stack_navigator::register`).
    /// `create_navigator` looks the factory up by presentation
    /// TypeId; unregistered kinds panic at create time.
    /// Per-navigator-instance SDK handler. Keyed by the navigator id
    /// stamped on the container's `data-navigator-id` attribute.
    /// `Backend::create_navigator` resolves the factory, runs `init`,
    /// and stores the returned handler here so subsequent
    /// `navigator_attach_initial` / `release_navigator` /
    /// `make_navigator_handle` / `apply_navigator_slot_style` calls
    /// can route through the handler's kind-specific logic instead
    /// of through hard-coded backend machinery.
    ///
    /// `Rc<RefCell<...>>` so the trait impl methods can clone an
    /// independent handle out of the map, drop the map borrow, then
    /// call `&mut B`-taking methods on the handler without
    /// double-borrowing `self`.
    /// Per-node animated-property state. Tracks the most recent
    /// values written via `Backend::set_animated_f32` /
    /// `set_animated_color` so compound properties like CSS
    /// `translate: <x> <y>` and `scale: <x> <y>` can be re-emitted
    /// without clobbering unrelated axes. See [`animated`] module
    /// for the per-property routing.
    pub(crate) animated_states: animated::AnimatedStateMap,
    /// Identity set of framework primitive **root** DOM nodes, populated by
    /// `note_introspection_root` (called from the walker as each primitive is
    /// registered). The native-introspection walk uses object identity here
    /// to know where one primitive's DOM ends and a child primitive's begins
    /// — it prunes the tree at any descendant in this set. A `js_sys::Set`
    /// (SameValueZero identity for objects) so it needs no node-id round-trip.
    /// Populated only in robot builds (the walker calls
    /// `note_introspection_root`); a single cheap JS Set otherwise idle.
    pub(crate) introspection_roots: js_sys::Set,
}

/// Diagnostic snapshot returned by [`WebBackend::debug_counts`].
#[derive(Debug, Clone, Copy)]
pub struct WebBackendCounts {
    pub dynamic: usize,
    pub state_listeners: usize,
    pub pregen: usize,
    pub pregen_by_ptr: usize,
    pub free_rule_indices: usize,
}

pub(crate) struct PregenEntry {
    #[allow(dead_code)]
    pub(crate) name: String,
    pub(crate) rule_index: u32,
    pub(crate) refcount: u32,
}

pub(crate) struct DynamicSlot {
    /// Shared (class_name, content_key) pair, refcounted across the
    /// pointer cache, the dynamic_by_content entry, and every
    /// `DynamicSlot` referencing it. Pre-Rc, every slot held its
    /// own two `String` copies; with N reactive-styled nodes
    /// sharing the same content, that was 2N heap allocations
    /// every fan-out for no semantic gain.
    pub(crate) shared: std::rc::Rc<DynamicPtrEntry>,
}

/// Strings that live as long as a dynamic content entry — pinned
/// behind an `Rc` so the per-node slots and the pointer cache
/// don't each hold their own copies. The `refcount` lives here
/// too (interior mutability) so the hot apply path can bump it
/// in O(1) without re-hashing `content_key` to find the
/// `dynamic_by_content` slot.
pub(crate) struct DynamicPtrEntry {
    pub(crate) class_name: String,
    pub(crate) content_key: String,
    /// Number of `DynamicSlot`s currently referencing this entry's
    /// CSS rule. Bumped on every apply that resolves to this
    /// content; decremented when the slot is replaced or the node
    /// unmounts. When it hits zero, the `dynamic_by_content`
    /// entry (and the rules it owns) gets dropped.
    pub(crate) refcount: std::cell::Cell<u32>,
}

/// Refcounted dynamic CSS rule shared across the cohort of nodes
/// that resolved to the same `(base + overlays)` content. Sharing
/// avoids per-node `insert_rule` churn — at scale (one signal
/// fanning out to N reactive-styled nodes) this is the difference
/// between O(1) and O(N) rule inserts. Lifetime: created when a
/// node first resolves to this content; deleted when the last
/// node's slot stops referencing it.
pub(crate) struct DynamicRule {
    /// Shared with `dynamic_by_ptr` and every `DynamicSlot` that
    /// references this rule. Lets the apply hot path skip cloning
    /// `class_name` + `content_key` `String`s per call. Refcount
    /// lives on `shared` (interior mutability via `Cell<u32>`) so
    /// hot-path apply doesn't need to look up this map entry at
    /// all — it just bumps `shared.refcount`.
    pub(crate) shared: std::rc::Rc<DynamicPtrEntry>,
    /// CSS rule index for the base rule. Always set.
    pub(crate) rule_index: u32,
    /// Additional rule indices for per-state overlays
    /// (`.cls:hover`, `:active`, `:focus`, `[disabled]`). Empty for
    /// nodes without `state` blocks.
    pub(crate) state_rule_indices: Vec<u32>,
}



impl WebBackend {
    /// Constructs a backend that will mount its root under `mount_selector`
    /// (e.g. `"#app"`). Panics if the element is not found.
    /// Boot in HYDRATION mode against a server-rendered mount: instead
    /// of clearing `#app` and rebuilding, the backend ADOPTS the existing
    /// SSR DOM — `create_*` returns the matching pre-rendered node (walked
    /// in pre-order) and just wires handlers/reactivity onto it. The
    /// browser keeps the server's already-laid-out DOM (no flash, no
    /// rebuild). On a tag mismatch (server/client render divergence) it
    /// disables adoption and `finish` falls back to a clean rebuild.
    ///
    /// PREREQUISITE for a clean adoption: the first client render must
    /// match the server render. The viewport is the main divergence — seed
    /// `runtime_shared::set_viewport_size(...)` with the SSR-assumed viewport
    /// (see `data-ssr-viewport` / [`ssr_viewport`](crate::ssr_viewport))
    /// BEFORE `mount`, then `install_viewport_observer()` AFTER so the real
    /// viewport drives a reactive update post-adoption.
    #[cfg(feature = "hydrate")]
    pub fn hydrate(mount_selector: &str) -> Self {
        let mut b = Self::new(mount_selector);
        b.hydrating = true;
        // First element child of the mount = the SSR root the walker's
        // first `create_*` will adopt.
        b.hydration_cursor = b.mount.first_element_child().map(|e| e.unchecked_into());
        // Buffer microtasks during the build so `mount` drains the nav's
        // deferred chrome/screen builds inside the adoption window.
        crate::scheduler::begin_hydration_buffering();
        b
    }

    /// During hydration, adopt the SSR navigator container: the element at
    /// the cursor if it carries `class` (e.g. `"ui-nav-root"`). Returns it
    /// (leaving the cursor on it); the navigator adopts its frame via
    /// [`hydrate_adopt_child`] + re-enters regions via [`hydrate_enter`].
    /// `None` when not hydrating or the cursor doesn't match.
    #[cfg(feature = "hydrate")]
    pub fn hydrate_adopt_container(&mut self, class: &str) -> Option<web_sys::Node> {
        if !self.hydrating {
            return None;
        }
        let cur = self.hydration_cursor.clone()?;
        let el = cur.dyn_ref::<web_sys::Element>()?;
        if !element_has_class(el, class) {
            return None;
        }
        Some(cur)
    }

    /// During hydration, adopt the server-rendered child of `parent`
    /// carrying `class` (match-by-class, parent-relative). METHOD form —
    /// for callers that hold `&mut WebBackend` synchronously (e.g. the
    /// navigator frame build runs *inside* `create_navigator`'s
    /// `borrow_mut`, so the global-handle free fn's `try_borrow` would
    /// fail there). `None` when not hydrating or no match.
    #[cfg(feature = "hydrate")]
    pub fn hydrate_adopt_child_of(
        &self,
        parent: &web_sys::Node,
        class: &str,
    ) -> Option<web_sys::Node> {
        if !self.hydrating {
            return None;
        }
        let parent_el = parent.dyn_ref::<web_sys::Element>()?;
        let mut child = parent_el.first_element_child();
        while let Some(c) = child {
            if element_has_class(&c, class) {
                return Some(c.unchecked_into());
            }
            child = c.next_element_sibling();
        }
        None
    }

    /// NAVIGATOR cursor steering, part 1 (see
    /// `runtime_vocabulary::caps::LifecycleOps::hydrate_nav_screen_begin`
    /// for the contract): the navigator realizes its initial screen
    /// BEFORE the layout builds the outlet, but the SSR document nests
    /// the screen INSIDE the outlet. Save the cursor, move it to the
    /// server-rendered screen position (first element child of the
    /// outlet stamped `data-iy-nav-outlet="<base>"` under `root`), and
    /// remember the outlet so its later adoption skips the consumed
    /// subtree. When the marker is missing (document from an SSR build
    /// predating it), the cursor is parked instead — the screen builds
    /// fresh and `show_in_outlet`'s clear-and-insert swaps it in for the
    /// server's copy, while the chrome around it still adopts.
    #[cfg(feature = "hydrate")]
    pub fn hydrate_nav_screen_begin_impl(&mut self, root: &Node, base: &str) {
        if !self.hydrating {
            return;
        }
        // Mid-remount (`suppress`): this navigator is inside a subtree
        // already building fresh — don't disturb the parked remount
        // cursor; push an inactive frame so `end` pops symmetrically.
        if self.hydration_suppress {
            self.hydration_nav_saved.push((false, None));
            return;
        }
        let attr = runtime_shared::primitives::navigator::NAV_OUTLET_HYDRATION_ATTR;
        let outlet = root
            .dyn_ref::<web_sys::Element>()
            .and_then(|el| el.query_selector(&format!("[{attr}=\"{base}\"]")).ok().flatten());
        match outlet {
            Some(outlet) => {
                self.hydration_nav_saved.push((true, self.hydration_cursor.take()));
                self.hydration_cursor =
                    outlet.first_element_child().map(|e| e.unchecked_into::<web_sys::Node>());
                self.hydration_consumed_outlets.push(outlet.unchecked_into());
            }
            None => {
                // No marker — park the cursor for the screen build
                // (fresh-build fallback), restore it for the layout.
                self.hydration_nav_saved.push((true, self.hydration_cursor.take()));
            }
        }
    }

    /// NAVIGATOR cursor steering, part 2: restore the cursor saved by
    /// the matching [`Self::hydrate_nav_screen_begin_impl`] so the
    /// author-layout build adopts from the navigator's first layout
    /// node. Any mismatch state armed inside the screen walk is cleared
    /// with it — the screen subtree is done; a pending flag must not
    /// leak into the layout build's first create.
    #[cfg(feature = "hydrate")]
    pub fn hydrate_nav_screen_end_impl(&mut self) {
        if !self.hydrating {
            return;
        }
        if let Some((active, saved)) = self.hydration_nav_saved.pop() {
            if active && !self.hydration_suppress {
                self.hydration_cursor = saved;
                self.hydration_pending_fresh = false;
                self.hydration_pending_tag = None;
            }
        }
    }

    /// During hydration, suspend the cursor so the next `create_*` build
    /// fresh without adopting/arming a remount. METHOD form for the
    /// synchronous in-`borrow_mut` caller (end of the navigator frame
    /// build, before the walker's throwaway initial screen).
    #[cfg(feature = "hydrate")]
    pub fn hydrate_suspend_cursor(&mut self) {
        if !self.hydrating {
            return;
        }
        self.hydration_cursor = None;
        self.hydration_suppress = false;
        self.hydration_pending_fresh = false;
    }

    /// During hydration, descend the cursor into the first element child of
    /// `region`. METHOD form of the free [`hydrate_enter`] fn — for callers
    /// holding `&mut WebBackend` synchronously (e.g. inside `create_navigator`'s
    /// `borrow_mut`, where the global-handle free fn's `try_borrow` would fail
    /// and silently no-op). A no-layout stack/tab navigator uses this right
    /// after adopting its container so the SYNCHRONOUS walker `attach_initial`
    /// screen build adopts the screen's root node — not the container itself.
    #[cfg(feature = "hydrate")]
    pub fn hydrate_enter_region(&mut self, region: &web_sys::Node) {
        if !self.hydrating {
            return;
        }
        let first = region
            .dyn_ref::<web_sys::Element>()
            .and_then(|el| el.first_element_child());
        self.hydration_cursor = first.map(|e| e.unchecked_into::<web_sys::Node>());
        self.hydration_suppress = false;
        self.hydration_pending_fresh = false;
    }

    /// During hydration, return the next SSR node to adopt if its tag
    /// matches `tag` (advancing the cursor into its children); otherwise
    /// `None` (the caller creates a fresh element).
    ///
    /// On a TAG MISMATCH it does NOT advance or fail — it leaves the
    /// cursor parked on the stale node and flags `pending_fresh`, so the
    /// caller's freshly-created node is captured by
    /// [`Self::hydrate_note_fresh`] as a subtree-local remount root.
    /// Inside a remount subtree (`suppress`), it always returns `None`.
    #[cfg(feature = "hydrate")]
    pub(crate) fn hydrate_next(&mut self, tag: &str) -> Option<web_sys::Element> {
        if !self.hydrating || self.hydration_suppress {
            return None;
        }
        let cur = self.hydration_cursor.clone()?;
        let el: web_sys::Element = cur.dyn_into().ok()?;
        if el.tag_name().eq_ignore_ascii_case(tag) {
            // A steered screen build already consumed this outlet's
            // subtree (`hydrate_nav_screen_begin`) — adopt the outlet
            // itself but jump PAST its children, which belong to the
            // screen, not to whatever the layout build creates next.
            let consumed = {
                let pos = self
                    .hydration_consumed_outlets
                    .iter()
                    .position(|n| n.is_same_node(Some(el.as_ref())));
                pos.map(|i| self.hydration_consumed_outlets.swap_remove(i)).is_some()
            };
            self.hydration_cursor = if consumed {
                Self::next_preorder_skip_subtree(el.as_ref(), &self.mount)
            } else {
                Self::next_preorder(&el, &self.mount)
            };
            Some(el)
        } else {
            // Mismatch — leave the cursor on the stale node; the next
            // fresh node the caller builds becomes the remount root.
            self.hydration_pending_fresh = true;
            self.hydration_pending_tag = Some(tag.to_string());
            None
        }
    }

    /// Like [`hydrate_next`] but on adoption skips the matched node's
    /// subtree instead of descending into its children. For primitives
    /// whose contents (icon `<path>`s, etc.) are built internally and
    /// NOT walked by the framework — without this, the cursor would
    /// land on a child of the adopted node and the next walker step
    /// would mismatch against it.
    #[cfg(feature = "hydrate")]
    pub(crate) fn hydrate_next_skip_subtree(
        &mut self,
        tag: &str,
    ) -> Option<web_sys::Element> {
        if !self.hydrating || self.hydration_suppress {
            return None;
        }
        let cur = self.hydration_cursor.clone()?;
        let el: web_sys::Element = cur.dyn_into().ok()?;
        if el.tag_name().eq_ignore_ascii_case(tag) {
            self.hydration_cursor = Self::next_preorder_skip_subtree(&el, &self.mount);
            Some(el)
        } else {
            self.hydration_pending_fresh = true;
            self.hydration_pending_tag = Some(tag.to_string());
            None
        }
    }

    /// Called by every `create_*` right after it builds a FRESH node.
    /// If a mismatch is pending, this `fresh` node is the root of a
    /// subtree-local remount: record what it replaces (the stale SSR
    /// node at the cursor) and where to resume adopting (the stale
    /// node's next sibling), and enter `suppress` so the rest of this
    /// subtree builds fresh. Cheap no-op otherwise.
    #[cfg(feature = "hydrate")]
    pub(crate) fn hydrate_note_fresh(&mut self, fresh: &web_sys::Node) {
        if !self.hydration_pending_fresh {
            return;
        }
        self.hydration_pending_fresh = false;
        let wanted = self.hydration_pending_tag.take();
        let Some(stale) = self.hydration_cursor.clone() else { return };

        // Diagnostics: which BRANCH is being remounted.
        if let Some(se) = stale.dyn_ref::<web_sys::Element>() {
            let here: String = se.outer_html().chars().take(140).collect();
            let mut chain = Vec::new();
            let mut p = se.parent_element();
            while let Some(pe) = p {
                if pe.is_same_node(Some(self.mount.as_ref())) {
                    break;
                }
                let cls = pe.class_name();
                chain.push(if cls.is_empty() {
                    format!("<{}>", pe.tag_name().to_lowercase())
                } else {
                    format!("<{} .{}>", pe.tag_name().to_lowercase(), cls.split(' ').next().unwrap_or(""))
                });
                p = pe.parent_element();
            }
            chain.reverse();
            web_sys::console::warn_1(
                &format!(
                    "[hydrate] SSR/client diverge — remounting just this subtree (siblings still \
                     adopt).\n  client wanted: <{}>\n  branch: {}\n  stale SSR node: {}",
                    wanted.as_deref().unwrap_or("?"),
                    chain.join(" > "),
                    here
                )
                .into(),
            );
        }

        self.hydration_remount_resume = Self::next_preorder_skip_subtree(&stale, &self.mount);
        self.hydration_remount_root = Some(fresh.clone());
        self.hydration_remount_stale = Some(stale);
        self.hydration_suppress = true;
    }

    /// Snapshot the SSR adoption cursor before an `Element::External`
    /// handler runs. `None` off the hydrate path. Paired with
    /// [`Self::hydrate_external_note_if_unadopted`].
    #[cfg(feature = "hydrate")]
    pub(crate) fn hydrate_cursor_snapshot(&self) -> Option<web_sys::Node> {
        if self.hydrating {
            self.hydration_cursor.clone()
        } else {
            None
        }
    }

    /// Called right after an external handler builds its node. If the cursor
    /// is unchanged from `before`, the handler did NOT adopt the SSR host at
    /// the cursor (it built fresh — e.g. the GPU canvas), so arm a subtree
    /// remount: the fresh `node` becomes the remount root that
    /// [`Self::hydrate_resync_remount`] swaps in for the stale SSR host when
    /// the walker parents it. This keeps the cursor aligned for the external's
    /// siblings and detaches the orphaned host. A hydration-aware handler
    /// (one that calls `hydrate_next`) advances the cursor, so `before !=`
    /// cursor and this no-ops.
    #[cfg(feature = "hydrate")]
    pub(crate) fn hydrate_external_note_if_unadopted(
        &mut self,
        before: &Option<web_sys::Node>,
        node: &web_sys::Node,
    ) {
        if !self.hydrating || self.hydration_suppress {
            return;
        }
        let unchanged = matches!(
            (before, &self.hydration_cursor),
            (Some(a), Some(b)) if a.is_same_node(Some(b))
        );
        if unchanged {
            // The SSR host at the cursor was never consumed → treat the fresh
            // external node as the replacement for it.
            self.hydration_pending_fresh = true;
            self.hydration_pending_tag = Some("external".to_string());
            self.hydrate_note_fresh(node);
        }
    }

    /// Resync a subtree-local hydration remount when `child` is the fresh
    /// remount root recorded by [`Self::hydrate_note_fresh`]: swap `child`
    /// in for the stale SSR node it replaces (in place, so siblings keep
    /// their DOM order), restore the adoption cursor to the stale node's
    /// next sibling, and exit `suppress`. Returns `true` when it handled
    /// `child` — the caller must then NOT insert it again.
    ///
    /// Shared by [`Backend::insert`], [`Backend::insert_at`], and
    /// [`Backend::insert_many`] so a remount root gets its stale SSR
    /// subtree detached no matter which attach path the walker parents it
    /// through. This matters because the anchorless `when` / `switch`
    /// splice (`build_when_spliced` / the `Each` reconciler) parents the
    /// branch via `insert_at`, and the `Repeat` fallback via `insert_many`
    /// — neither of which used to run the resync. A remount root parented
    /// by `insert_at` therefore left the stale SSR node in the DOM: the
    /// duplicated absolutely-positioned nav this method was added to fix.
    #[cfg(feature = "hydrate")]
    fn hydrate_resync_remount(&mut self, parent: &mut web_sys::Node, child: &web_sys::Node) -> bool {
        if !self
            .hydration_remount_root
            .as_ref()
            .map(|r| r.is_same_node(Some(child)))
            .unwrap_or(false)
        {
            return false;
        }
        if let Some(stale) = self.hydration_remount_stale.take() {
            if let Some(sp) = stale.parent_node() {
                let _ = sp.replace_child(child, &stale);
            } else {
                let _ = parent.append_child(child);
            }
        }
        self.hydration_cursor = self.hydration_remount_resume.take();
        self.hydration_remount_root = None;
        self.hydration_suppress = false;
        true
    }

    /// During hydration, whether `child` is an already-adopted SSR node
    /// that is ALREADY parented to `parent`. Adopted nodes are live in the
    /// SSR DOM in build order, so re-inserting (or repositioning) one is a
    /// wasteful — and, for `insert_at`, a reordering — no-op. Callers skip
    /// the backend insert when this holds (outside a remount `suppress`
    /// subtree, where the child is genuinely fresh).
    #[cfg(feature = "hydrate")]
    fn hydrate_child_already_adopted(&self, parent: &web_sys::Node, child: &web_sys::Node) -> bool {
        child
            .parent_node()
            .map(|p| p.is_same_node(Some(parent)))
            .unwrap_or(false)
    }

    // ---------------------------------------------------------------
    // No-hydrate stubs. Public surface stays callable from SDK crates +
    // generated wrappers; bodies optimize to a const `None` / no-op
    // and DCE drops the cursor/diagnostic machinery from the bundle.
    // `WebBackend::hydrate(...)` falls back to `new(...)` — the v1
    // clear-and-rebuild path in `finish()` then runs (flicker on
    // bundle boot, but no broken DOM).
    // ---------------------------------------------------------------

    #[cfg(not(feature = "hydrate"))]
    pub fn hydrate(mount_selector: &str) -> Self {
        Self::new(mount_selector)
    }
    #[cfg(not(feature = "hydrate"))]
    pub fn hydrate_adopt_container(&mut self, _class: &str) -> Option<web_sys::Node> {
        None
    }
    #[cfg(not(feature = "hydrate"))]
    pub fn hydrate_adopt_child_of(
        &self,
        _parent: &web_sys::Node,
        _class: &str,
    ) -> Option<web_sys::Node> {
        None
    }
    #[cfg(not(feature = "hydrate"))]
    pub fn hydrate_suspend_cursor(&mut self) {}
    #[cfg(not(feature = "hydrate"))]
    pub fn hydrate_enter_region(&mut self, _region: &web_sys::Node) {}
    #[cfg(not(feature = "hydrate"))]
    pub(crate) fn hydrate_next(&mut self, _tag: &str) -> Option<web_sys::Element> {
        None
    }
    #[cfg(not(feature = "hydrate"))]
    pub(crate) fn hydrate_next_skip_subtree(
        &mut self,
        _tag: &str,
    ) -> Option<web_sys::Element> {
        None
    }
    #[cfg(not(feature = "hydrate"))]
    pub(crate) fn hydrate_note_fresh(&mut self, _fresh: &web_sys::Node) {}
    #[cfg(not(feature = "hydrate"))]
    pub(crate) fn hydrate_cursor_snapshot(&self) -> Option<web_sys::Node> {
        None
    }
    #[cfg(not(feature = "hydrate"))]
    pub(crate) fn hydrate_external_note_if_unadopted(
        &mut self,
        _before: &Option<web_sys::Node>,
        _node: &web_sys::Node,
    ) {
    }

    /// Next node in a pre-order DFS of the SSR tree, bounded by `mount`.
    /// Descends into children first. Matches the walker's pre-order
    /// `create_*` order.
    #[cfg(feature = "hydrate")]
    fn next_preorder(node: &web_sys::Node, mount: &web_sys::Element) -> Option<web_sys::Node> {
        let el = node.dyn_ref::<web_sys::Element>()?;
        if let Some(child) = el.first_element_child() {
            return Some(child.unchecked_into());
        }
        Self::next_preorder_skip_subtree(node, mount)
    }

    /// Pre-order successor that SKIPS `node`'s subtree (its next sibling,
    /// else climb). Used to resume after a remounted subtree.
    #[cfg(feature = "hydrate")]
    fn next_preorder_skip_subtree(
        node: &web_sys::Node,
        mount: &web_sys::Element,
    ) -> Option<web_sys::Node> {
        let mut cur: web_sys::Element = node.dyn_ref::<web_sys::Element>()?.clone();
        loop {
            if let Some(sib) = cur.next_element_sibling() {
                return Some(sib.unchecked_into());
            }
            let parent = cur.parent_element()?;
            if parent.is_same_node(Some(mount.as_ref())) {
                return None;
            }
            cur = parent;
        }
    }

    pub fn new(mount_selector: &str) -> Self {
        let window = web_sys::window().expect("no window");
        let doc = window.document().expect("no document");
        let mount = doc
            .query_selector(mount_selector)
            .expect("query failed")
            .expect("mount element not found");
        Self::new_in(mount)
    }

    /// Boot against an explicit mount element instead of a document
    /// selector. Equivalent to [`new`](Self::new) but skips the
    /// document-scoped `querySelector` — which can't reach a shadow
    /// root or an element that foreign code owns.
    ///
    /// This is the basis for the "external export" Web Component
    /// bridge: each custom-element instance mounts its own Idealyst
    /// tree into its own host node, so multiple independent trees can
    /// coexist on a page the framework doesn't own.
    pub fn new_in(mount: web_sys::Element) -> Self {
        let window = web_sys::window().expect("no window");
        let doc = window.document().expect("no document");
        let mut backend = Self {
            doc,
            mount,
            #[cfg(feature = "hydrate")]
            hydrating: false,
            #[cfg(feature = "hydrate")]
            hydration_cursor: None,
            #[cfg(feature = "hydrate")]
            hydration_suppress: false,
            #[cfg(feature = "hydrate")]
            hydration_pending_fresh: false,
            #[cfg(feature = "hydrate")]
            hydration_pending_tag: None,
            #[cfg(feature = "hydrate")]
            hydration_remount_root: None,
            #[cfg(feature = "hydrate")]
            hydration_remount_stale: None,
            #[cfg(feature = "hydrate")]
            hydration_remount_resume: None,
            #[cfg(feature = "hydrate")]
            hydration_nav_saved: Vec::new(),
            #[cfg(feature = "hydrate")]
            hydration_consumed_outlets: Vec::new(),
            _click_closures: Vec::new(),
            _pressable_key_closures: Vec::new(),
            _app_key_closure: None,
            _link_click_closures: Vec::new(),
            state_listeners: FxHashMap::default(),
            inline_props: FxHashMap::default(),
            spinner_keyframes_injected: false,
            virtualizer_shim_injected: false,
            virtual_grid_shim_injected: false,
            batch_shim_injected: false,
            batch_fn: None,
            text_batch_shim_injected: false,
            text_bindings_shim_injected: false,
            text_register_fn: None,
            signal_changed_fn: None,
            binding_register_fn: None,
            binding_release_fn: None,
            next_text_id: 0,
            text_queue: crate::batch_queue::StringBatchQueue::new(
                "__idealystUpdateTextBatch",
            ),
            text_release_batch: crate::batch_queue::IdBatch::new(
                "__idealystReleaseTextBatch",
            ),
            class_batch_shim_injected: false,
            class_register_fn: None,
            class_nodes_registered: FxHashSet::default(),
            class_queue: crate::batch_queue::StringBatchQueue::new(
                "__idealystApplyClassesBatch",
            ),
            class_release_batch: crate::batch_queue::IdBatch::new(
                "__idealystReleaseStyledNodesBatch",
            ),
            class_bindings_shim_injected: false,
            node_id_shim_injected: false,
            node_id_fn: None,
            class_binding_register_fn: None,
            class_binding_release_batch: crate::batch_queue::IdBatch::new(
                "__idealystReleaseClassBindingsBatch",
            ),
            next_class_binding_id: 0,
            virtualizer_instances: FxHashMap::default(),
            virtual_grid_instances: FxHashMap::default(),
            next_virtualizer_id: 0,
            next_virtual_grid_id: 0,
            graphics_instances: FxHashMap::default(),
            next_graphics_id: 0,
            style_element: None,
            pregen: FxHashMap::default(),
            pregen_by_ptr: FxHashMap::default(),
            dynamic: FxHashMap::default(),
            dynamic_by_content: FxHashMap::default(),
            dynamic_by_ptr: FxHashMap::default(),
            free_rule_indices: Vec::new(),
            theme_root_rule_index: None,
            app_bg_rule_index: None,
            scrollbar_rule_indices: Vec::new(),
            portal_instances: FxHashMap::default(),
            next_portal_id: 0,
            asset_urls: FxHashMap::default(),
            blob_asset_urls: FxHashSet::default(),
            font_face_rule_indices: runtime_shared::collections::SmallIdMap::new(),
            animated_states: FxHashMap::default(),
            introspection_roots: js_sys::Set::new(&wasm_bindgen::JsValue::UNDEFINED),
        };
        backend
    }

    /// Register a signal with the JS-side reactive layer so its
    /// future writes ship to JS for fan-out. Call once per signal
    /// — subsequent calls overwrite the previous stringifier
    /// (which is fine; the closure captures the same `Signal<T>`
    /// handle every time).
    ///
    /// `stringifier` runs from inside `Signal::set` / `Signal::update`
    /// after the Rust subscriber fan-out and must produce a `String`
    /// representation of the signal's current value (typically
    /// `signal.get_untracked().to_string()`). The result is shipped
    /// across the wasm→JS boundary via
    /// `__idealystOnSignalChanged(sid, value)`, where the JS-side
    /// binding registry handles the per-binding fan-out.
    ///
    /// Caller must have installed the text batcher first (see
    /// [`install_text_batcher`]) so the JS shim and self-handle
    /// are both available.
    pub fn register_signal_for_js<F>(&mut self, sid_raw: u64, stringifier: F)
    where
        F: Fn() -> String + 'static,
    {
        // Ensure the binding shim is loaded so
        // `__idealystOnSignalChanged` is callable from the closure.
        self.ensure_text_bindings_shim();
        // Capture a Weak self-handle so the notifier closure can
        // find its way back to `&mut self` when the signal fires,
        // without creating a cyclic Rc that would leak the backend
        // forever.
        let weak = WEB_BACKEND_HANDLE
            .with(|s| s.borrow().clone())
            .expect(
                "WEB_BACKEND_HANDLE must be set (call install_text_batcher first) \
                 to use register_signal_for_js",
            );
        let stringifier = std::rc::Rc::new(stringifier);
        runtime_shared::register_signal_js_notifier(sid_raw, move || {
            let value = stringifier();
            if let Some(rc) = weak.upgrade() {
                rc.borrow_mut().ship_signal_change_to_js(sid_raw, &value);
            }
        });
    }

    /// Ship a `(signal_id, new_value)` notification to the JS-side
    /// reactive layer. Single FFI hop — JS handles the per-binding
    /// fan-out internally. Called from the notifier closure
    /// installed by [`Self::register_signal_for_js`], and (pub(crate))
    /// from the new-core `notify_signal_value_js` capability override
    /// in `newcore.rs` — world signals have no `Signal::set` JS hook,
    /// so a vocabulary notifier effect delivers commits instead.
    pub(crate) fn ship_signal_change_to_js(&mut self, sid_raw: u64, value: &str) {
        use wasm_bindgen::JsValue;
        if self.signal_changed_fn.is_none() {
            let window = web_sys::window().expect("no window");
            let f_val = js_sys::Reflect::get(
                &window,
                &JsValue::from_str("__idealystOnSignalChanged"),
            )
            .expect("Reflect::get for __idealystOnSignalChanged failed");
            self.signal_changed_fn = Some(
                f_val
                    .dyn_into::<js_sys::Function>()
                    .expect("__idealystOnSignalChanged is not a Function — shim missing"),
            );
        }
        let _ = self
            .signal_changed_fn
            .as_ref()
            .expect("set above")
            .call2(
                &JsValue::NULL,
                // u32 fits the typical SignalId.0; we send as f64
                // because JS treats Numbers as f64. The JS-side
                // Map<sid, ...> uses these as keys.
                &JsValue::from(sid_raw as u32),
                &JsValue::from_str(value),
            )
            .expect("__idealystOnSignalChanged call failed");
    }

    /// Register a reactive text binding with the JS-side layer.
    /// After this call, the text node at `text_id` updates entirely
    /// from JS whenever any signal in `signal_ids` fires — no Rust
    /// Effect, no per-leaf wasm crossing on fan-out.
    ///
    /// - `text_id`         : the id returned by
    ///                       [`Backend::create_text_with_id`](runtime_shared::Backend::create_text_with_id).
    /// - `signal_ids`      : signal raw ids (`Signal::id()`) the
    ///                       binding interpolates, in template-slot
    ///                       order.
    /// - `template_parts`  : the N+1 static parts surrounding the
    ///                       N signal slots (e.g. for `"leaf {}: g={}"`
    ///                       pass `["leaf ", ": g=", ""]`).
    /// - `initial_values`  : the N initial signal values as strings
    ///                       (typically `signal.get_untracked().to_string()`).
    ///                       Used both to seed the JS-side signal
    ///                       cache AND to compute the binding's
    ///                       initial `nodeValue` synchronously
    ///                       inside this call (no empty-text flash).
    ///
    /// Each signal in `signal_ids` should have a JS-side notifier
    /// installed by the time this returns; the
    /// [`runtime_shared::Backend::register_reactive_text_binding`]
    /// trait method that wraps this passes `stringifiers` and we
    /// auto-register a notifier per signal here (only if one isn't
    /// already installed — preserves notifiers a class-binding may
    /// have set up on the same signal first).
    pub fn register_reactive_text_binding(
        &mut self,
        text_id: u32,
        signal_ids: &[u64],
        template_parts: &[&str],
        initial_values: &[&str],
        stringifiers: &[std::rc::Rc<dyn Fn() -> String>],
    ) {
        use wasm_bindgen::JsValue;
        debug_assert_eq!(
            template_parts.len(),
            signal_ids.len() + 1,
            "template_parts must have N+1 entries for N signal slots",
        );
        debug_assert_eq!(
            initial_values.len(),
            signal_ids.len(),
            "initial_values must have one entry per signal id",
        );
        debug_assert_eq!(
            stringifiers.len(),
            signal_ids.len(),
            "stringifiers must have one entry per signal id",
        );
        self.ensure_text_bindings_shim();

        // Auto-install per-signal JS notifiers so writes to any
        // bound signal flow through `__idealystOnSignalChanged` and
        // the JS-side text dispatcher repaints the node. Skip
        // signals that already have a notifier — a class binding
        // (or an earlier text binding) may have set one up, and
        // overwriting would stomp THEIR teardown path. The existing
        // notifier still calls `__idealystOnSignalChanged`, which
        // the text dispatcher taps regardless of who installed it.
        for (sid, stringifier) in signal_ids.iter().zip(stringifiers.iter()) {
            if !runtime_shared::signal_has_js_notifier(*sid) {
                let stringifier = stringifier.clone();
                self.register_signal_for_js(*sid, move || stringifier());
            }
        }
        if self.binding_register_fn.is_none() {
            let window = web_sys::window().expect("no window");
            let f_val = js_sys::Reflect::get(
                &window,
                &JsValue::from_str("__idealystRegisterBinding"),
            )
            .expect("Reflect::get for __idealystRegisterBinding failed");
            self.binding_register_fn = Some(
                f_val
                    .dyn_into::<js_sys::Function>()
                    .expect("__idealystRegisterBinding is not a Function — shim missing"),
            );
        }

        // Get the Text DOM node out of the registry by id. We
        // stored Text nodes (not their wrapping spans) at create
        // time exactly so the binding can write to `nodeValue`
        // directly.
        let text_node: JsValue = {
            let window = web_sys::window().expect("no window");
            let registry = js_sys::Reflect::get(
                &window,
                &JsValue::from_str("__idealystTextRegistry"),
            )
            .expect("Reflect::get for __idealystTextRegistry failed");
            js_sys::Reflect::get_u32(&registry, text_id)
                .expect("text id not in __idealystTextRegistry — was create_text_with_id called?")
        };

        // Encode signal_ids as Uint32Array (single FFI marshal),
        // parts + initials as NUL-joined strings (single FFI each).
        let ids_u32: Vec<u32> = signal_ids.iter().map(|&s| s as u32).collect();
        let ids_buf = js_sys::Uint32Array::from(&ids_u32[..]);
        let parts_joined = template_parts.join("\0");
        let initials_joined = initial_values.join("\0");

        let _ = self
            .binding_register_fn
            .as_ref()
            .expect("set above")
            .apply(
                &JsValue::NULL,
                &js_sys::Array::of5(
                    &JsValue::from(text_id),
                    &text_node,
                    &ids_buf,
                    &JsValue::from_str(&parts_joined),
                    &JsValue::from_str(&initials_joined),
                ),
            )
            .expect("__idealystRegisterBinding call failed");
    }

    /// Release a JS-side binding previously registered via
    /// [`Self::register_reactive_text_binding`]. The text node
    /// itself is released separately via the existing
    /// `release_text_id` path; this only clears the binding
    /// metadata (signal subscriptions) on the JS side.
    pub fn release_reactive_text_binding(&mut self, text_id: u32) {
        use wasm_bindgen::JsValue;
        if self.binding_release_fn.is_none() {
            let window = web_sys::window().expect("no window");
            let f_val = js_sys::Reflect::get(
                &window,
                &JsValue::from_str("__idealystReleaseBinding"),
            )
            .expect("Reflect::get for __idealystReleaseBinding failed");
            self.binding_release_fn = Some(
                f_val
                    .dyn_into::<js_sys::Function>()
                    .expect("__idealystReleaseBinding is not a Function — shim missing"),
            );
        }
        let _ = self
            .binding_release_fn
            .as_ref()
            .expect("set above")
            .call1(&JsValue::NULL, &JsValue::from(text_id))
            .expect("__idealystReleaseBinding call failed");
    }

    /// Register a JS-side reactive class binding. Pre-resolves at
    /// mount; signal writes fan out from the JS dispatcher to
    /// every node subscribed on the same signal.
    ///
    /// Mechanics:
    ///   1. Ensure the styled-node registry has an entry for the
    ///      node (the binding dispatcher reads node handles from
    ///      `__idealystStyledNodes`).
    ///   2. Install a signal-changed notifier so writes flow to the
    ///      `__idealystOnSignalChanged` dispatcher (which our
    ///      class_bindings.js shim taps into).
    ///   3. Ship the (binding_id, node_id, signal_id, values,
    ///      classes) table to JS in one FFI call.
    pub fn register_reactive_class_binding(
        &mut self,
        node: &Node,
        signal_id: u64,
        values: &[u32],
        classes: &[&str],
        value_reader: std::rc::Rc<dyn Fn() -> u32>,
    ) -> u32 {
        use wasm_bindgen::JsValue;

        self.ensure_class_bindings_shim();

        // The dispatcher looks up the node from `__idealystStyledNodes`
        // by id. Register if this is the first time we're touching
        // this node — same pattern the class-batch apply path uses,
        // and shares the same registry, so a node that's both
        // class-batched AND signal-bound only registers once.
        let node_id = self.node_id(node);
        if !self.class_nodes_registered.contains(&node_id) {
            self.register_styled_node(node, node_id);
            self.class_nodes_registered.insert(node_id);
        }

        // Install a signal-changed notifier. The framework's
        // `register_signal_js_notifier` allows at most one
        // notifier per signal (the second registration overwrites
        // the first), so if the user has also wired this signal
        // for text bindings via `register_signal_for_js`, that
        // notifier wins. Class bindings still work in that case
        // because the existing text-binding stringifier also
        // ships `__idealystOnSignalChanged`, which our class
        // dispatcher taps. The bare-class-binding case (no text
        // binding on this signal) is what this branch covers.
        //
        // We register unconditionally — if a previous binding for
        // a different node also called this, the closure shape is
        // identical, so overwriting is safe.
        let weak = WEB_BACKEND_HANDLE
            .with(|s| s.borrow().clone())
            .expect("WEB_BACKEND_HANDLE must be set when class-binding path is active");
        let reader = value_reader.clone();
        runtime_shared::register_signal_js_notifier(signal_id, move || {
            let value = reader();
            if let Some(rc) = weak.upgrade() {
                rc.borrow_mut()
                    .ship_signal_change_to_js(signal_id, &value.to_string());
            }
        });

        // Lazily resolve the JS-side register fn.
        if self.class_binding_register_fn.is_none() {
            let window = web_sys::window().expect("no window");
            let f_val = js_sys::Reflect::get(
                &window,
                &JsValue::from_str("__idealystRegisterClassBinding"),
            )
            .expect("Reflect::get for __idealystRegisterClassBinding failed");
            self.class_binding_register_fn = Some(
                f_val
                    .dyn_into::<js_sys::Function>()
                    .expect(
                        "__idealystRegisterClassBinding is not a Function — \
                         class_bindings.js shim missing",
                    ),
            );
        }

        let binding_id = self.next_class_binding_id;
        self.next_class_binding_id += 1;

        // Encode the args. `values` ships as Uint32Array; `classes`
        // as one big length-prefixed string buffer + Uint32Array
        // of lengths (same wire shape as the class-apply batch).
        let values_buf = js_sys::Uint32Array::from(values);
        let mut classes_joined = String::with_capacity(64);
        let mut lengths: Vec<u32> = Vec::with_capacity(classes.len());
        for cls in classes {
            classes_joined.push_str(cls);
            let utf16_len: u32 = if cls.is_ascii() {
                cls.len() as u32
            } else {
                cls.chars().map(|c| c.len_utf16() as u32).sum()
            };
            lengths.push(utf16_len);
        }
        let lengths_buf = js_sys::Uint32Array::from(&lengths[..]);

        // Pack the four small u32 args (binding_id, node_id, sig_lo,
        // sig_hi) into a 4-element header Uint32Array so the final
        // `apply` call has 4 args total — within `Array::of4`'s
        // single-FFI-hop reach. The alternative would be
        // `Array::new()` + 7 individual `push` calls (one FFI each)
        // which defeats the batching point.
        let sig_lo = (signal_id & 0xFFFF_FFFF) as u32;
        let sig_hi = (signal_id >> 32) as u32;
        let header = js_sys::Uint32Array::from(&[binding_id, node_id, sig_lo, sig_hi][..]);

        let _ = self
            .class_binding_register_fn
            .as_ref()
            .expect("set above")
            .apply(
                &JsValue::NULL,
                &js_sys::Array::of4(
                    &header,
                    &values_buf,
                    &JsValue::from_str(&classes_joined),
                    &lengths_buf,
                ),
            )
            .expect("__idealystRegisterClassBinding call failed");

        binding_id
    }

    /// Release a JS-side class binding. Pushes to the batched
    /// release queue (same `IdBatch` infrastructure the styled-node
    /// release path uses) so N releases on a switch-arm teardown
    /// ship in one FFI call.
    pub fn release_reactive_class_binding(&mut self, binding_id: u32) {
        self.class_binding_release_batch.push(binding_id);
        // Piggy-back on the class flush microtask — the same
        // `schedule_class_flush` mechanism that drains the apply +
        // styled-node-release queues also drains this one.
        self.schedule_class_flush();
    }

    /// Drain queued `update_text_by_id` and `release_text_id` calls
    /// into a single FFI hop via `__idealystUpdateTextBatch`.
    ///
    /// Called from the microtask scheduled by
    /// [`WebBackend::update_text_by_id`]. The bench's `apply`
    /// timer measures synchronous JS + the immediately-following
    /// microtask drain, so this work still counts against `apply`
    /// — but the per-leaf FFI cost collapses from one
    /// `set_text_content` round-trip per leaf to one
    /// `Uint32Array`-shaped flush per fan-out.
    pub(crate) fn flush_pending_text(&mut self) {
        let _t_total = crate::phase_timer::PhaseTimer::start("text_flush_total");
        // Releases first — scope teardown can drop 2k+ text effects
        // at once. Both releases and updates ride a single FFI call
        // each via the shared `IdBatch` / `StringBatchQueue` helpers.
        self.text_release_batch.flush();
        self.text_queue.flush();
    }

    /// Append a `(id, content)` pending entry to the text-update
    /// queue. Bytes are written directly into the shared buffer via
    /// `write_fn` so callers using `format!`-style construction
    /// don't allocate an intermediate `String`.
    pub(crate) fn append_pending_text<F: FnOnce(&mut String)>(
        &mut self,
        id: u32,
        write_fn: F,
    ) {
        self.text_queue.queue_with(id, write_fn);
    }

    /// Schedule a microtask-driven flush of pending text updates.
    /// Idempotent within a turn — concurrent queues coalesce.
    fn schedule_text_flush(&self) {
        if self.text_queue.mark_scheduled() {
            return;
        }
        let weak = WEB_BACKEND_HANDLE
            .with(|s| s.borrow().clone())
            .expect("WEB_BACKEND_HANDLE must be set when batched text path is active");
        let flag = self.text_queue.flush_flag();
        runtime_shared::schedule_microtask(move || {
            flag.set(false);
            if let Some(rc) = weak.upgrade() {
                rc.borrow_mut().flush_pending_text();
            }
        });
    }

    /// Diagnostic: snapshot of all the per-node HashMaps the backend
    /// owns. Used by the arena bench to detect when a rebuild loop
    /// leaves stale entries behind. Each field is a `usize` count of
    /// live entries; `free_rule_indices` shows how many CSS-rule
    /// slots are recycled (waiting to be reused) — large values
    /// indicate a previously-grown sheet that hasn't been compacted.
    pub fn debug_counts(&self) -> WebBackendCounts {
        WebBackendCounts {
            dynamic: self.dynamic.len(),
            state_listeners: self.state_listeners.len(),
            pregen: self.pregen.len(),
            pregen_by_ptr: self.pregen_by_ptr.len(),
            free_rule_indices: self.free_rule_indices.len(),
        }
    }

    /// Queue a `class` attribute update for `node` to be flushed at
    /// the next microtask. Replaces the direct `set_attribute("class",
    /// …)` call on the style apply hot path — saves one wasm→JS
    /// boundary crossing per update at fan-out (~60 ms at N=100k
    /// shared-signal subscribers).
    ///
    /// If the JS-side shim handle isn't installed (e.g. variant
    /// forgot to call `install_text_batcher`), this falls back to a
    /// direct `set_attribute` so correctness never depends on the
    /// fast path being wired up.
    pub(crate) fn queue_class_apply(&mut self, node: &Node, class_name: &str) {
        let _t = crate::phase_timer::PhaseTimer::start("queue_class_apply");
        // Fall back to direct setAttribute if the shim batcher hasn't
        // been installed by the host crate. Keeps test backends + ad-hoc
        // usage correct (just slower).
        let has_handle = WEB_BACKEND_HANDLE.with(|s| s.borrow().is_some());
        if !has_handle {
            if let Some(element) = node.dyn_ref::<web_sys::Element>() {
                let _ = element.set_attribute("class", class_name);
            }
            return;
        }
        self.ensure_class_batch_shim();

        let id = self.node_id(node);
        if !self.class_nodes_registered.contains(&id) {
            // FIRST apply for this node: register it with the JS-side
            // styled-node map (so later batched updates address it by
            // id) AND set the class SYNCHRONOUSLY rather than queuing
            // it for the microtask flush.
            //
            // The build walker styles a node BEFORE inserting it into
            // its parent (`walker/view.rs`: `build(...)` then
            // `insert(...)`), so the node is still DETACHED here — a
            // synchronous `setAttribute` can't trigger a visible reflow,
            // and it guarantees the node carries its themed class on its
            // FIRST style resolution once it's attached.
            //
            // Deferring this first class to the batch microtask was the
            // boot/navigation FOUC: the node attached and got its first
            // style resolution class-less (border-color resolves to
            // `currentColor`/black, background to transparent), so the
            // class's `transition` then animated from that unstyled
            // state to the themed value on the first painted frame. CSS
            // suppresses transitions on an element's first style
            // computation — but only when that first computation already
            // carries the final class. Setting it synchronously restores
            // that invariant. The value written is byte-identical to what
            // the batch flush would set (same `class_name`, same full
            // `setAttribute('class', …)` replace the JS shim does), so
            // this is purely a timing change.
            //
            // Subsequent applies (theme re-style, reactive/signal
            // fan-out) still ride the batched queue below — that's where
            // the FFI-coalescing win matters (one signal → N nodes),
            // and those nodes are already painted, so a value change
            // there SHOULD transition (e.g. the dark-mode swap).
            self.register_styled_node(node, id);
            self.class_nodes_registered.insert(id);
            if let Some(element) = node.dyn_ref::<web_sys::Element>() {
                let _ = element.set_attribute("class", class_name);
            }
            return;
        }

        self.class_queue.queue(id, class_name);
        self.schedule_class_flush();
    }

    /// One-time registration of `node` with the JS-side
    /// `__idealystStyledNodes` map so subsequent batched applies
    /// can address it by id alone.
    fn register_styled_node(&mut self, node: &Node, id: u32) {
        if self.class_register_fn.is_none() {
            let window = web_sys::window().expect("no window");
            let f_val = js_sys::Reflect::get(
                &window,
                &wasm_bindgen::JsValue::from_str("__idealystRegisterStyledNode"),
            )
            .expect("Reflect::get for __idealystRegisterStyledNode failed");
            self.class_register_fn = Some(
                f_val.dyn_into::<js_sys::Function>().expect(
                    "__idealystRegisterStyledNode is not a Function — class_batch shim missing",
                ),
            );
        }
        let _ = self
            .class_register_fn
            .as_ref()
            .expect("set above")
            .call2(
                &wasm_bindgen::JsValue::NULL,
                &wasm_bindgen::JsValue::from(id),
                node.as_ref(),
            )
            .expect("__idealystRegisterStyledNode call failed");
    }

    /// Schedule a microtask flush. Idempotent within a turn — if
    /// one's already scheduled, subsequent queues just append to
    /// the buffer.
    fn schedule_class_flush(&self) {
        if self.class_queue.mark_scheduled() {
            return;
        }
        let weak = WEB_BACKEND_HANDLE
            .with(|s| s.borrow().clone())
            .expect("WEB_BACKEND_HANDLE must be set when batched class path is active");
        let flag = self.class_queue.flush_flag();
        runtime_shared::schedule_microtask(move || {
            // Clear the flag BEFORE flushing so updates produced
            // during the flush (re-entrant signal writes) re-schedule
            // a fresh microtask rather than being dropped.
            flag.set(false);
            if let Some(rc) = weak.upgrade() {
                rc.borrow_mut().flush_pending_classes();
            }
        });
    }

    /// Drain pending releases + apply-batch. Both sub-flushes are
    /// idempotent on empty queues.
    pub(crate) fn flush_pending_classes(&mut self) {
        let _t_total = crate::phase_timer::PhaseTimer::start("class_flush_total");
        // Releases first — collapses N per-id FFI calls to one via
        // the shared `IdBatch` helper. Apply-batch follows.
        self.class_release_batch.flush();
        // Drain pending SignalClass binding releases too. Same
        // microtask serves all three queues so a switch-arm
        // teardown produces at most one FFI call per kind.
        self.class_binding_release_batch.flush();
        self.class_queue.flush();
    }

    /// Drop a node from the JS-side styled-node registry. Called by
    /// `impl_on_node_unstyled` so released elements can be GC'd
    /// instead of being pinned by the shim's `Map`.
    pub(crate) fn release_styled_node(&mut self, id: u32) {
        if self.class_nodes_registered.remove(&id) {
            self.class_release_batch.push(id);
            // If a flush isn't already scheduled, schedule one so
            // the release reaches JS at the next microtask.
            self.schedule_class_flush();
        }
    }

    /// Assigns a stable per-Node id we use as a key in `dynamic`,
    /// `state_listeners`, `animated_states`, and friends.
    ///
    /// Identity is keyed by the underlying JS object — multiple
    /// Rust `web_sys::Node` wrappers around the same DOM element
    /// always resolve to the same id. That's necessary because the
    /// framework freely constructs fresh wrappers (e.g. when
    /// filling a `Ref<ViewHandle>`'s `Rc<dyn Any>`), and the
    /// `*const Node` Rust pointer has no relationship to the
    /// underlying JS object — different wrappers around the same
    /// DOM element have different pointer values, and the Rust
    /// allocator readily reuses freed wrapper addresses for
    /// unrelated wrappers.
    ///
    /// Implementation:
    ///
    /// - **Every call goes through the JS-side `WeakMap<Node, u32>`**
    ///   (see `runtime/js/node_ids.js`). Same JS object always →
    ///   same id. The WeakMap auto-clears entries when DOM
    ///   elements are GC'd, so no explicit registry teardown is
    ///   needed.
    /// - **No Rust-side cache.** An earlier pointer-keyed
    ///   `FxHashMap<*const Node, u32>` fast cache had a stale-id
    ///   bug: a freed wrapper's address could be reused by a
    ///   completely unrelated wrapper, and the cache would
    ///   return the prior wrapper's id for the new wrapper —
    ///   leaking per-node state across DOM elements. That cache
    ///   has been removed; correctness wins over the FFI savings.
    /// - **Optional debug**: with the `debug-node-ids` feature
    ///   on, mirror the id outward as a `data-idealyst-id="N"`
    ///   attribute on Elements so it shows up in devtools / e2e
    ///   selectors. Off by default — production builds don't
    ///   pollute the DOM.
    ///
    /// Cost: one FFI hop per call. `node_id` is invoked from
    /// every `apply_style` / `apply_styled_states` /
    /// `set_animated_*` / `on_node_unstyled`. If this ever
    /// becomes a measurable bottleneck, a safer cache scheme
    /// — e.g. keying by `Rc<Node>` for paths that own one —
    /// would restore the fast path without the correctness hole.
    /// Not measured yet; see `tests/web_perf.rs` for the
    /// benchmark when it lands.
    pub(crate) fn node_id(&mut self, node: &Node) -> u32 {
        // No Rust-side pointer cache: the framework regularly
        // constructs fresh `web_sys::Node` wrappers around the same
        // DOM element (e.g. when filling a `Ref<ViewHandle>`'s
        // `Rc<dyn Any>`), and the wrapper's heap address has no
        // relationship to the underlying JS object. The Rust
        // allocator readily reuses freed wrapper addresses for
        // unrelated wrappers, so a pointer-keyed cache returns a
        // stale id the moment a fresh wrapper recycles an address
        // we've cached before — the exact bug that left only one
        // band's gradient animating on the welcome example. We
        // always go through the JS-side `WeakMap` (one FFI per
        // call) and trust *that* as the source of truth.
        self.ensure_node_id_shim();
        if self.node_id_fn.is_none() {
            let window = web_sys::window().expect("no window");
            let f_val =
                js_sys::Reflect::get(&window, &wasm_bindgen::JsValue::from_str("__idealystNodeId"))
                    .expect("Reflect::get for __idealystNodeId failed");
            self.node_id_fn = Some(
                f_val
                    .dyn_into::<js_sys::Function>()
                    .expect("__idealystNodeId is not a Function — shim injection failed"),
            );
        }
        let f = self.node_id_fn.as_ref().expect("set above");
        let id_val = f
            .call1(&wasm_bindgen::JsValue::NULL, node.as_ref())
            .expect("__idealystNodeId call failed");
        let id = id_val
            .as_f64()
            .expect("__idealystNodeId must return a number") as u32;

        // Optional dev-aid: mirror the id onto the Element as a
        // `data-idealyst-id` attribute so devtools / e2e selectors
        // can see it. Compiled out in production.
        #[cfg(feature = "debug-node-ids")]
        if let Some(elem) = node.dyn_ref::<web_sys::Element>() {
            let _ = elem.set_attribute("data-idealyst-id", &id.to_string());
        }

        id
    }

    /// Shared body for `execute_batch` and `execute_batch_with_attach`.
    /// When `attach` is `Some((parent, locals))`, the shim parents
    /// `nodes[local]` to `parent` for each `local` in `locals` —
    /// folding what would otherwise be an `insert_many` follow-up
    /// call into the same FFI round-trip. Measured ~60 ms savings
    /// at 100 k rows.
    ///
    /// The flat-buffer encoding (4 u32s per op, NUL-separated string
    /// table) is the same in both modes; only the JS shim's argument
    /// list differs (3 args vs 5).
    pub(crate) fn execute_batch_inner(
        &mut self,
        batch: runtime_shared::BackendBatch,
        attach: Option<(&mut web_sys::Node, &[u32])>,
    ) -> Vec<web_sys::Node> {
        use js_sys::Array;
        use wasm_bindgen::JsCast;
        use wasm_bindgen::JsValue;

        let _t_total = crate::phase_timer::PhaseTimer::start("execute_batch_total");

        if batch.node_count == 0 {
            return Vec::new();
        }

        // First call: inject the shim and cache the function handle.
        self.ensure_batch_shim();
        if self.batch_fn.is_none() {
            let window = web_sys::window().expect("no window");
            let f_val = js_sys::Reflect::get(&window, &JsValue::from_str("__idealystExecuteBatch"))
                .expect("Reflect::get for __idealystExecuteBatch failed");
            let f = f_val
                .dyn_into::<js_sys::Function>()
                .expect("__idealystExecuteBatch is not a Function — shim injection failed");
            self.batch_fn = Some(f);
        }

        // Flat-buffer encoding. Each op is exactly 4 u32s:
        //
        //   [kind, arg0, arg1, arg2]
        //
        //   CreateView         [0, local_id, 0, 0]
        //   CreateText         [1, local_id, 0, string_idx]
        //   ApplyStyleStatic   [2, node_id,  0, string_idx]
        //   Insert             [3, parent,   child, 0]
        //
        // String payloads (CreateText content, ApplyStyleStatic class
        // name) are concatenated with a NUL separator and shipped as
        // a single `JsValue::from_str` — JS splits once. Our content
        // strings ("Row #N", CSS class names) never contain NUL.
        let _t_encode = crate::phase_timer::PhaseTimer::start("execute_batch_encode");
        let mut u32s: Vec<u32> = Vec::with_capacity(batch.ops.len() * 4);
        let mut strings: String = String::with_capacity(batch.ops.len() * 16);
        let mut string_count: u32 = 0;
        for op in batch.ops.iter() {
            match op {
                runtime_shared::BatchOp::CreateView { local_id } => {
                    u32s.extend_from_slice(&[0, *local_id, 0, 0]);
                }
                runtime_shared::BatchOp::CreateText { local_id, content } => {
                    if string_count > 0 {
                        strings.push('\0');
                    }
                    strings.push_str(content);
                    u32s.extend_from_slice(&[1, *local_id, 0, string_count]);
                    string_count += 1;
                }
                runtime_shared::BatchOp::ApplyStyleStatic {
                    node,
                    class_name,
                    rules: _,
                } => {
                    if string_count > 0 {
                        strings.push('\0');
                    }
                    strings.push_str(class_name);
                    u32s.extend_from_slice(&[2, *node, 0, string_count]);
                    string_count += 1;
                }
                runtime_shared::BatchOp::Insert { parent, child } => {
                    u32s.extend_from_slice(&[3, *parent, *child, 0]);
                }
            }
        }
        let u32_buf = js_sys::Uint32Array::from(&u32s[..]);
        let strings_buf = JsValue::from_str(&strings);
        drop(_t_encode);

        let _t_ffi = crate::phase_timer::PhaseTimer::start("execute_batch_ffi_call");
        let f = self.batch_fn.as_ref().expect("batch_fn set above");
        let node_count_val = JsValue::from(batch.node_count);
        let result = match attach {
            None => f
                .call3(&JsValue::NULL, &u32_buf, &strings_buf, &node_count_val)
                .expect("__idealystExecuteBatch call failed"),
            Some((parent, locals)) => {
                // Two extra args: the parent Node (one JsValue
                // crosses the boundary) and a Uint32Array of
                // `local_id`s to attach (one buffer crosses,
                // regardless of length). The JS shim does N
                // `appendChild` calls inside its own loop without
                // re-entering wasm.
                let locals_buf = js_sys::Uint32Array::from(locals);
                let args = Array::of5(
                    &u32_buf,
                    &strings_buf,
                    &node_count_val,
                    parent.as_ref(),
                    &locals_buf,
                );
                f.apply(&JsValue::NULL, &args)
                    .expect("__idealystExecuteBatch call (with attach) failed")
            }
        };
        drop(_t_ffi);

        let _t_decode = crate::phase_timer::PhaseTimer::start("execute_batch_decode");
        let nodes_array = result
            .dyn_into::<Array>()
            .expect("__idealystExecuteBatch must return an Array");

        let mut nodes = Vec::with_capacity(batch.node_count as usize);
        for i in 0..batch.node_count {
            let val = nodes_array.get(i);
            let node = val
                .dyn_into::<web_sys::Node>()
                .expect("execute_batch return-array entry must be a Node");
            nodes.push(node);
        }
        nodes
    }
}

// ---------------------------------------------------------------------------
// Backend trait impl. Each method delegates to the matching primitive
// module (or to one of the style/defaults helpers on `WebBackend`).
// Keep this thin — anything substantial belongs in the primitive's file.
// ---------------------------------------------------------------------------

// The backend mechanism, as inherent methods (runtime v2: the `Backend`
// mega-trait is gone). Bodies are verbatim what the trait impl carried;
// `newcore.rs` adapts them onto `runtime_scene::Host` + the
// `runtime_vocabulary::caps::*Ops` capability traits, one delegation per
// method. `_impl` suffix keeps the adapter's call sites unambiguous.
impl WebBackend {

    pub(crate) fn platform_impl(&self) -> runtime_shared::Platform {
        runtime_shared::Platform::Web
    }

    pub(crate) fn attach_html_class_impl(&self, node: &Node, class: &str) {
        // `classList.add` (not `className =`) so a preminted/structural
        // class composes with classes the style engine or hydration
        // already stamped. Idempotent on hydration re-adoption.
        if let Some(el) = node.dyn_ref::<web_sys::Element>() {
            let _ = el.class_list().add_1(class);
        }
    }

    pub(crate) fn detach_html_class_impl(&self, node: &Node, class: &str) {
        // `classList.remove` — the inverse of `attach_html_class_impl`, so
        // a reactive preminted style can swap its axis class without
        // touching the classes hydration or the style engine stamped.
        // Removing an absent class is a no-op in the DOM, so the first
        // run of a `PremintedDynamic` effect (nothing stamped yet) is safe.
        if let Some(el) = node.dyn_ref::<web_sys::Element>() {
            let _ = el.class_list().remove_1(class);
        }
    }

    pub(crate) fn supports_preminted_styles_impl(&self) -> bool {
        true
    }

    pub(crate) fn apply_default_text_font_impl(&mut self, font: Option<&runtime_shared::FontFamily>) {
        // Inline custom property on `<html>` — wins over any stylesheet
        // `:root` definition and needs no rule bookkeeping. Preminted
        // rule bodies read it via `var(--iy-default-font, inherit)`.
        let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        else {
            return;
        };
        let style = root.style();
        match font {
            Some(ff) => {
                let _ = style
                    .set_property(css::DEFAULT_TEXT_FONT_VAR, &css::font_family_css_value(ff));
                // …and a real, INHERITABLE `font-family`. The variable
                // alone only reaches PREMINTED rule bodies (which read
                // `var(--iy-default-font, inherit)`); nothing on the live
                // path reads it. Static applications get the theme font
                // folded into their own rules by `fill_default_text_font`,
                // but reactive ones deliberately skip that fold — so with
                // the variable alone they received no `font-family` at all
                // and inherited, hitting the browser's serif fallback when
                // no ancestor set one. Declaring it here supplies them by
                // inheritance without changing any minted class hash
                // (which is why the dynamic path must not fold it).
                let _ = style.set_property(
                    "font-family",
                    &format!("var({})", css::DEFAULT_TEXT_FONT_VAR),
                );
            }
            None => {
                let _ = style.remove_property(css::DEFAULT_TEXT_FONT_VAR);
                let _ = style.remove_property("font-family");
            }
        }
    }

    // Native render introspection (parity testing) — reads the browser's
    // resolved `getComputedStyle`/`getBoundingClientRect`. Available whenever
    // the robot bridge can call it (no extra feature); only the introspect
    // phase-timer inside auto-stubs out without `debug-stats`. See
    // `introspect.rs`.
    pub(crate) fn supports_native_introspection_impl(&self) -> bool {
        true
    }

    pub(crate) fn url_opener_impl(&self) -> Option<std::rc::Rc<dyn Fn(&str)>> {
        // `_blank` opens a new tab. Without a target the navigation
        // replaces the current document, which unmounts the framework
        // — `open_url` is for *leaving* to an external page, so a new
        // tab is the right default (in-app navigation goes through the
        // `Link` primitive, which stays single-page).
        Some(std::rc::Rc::new(|url: &str| {
            if let Some(win) = web_sys::window() {
                let _ = win.open_with_url_and_target(url, "_blank");
            }
        }))
    }

    pub(crate) fn fullscreen_setter_impl(&self) -> Option<std::rc::Rc<dyn Fn(bool)>> {
        // Best-effort Fullscreen API. `requestFullscreen` MUST be called
        // from a user-gesture event handler or the browser rejects it
        // (the returned Promise rejects) — a `set_fullscreen(true)` fired
        // outside one is silently ignored by the UA. `exit_fullscreen`
        // has no such restriction. We fire-and-forget either way, matching
        // the no-success-signal posture of `open_url`.
        Some(std::rc::Rc::new(|enabled: bool| {
            let Some(win) = web_sys::window() else { return };
            let Some(doc) = win.document() else { return };
            if enabled {
                if let Some(el) = doc.document_element() {
                    let _ = el.request_fullscreen();
                }
            } else {
                doc.exit_fullscreen();
            }
        }))
    }

    pub(crate) fn color_scheme_impl(&self) -> runtime_shared::ColorScheme {
        let window = match self.doc.default_view() {
            Some(w) => w,
            None => return runtime_shared::ColorScheme::Auto,
        };
        let prefers_dark = window
            .match_media("(prefers-color-scheme: dark)")
            .ok()
            .flatten()
            .map(|mql| mql.matches())
            .unwrap_or(false);
        let prefers_light = window
            .match_media("(prefers-color-scheme: light)")
            .ok()
            .flatten()
            .map(|mql| mql.matches())
            .unwrap_or(false);
        if prefers_dark {
            runtime_shared::ColorScheme::Dark
        } else if prefers_light {
            runtime_shared::ColorScheme::Light
        } else {
            runtime_shared::ColorScheme::Auto
        }
    }

    pub(crate) fn create_view_impl(
        &mut self,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::view::create(self);
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn create_element_impl(&mut self, tag: &str) -> Node {
        // Cursor-aware: during hydration, adopt the matching SSR element
        // (so an External handler built through the Backend reuses the
        // server's DOM rather than bypassing it via raw `web_sys`).
        if let Some(el) = self.hydrate_next(tag) {
            return el.unchecked_into::<web_sys::Node>();
        }
        let node: web_sys::Node = self
            .doc
            .create_element(tag)
            .expect("create_element failed")
            .unchecked_into();
        self.hydrate_note_fresh(&node);
        node
    }

    pub(crate) fn is_hydrating_impl(&self) -> bool {
        #[cfg(feature = "hydrate")]
        {
            self.hydrating
        }
        #[cfg(not(feature = "hydrate"))]
        {
            false
        }
    }

    /// Set the HTML `id` attribute on the underlying element. Used
    /// by `Element::Lazy`'s web handler to give the placeholder
    /// container a stable id the chunk's `mount_chunk` can root its
    /// own `WebBackend` against.
    pub(crate) fn attach_html_id_impl(&self, node: &Node, id: &str) {
        use wasm_bindgen::JsCast;
        if let Some(el) = node.dyn_ref::<web_sys::Element>() {
            let _ = el.set_attribute("id", id);
        }
    }

    pub(crate) fn attach_html_style_impl(&self, node: &Node, prop: &str, value: &str) {
        use wasm_bindgen::JsCast;
        // `set_property` handles CSS custom properties (`--drawer-width`)
        // and normal declarations alike, and merges into the element's
        // existing inline style rather than clobbering it (the walker's
        // `apply_style` swaps the *class* attribute, not inline style, so
        // these coexist).
        if let Some(el) = node.dyn_ref::<web_sys::HtmlElement>() {
            let _ = el.style().set_property(prop, value);
        }
    }

    pub(crate) fn create_reactive_anchor_impl(&mut self) -> Node {
        primitives::view::create_reactive_anchor(self)
    }

    pub(crate) fn create_text_impl(
        &mut self,
        content: &str,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::text::create(self, content);
        // Text role has no first-class ARIA equivalent — the helper
        // emits nothing for it. Hint/identifier/live_region still apply.
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn create_styled_text_impl(
        &mut self,
        runs: &[runtime_shared::TextRun],
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::text::create_styled(self, runs);
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn update_styled_text_impl(&mut self, node: &Node, runs: &[runtime_shared::TextRun]) {
        // Never called on theme swaps (the cohort driver
        // short-circuits on cascade-capable backends — run colors are
        // `var()` refs); kept for direct callers.
        primitives::text::update_styled(self, node, runs);
    }

    pub(crate) fn create_button_impl(
        &mut self,
        label: &str,
        on_click: &runtime_shared::Action,
        leading_icon: Option<&runtime_shared::IconData>,
        trailing_icon: Option<&runtime_shared::IconData>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node =
            primitives::button::create(self, label, on_click.fire.clone(), leading_icon, trailing_icon);
        // `<button>` has implicit ARIA role; skip inferring one so we
        // don't write `role="button"` redundantly. Author overrides
        // via `props.role` still apply.
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn create_pressable_impl(
        &mut self,
        on_click: Rc<dyn Fn()>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::pressable::create(self, on_click);
        // Pressable is a `<div>` with click — explicit `role="button"`
        // is what tells the AX walker it's interactive.
        a11y::apply(
            &node,
            a11y,
            Some(runtime_shared::accessibility::Role::Button),
        );
        node
    }

    pub(crate) fn install_touch_handler_impl(
        &mut self,
        node: &Node,
        handler: runtime_shared::TouchHandler,
    ) {
        primitives::touch::install(node, handler);
    }

    pub(crate) fn install_wheel_handler_impl(
        &mut self,
        node: &Node,
        handler: runtime_shared::WheelHandler,
    ) {
        primitives::wheel::install(node, handler);
    }

    pub(crate) fn install_hover_handler_impl(
        &mut self,
        node: &Node,
        handler: runtime_shared::HoverHandler,
    ) {
        primitives::hover::install(node, handler);
    }

    pub(crate) fn install_file_drop_handler_impl(
        &mut self,
        node: &Node,
        handler: runtime_shared::FileDropHandler,
    ) {
        primitives::file_drop::install(node, handler);
    }

    pub(crate) fn mark_preserves_focus_impl(&mut self, node: &Node) {
        primitives::focus_retention::mark(self, node);
    }

    // `claim_touch` keeps the default no-op. On web, claims happen
    // inline in the pointer-event listener closure (where we have
    // the live `PointerEvent` to pass to `setPointerCapture`). The
    // trait method exists for symmetry with iOS / Android where the
    // framework dispatches events externally.

    pub(crate) fn insert_impl(&mut self, parent: &mut Node, child: Node) {
        #[cfg(feature = "hydrate")]
        if self.hydrating {
            // Subtree-remount resync: the fresh remount root is being
            // inserted → swap it in for the stale SSR node *in place*,
            // restore the cursor to the stale node's next sibling, and
            // leave the fresh subtree so siblings adopt again.
            if self.hydrate_resync_remount(parent, &child) {
                return;
            }
            // Outside a remount subtree: adopted nodes are already
            // parent↔child in the SSR DOM, so inserting is a no-op.
            // Inside a remount subtree (suppress, not the root): a fresh
            // node → fall through to a normal append.
            if !self.hydration_suppress && self.hydrate_child_already_adopted(parent, &child) {
                return;
            }
        }
        primitives::view::insert(parent, child)
    }

    pub(crate) fn insert_many_impl(&mut self, parent: &mut Node, children: Vec<Node>) {
        #[cfg(feature = "hydrate")]
        if self.hydrating {
            // Route each child through the same remount-resync + adopted
            // no-op as single `insert`, then batch-insert only the
            // genuinely fresh remainder. A remount root in the batch (the
            // `Repeat` fallback collects rows then hands them here) is
            // swapped in for its stale SSR node in place; adopted SSR
            // children are already parented and must not be re-inserted.
            let mut fresh: Vec<Node> = Vec::with_capacity(children.len());
            for child in children {
                if self.hydrate_resync_remount(parent, &child) {
                    continue;
                }
                if !self.hydration_suppress && self.hydrate_child_already_adopted(parent, &child) {
                    continue;
                }
                fresh.push(child);
            }
            primitives::view::insert_many(self, parent, fresh);
            return;
        }
        primitives::view::insert_many(self, parent, children)
    }

    // Child-splicing: the DOM does `insertBefore` / `removeChild`
    // directly, so keyed `Each` reconciliation runs in place — unchanged
    // rows keep their nodes (and their render scope), removed rows are
    // detached one-by-one, and reorders move existing nodes rather than
    // rebuilding. Without this the framework falls back to full rebuild.
    pub(crate) fn supports_child_splice_impl(&self) -> bool {
        true
    }

    pub(crate) fn insert_at_impl(&mut self, parent: &mut Node, child: Node, index: usize) {
        #[cfg(feature = "hydrate")]
        if self.hydrating {
            // Anchorless `when` / `switch` splice + keyed `Each` reconcile
            // parent their branch/rows through here. A remount root must
            // still be swapped in for its stale SSR node (else the diverged
            // subtree renders twice — the duplicated absolutely-positioned
            // nav bug); an adopted SSR node is already correctly positioned
            // in build order, so re-`insert_before`ing it would reorder it.
            if self.hydrate_resync_remount(parent, &child) {
                return;
            }
            if !self.hydration_suppress && self.hydrate_child_already_adopted(parent, &child) {
                return;
            }
        }
        primitives::view::insert_at(parent, child, index)
    }

    pub(crate) fn remove_child_impl(&mut self, parent: &Node, child: &Node) {
        primitives::view::remove_child(parent, child)
    }

    pub(crate) fn update_text_impl(&mut self, node: &Node, content: &str) {
        primitives::text::update_text(node, content)
    }

    pub(crate) fn create_text_with_id_impl(
        &mut self,
        content: &str,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Option<(Node, u32)> {
        let _t = crate::phase_timer::PhaseTimer::start("text_create_with_id");
        // Without an installed self-handle in `WEB_BACKEND_HANDLE`,
        // the microtask flush has no way back to `&mut self`. Bail
        // out so the framework falls back to the unbatched path
        // rather than queueing updates that will never drain.
        let has_handle = WEB_BACKEND_HANDLE.with(|s| s.borrow().is_some());
        if !has_handle {
            return None;
        }
        self.ensure_text_batch_shim();
        if self.text_register_fn.is_none() {
            let window = web_sys::window().expect("no window");
            let f_val = js_sys::Reflect::get(
                &window,
                &wasm_bindgen::JsValue::from_str("__idealystRegisterText"),
            )
            .expect("Reflect::get for __idealystRegisterText failed");
            self.text_register_fn = Some(
                f_val
                    .dyn_into::<js_sys::Function>()
                    .expect("__idealystRegisterText is not a Function — shim injection failed"),
            );
        }
        // Create the span WITH an inner Text node and register the
        // inner Text node — not the span — in the JS registry.
        // Update path then sets `text.nodeValue = ...` (O(1) string-
        // slot assignment) instead of `span.textContent = ...`
        // (which removes all children + creates a new Text node).
        // Measured: at 20 k leaves / 12 fan-outs, the difference is
        // ~30 ms per flush.
        let (span, inner_text) = primitives::text::create_with_inner_text_hydrating(self, content);
        let id = self.next_text_id;
        self.next_text_id += 1;
        let _ = self
            .text_register_fn
            .as_ref()
            .expect("set above")
            .call2(
                &wasm_bindgen::JsValue::NULL,
                &wasm_bindgen::JsValue::from(id),
                inner_text.as_ref(),
            )
            .expect("__idealystRegisterText call failed");
        // Apply ARIA to the outer span (the inner Text node has no
        // attributes of its own). No inferred role on text spans —
        // screen readers already announce text content directly.
        a11y::apply(&span, a11y, None);
        Some((span, id))
    }

    pub(crate) fn update_text_by_id_impl(&mut self, id: u32, content: String) {
        let _t = crate::phase_timer::PhaseTimer::start("text_update_by_id");
        self.append_pending_text(id, |buf| buf.push_str(&content));
        self.schedule_text_flush();
    }

    pub(crate) fn release_text_id_impl(&mut self, id: u32) {
        self.text_release_batch.push(id);
        // Piggy-back on the same flush microtask the updates use.
        // Releases without queued updates are rare (scope teardown
        // without a triggering signal change) but cheap to schedule.
        self.schedule_text_flush();
    }

    pub(crate) fn supports_js_text_bindings_impl(&self) -> bool {
        // True iff the variant has installed the text batcher (which
        // also pre-injects the bindings shim and sets
        // `WEB_BACKEND_HANDLE`). Without that, the signal-change
        // notifier closure has no way back to `&mut self` and the
        // JS-side update path wouldn't fire — better to fall back
        // to the Rust Effect.
        WEB_BACKEND_HANDLE.with(|s| s.borrow().is_some())
    }

    pub(crate) fn register_reactive_text_binding_impl(
        &mut self,
        text_id: u32,
        signal_ids: &[u64],
        template_parts: &[&str],
        initial_values: &[&str],
        stringifiers: &[std::rc::Rc<dyn Fn() -> String>],
    ) {
        // Delegates to the inherent method on `WebBackend`. The
        // inherent method exists separately because it predates the
        // trait-method (and is also useful directly for code paths
        // that hold a concrete `&mut WebBackend`).
        WebBackend::register_reactive_text_binding(
            self,
            text_id,
            signal_ids,
            template_parts,
            initial_values,
            stringifiers,
        )
    }

    pub(crate) fn release_reactive_text_binding_impl(&mut self, text_id: u32) {
        WebBackend::release_reactive_text_binding(self, text_id)
    }

    pub(crate) fn supports_js_class_bindings_impl(&self) -> bool {
        // Same gate as text bindings — both rely on
        // `WEB_BACKEND_HANDLE` being set (the signal-changed notifier
        // needs the self-handle to call back into the backend) and on
        // the shims being injected. Without the handle, the framework
        // falls back to a per-node Effect via the spec's compute
        // closure.
        WEB_BACKEND_HANDLE.with(|s| s.borrow().is_some())
    }

    pub(crate) fn register_reactive_class_binding_impl(
        &mut self,
        node: &Node,
        signal_id: u64,
        values: &[u32],
        classes: &[&str],
        value_reader: std::rc::Rc<dyn Fn() -> u32>,
    ) -> u32 {
        WebBackend::register_reactive_class_binding(
            self,
            node,
            signal_id,
            values,
            classes,
            value_reader,
        )
    }

    pub(crate) fn release_reactive_class_binding_impl(&mut self, binding_id: u32) {
        WebBackend::release_reactive_class_binding(self, binding_id)
    }

    pub(crate) fn mint_class_for_app_impl(
        &mut self,
        app: &runtime_shared::StyleApplication,
    ) -> Option<String> {
        Some(self.impl_mint_class_for_app(app))
    }

    pub(crate) fn create_image_impl(
        &mut self,
        src: &str,
        alt: Option<&str>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::image::create(self, src, alt);
        // `<img>` carries implicit `role="img"` — don't infer.
        // `alt` and `a11y.label` both target accessibility text; the
        // helper's aria-label takes precedence when both are set.
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn update_image_src_impl(&mut self, node: &Node, src: &str) {
        primitives::image::update_src(self, node, src)
    }

    pub(crate) fn update_image_alt_impl(&mut self, node: &Node, alt: Option<&str>) {
        primitives::image::update_alt(node, alt)
    }

    pub(crate) fn install_image_load_handler_impl(
        &mut self,
        node: &Node,
        handler: runtime_shared::ImageLoadHandler,
    ) {
        primitives::image::install_load(node, handler);
    }

    pub(crate) fn install_image_error_handler_impl(
        &mut self,
        node: &Node,
        handler: runtime_shared::ImageErrorHandler,
    ) {
        primitives::image::install_error(node, handler);
    }

    pub(crate) fn create_icon_impl(
        &mut self,
        data: &runtime_shared::primitives::icon::IconData,
        color: Option<&runtime_shared::Color>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::icon::create(self, data, color);
        // SVG icons get explicit `role="img"` since `<svg>` doesn't
        // have an implicit role by default; the helper writes it when
        // the inferred role is supplied.
        a11y::apply(&node, a11y, Some(runtime_shared::accessibility::Role::Image));
        node
    }

    pub(crate) fn update_icon_color_impl(&mut self, node: &Node, color: &runtime_shared::Color) {
        primitives::icon::update_color(node, color)
    }

    pub(crate) fn update_icon_data_impl(
        &mut self,
        node: &Node,
        data: &runtime_shared::primitives::icon::IconData,
    ) {
        primitives::icon::update_data(self, node, data)
    }

    pub(crate) fn update_icon_stroke_impl(&mut self, node: &Node, progress: f32) {
        primitives::icon::update_stroke(node, progress)
    }

    pub(crate) fn animate_icon_stroke_impl(
        &mut self,
        node: &Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: runtime_shared::Easing,
        infinite: bool,
        _autoreverses: bool,
    ) {
        primitives::icon::animate_stroke(node, from, to, duration_ms, easing, infinite)
    }

    pub(crate) fn create_text_input_impl(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        on_blur: Option<runtime_shared::primitives::text_input::BlurHandler>,
        secure: bool,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::text_input::create(
            self,
            initial_value,
            placeholder,
            on_change,
            on_key_down,
            on_blur,
            secure,
        );
        // `<input>` has implicit textbox role; no inference needed.
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn update_text_input_value_impl(&mut self, node: &Node, value: &str) {
        primitives::text_input::update_value(node, value)
    }

    pub(crate) fn update_text_input_secure_impl(&mut self, node: &Node, secure: bool) {
        primitives::text_input::update_secure(node, secure)
    }

    pub(crate) fn set_text_input_focus_handler_impl(&mut self, node: &Node, handler: Rc<dyn Fn(bool)>) {
        primitives::text_input::set_focus_handler(self, node, handler);
    }

    pub(crate) fn update_text_input_placeholder_impl(&mut self, node: &Node, placeholder: Option<&str>) {
        primitives::text_input::update_placeholder(node, placeholder)
    }

    pub(crate) fn create_text_area_impl(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::text_area::create(
            self,
            initial_value,
            placeholder,
            wrap,
            min_rows,
            max_rows,
            on_change,
            on_key_down,
        );
        // `<textarea>` is implicitly a multiline textbox; no inference.
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn update_text_area_value_impl(&mut self, node: &Node, value: &str) {
        primitives::text_area::update_value(node, value)
    }

    pub(crate) fn make_text_area_handle_impl(
        &self,
        node: &Node,
    ) -> runtime_shared::primitives::text_area::TextAreaHandle {
        primitives::text_area::make_handle(node)
    }

    pub(crate) fn create_toggle_impl(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::toggle::create(self, initial_value, on_change);
        a11y::apply(&node, a11y, Some(runtime_shared::accessibility::Role::Switch));
        node
    }

    pub(crate) fn update_toggle_value_impl(&mut self, node: &Node, value: bool) {
        primitives::toggle::update_value(node, value)
    }

    pub(crate) fn create_scroll_view_impl(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::scroll_view::create(self, horizontal, on_scroll);
        // ScrollView has no first-class ARIA role — it's a generic
        // container; the platform handles scroll affordances. Author
        // can override.
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn create_slider_impl(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::slider::create(self, initial_value, min, max, step, on_change);
        // `<input type=range>` is implicitly `role=slider`; skip inference.
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn update_slider_value_impl(&mut self, node: &Node, value: f32) {
        primitives::slider::update_value(node, value)
    }

    pub(crate) fn create_activity_indicator_impl(
        &mut self,
        size: runtime_shared::primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&runtime_shared::Color>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::activity_indicator::create(self, size, color);
        a11y::apply(&node, a11y, Some(runtime_shared::accessibility::Role::Spinner));
        node
    }

    pub(crate) fn update_activity_indicator_size_impl(
        &mut self,
        node: &Node,
        size: runtime_shared::primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        primitives::activity_indicator::update_size(node, size)
    }

    pub(crate) fn create_virtualizer_impl(
        &mut self,
        callbacks: runtime_shared::VirtualizerCallbacks<Node>,
        overscan: f32,
        layout: runtime_shared::VirtualLayout,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::virtualizer::create(self, callbacks, overscan, layout);
        a11y::apply(&node, a11y, Some(runtime_shared::accessibility::Role::List));
        node
    }

    pub(crate) fn virtualizer_data_changed_impl(&mut self, node: &Node) {
        primitives::virtualizer::data_changed(self, node)
    }

    pub(crate) fn release_virtualizer_impl(&mut self, node: &Node) {
        primitives::virtualizer::release(self, node)
    }

    pub(crate) fn create_graphics_impl(
        &mut self,
        on_ready: runtime_shared::primitives::graphics::OnReady,
        on_resize: runtime_shared::primitives::graphics::OnResize,
        on_lost: runtime_shared::primitives::graphics::OnLost,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::graphics::create(self, on_ready, on_resize, on_lost);
        // `<canvas>` has no implicit ARIA role; author code MUST set
        // `props.label` for screen-reader users to know what's
        // rendered. We don't infer a role here — let author decide
        // (canvas + label is enough; explicit role="img" is also
        // common).
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn release_graphics_impl(&mut self, node: &Node) {
        primitives::graphics::release(self, node)
    }

    pub(crate) fn make_graphics_handle_impl(
        &self,
        node: &Node,
    ) -> runtime_shared::primitives::graphics::GraphicsHandle {
        primitives::graphics::make_handle(self, node)
    }

    pub(crate) fn create_link_impl(
        &mut self,
        config: runtime_shared::primitives::link::LinkConfig,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::link::create(self, config);
        // `<a>` has implicit role="link"; skip inference.
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn update_link_url_impl(&mut self, node: &Node, url: &str) {
        primitives::link::update_url(node, url)
    }

    pub(crate) fn make_link_handle_impl(
        &self,
        node: &Node,
    ) -> runtime_shared::primitives::link::LinkHandle {
        primitives::link::make_handle(node)
    }

    pub(crate) fn create_portal_impl(
        &mut self,
        target: runtime_shared::primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        let node = primitives::portal::create(self, target, on_dismiss, trap_focus);
        // Portal containers are transparent (the mounted content
        // carries its own role); pass None for inferred role and let
        // author opt into `Dialog`/`AlertDialog` via props.role.
        a11y::apply(&node, a11y, None);
        node
    }

    pub(crate) fn release_portal_impl(&mut self, node: &Node) {
        primitives::portal::release(self, node)
    }

    pub(crate) fn make_portal_handle_impl(
        &self,
        node: &Node,
    ) -> runtime_shared::primitives::portal::PortalHandle {
        primitives::portal::make_handle(node)
    }

    pub(crate) fn create_external_impl(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _payload: &Rc<dyn std::any::Any>,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Node {
        // Runtime v2: there is no backend-side External registry any more.
        // Third-party primitives register a payload handler on the scene
        // `Registry` (`runtime_scene::Registry::register`), which dispatches
        // BEFORE reaching a backend cap, so this method is only ever the
        // last-resort placeholder — the frozen degradation an SDK's own
        // handler asks for on a host it has no leg for
        // (`ExternalOps::create_external` → children → style → teardown).
        // Hydration still applies: an unadopted fresh node must arm the
        // subtree remount, or the cursor stalls on the unconsumed SSR host
        // and every following sibling mismatches (the first
        // `[hydrate] diverge`).
        let hydrate_cursor_before = self.hydrate_cursor_snapshot();
        let node: Node = external_placeholder_element(&self.doc, type_name).into();
        // External handlers don't know their semantic role; the author
        // supplies it via props.role. No inferred role here.
        a11y::apply(&node, a11y, None);
        self.hydrate_external_note_if_unadopted(&hydrate_cursor_before, &node);
        node
    }

    pub(crate) fn release_external_impl(&mut self, _node: &Node) {
        // The web backend has no per-external bookkeeping today.
        // Future hooks (e.g. per-instance event-listener cleanup)
        // would land here, queried by `data-external-id` like
        // portals/virtualizers/graphics.
    }

    pub(crate) fn apply_presence_impl(
        &mut self,
        node: &Node,
        state: runtime_shared::PresenceState,
        transition: Option<(u32, runtime_shared::Easing)>,
    ) {
        primitives::presence::apply(self, node, state, transition)
    }

    pub(crate) fn clear_children_impl(&mut self, node: &Node) {
        primitives::view::clear_children(node)
    }

    pub(crate) fn register_stylesheet_impl(&mut self, rules: &[Rc<StyleRules>]) {
        self.impl_register_stylesheet(rules)
    }

    pub(crate) fn unregister_stylesheet_impl(&mut self, rules: &[Rc<StyleRules>]) {
        self.impl_unregister_stylesheet(rules)
    }

    pub(crate) fn install_tokens_impl(&mut self, tokens: &[runtime_shared::TokenEntry]) {
        self.impl_install_theme_variables(tokens)
    }

    pub(crate) fn update_tokens_impl(&mut self, tokens: &[runtime_shared::TokenEntry]) {
        // Same machinery handles both — the impl detects whether
        // the :root rule already exists and either inserts or
        // setProperty's.
        self.impl_install_theme_variables(tokens)
    }

    pub(crate) fn set_app_background_impl(&mut self, color: &runtime_shared::Tokenized<runtime_shared::Color>) {
        self.impl_set_app_background(color)
    }

    pub(crate) fn set_scrollbar_theme_impl(
        &mut self,
        thumb: &runtime_shared::Tokenized<runtime_shared::Color>,
        track: &runtime_shared::Tokenized<runtime_shared::Color>,
    ) {
        self.impl_set_scrollbar_theme(thumb, track)
    }

    pub(crate) fn set_app_key_handler_impl(
        &mut self,
        handler: Option<runtime_shared::primitives::key::KeyDownHandler>,
    ) {
        crate::primitives::keyboard::install_app_key_handler(self, handler)
    }

    pub(crate) fn register_asset_impl(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        self.impl_register_asset(id, kind, source)
    }

    pub(crate) fn unregister_asset_impl(&mut self, id: AssetId, kind: AssetTag) {
        self.impl_unregister_asset(id, kind)
    }

    pub(crate) fn register_typeface_impl(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        self.impl_register_typeface(id, family_name, faces, fallback)
    }

    pub(crate) fn unregister_typeface_impl(&mut self, id: TypefaceId) {
        self.impl_unregister_typeface(id)
    }

    pub(crate) fn apply_style_impl(&mut self, node: &Node, style: &Rc<StyleRules>) {
        self.impl_apply_style(node, style)
    }

    /// DOM scroll offset of `node` (0,0 for non-Element nodes and
    /// non-scrolling elements — `scrollLeft/Top` read 0 there). Used by
    /// the navigator substrate's URL sync for back-restores scroll.
    pub(crate) fn node_scroll_impl(&self, node: &Node) -> (f32, f32) {
        node.dyn_ref::<web_sys::Element>()
            .map(|el| (el.scroll_left() as f32, el.scroll_top() as f32))
            .unwrap_or((0.0, 0.0))
    }

    /// Set `node`'s DOM scroll offset. Setting on a non-scrolling
    /// element is a browser-defined no-op, matching the trait contract.
    pub(crate) fn set_node_scroll_impl(&mut self, node: &Node, x: f32, y: f32) {
        if let Some(el) = node.dyn_ref::<web_sys::Element>() {
            el.set_scroll_left(x as i32);
            el.set_scroll_top(y as i32);
        }
    }

    pub(crate) fn set_animated_f32_impl(
        &mut self,
        node: &Node,
        prop: runtime_shared::animation::AnimProp,
        value: f32,
    ) {
        self.impl_set_animated_f32(node, prop, value);
    }

    pub(crate) fn set_animated_color_impl(
        &mut self,
        node: &Node,
        prop: runtime_shared::animation::AnimProp,
        value: [f32; 4],
    ) {
        self.impl_set_animated_color(node, prop, value);
    }

    /// Opt into the walker's batched-Repeat path. When the walker sees
    /// a `Element::Repeat` whose rows are pure View+Text+static-style,
    /// it builds a [`BackendBatch`] and ships it through
    /// [`execute_batch`] instead of issuing per-row backend calls.
    pub(crate) fn supports_batched_repeat_impl(&self) -> bool {
        true
    }

    /// Resolve a content-keyed CSS class for a static `StyleRules`.
    /// Returns the cached class name if the rules were registered
    /// (the walker calls `register_stylesheet` via
    /// `style::ensure_registered_with` before invoking this), or
    /// `None` if no cache hit — the walker then bails out of the
    /// batch path for this Repeat and the per-call apply route mints
    /// a dynamic class through `impl_apply_style`.
    ///
    /// Returning `None` is the safe fallback. The batch path only
    /// fires when every row's class can be name-shipped in one FFI
    /// call; if any row's style isn't pre-minted, falling back to
    /// per-call is correct.
    pub(crate) fn mint_style_class_impl(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        let _t = crate::phase_timer::PhaseTimer::start("mint_style_class");

        // Fast path: pointer-keyed lookup. The framework's resolution
        // cache returns the same `Rc<StyleRules>` for a given
        // `(sheet, variants, overrides)`, so a styled cohort of N
        // homogeneous rows hands us identical Rcs — pointer-eq lookup
        // skips the per-call `content_key()` hash entirely. `pregen_by_ptr`
        // is populated alongside `pregen` during
        // `impl_register_stylesheet`.
        let ptr = std::rc::Rc::as_ptr(style);
        if let Some(name) = self.pregen_by_ptr.get(&ptr) {
            let _t_hit = crate::phase_timer::PhaseTimer::start("mint_style_class_ptr_hit");
            let r = name.clone();
            drop(_t_hit);
            return Some(r);
        }

        // Slow path: content-keyed lookup. Used when the caller passes
        // a fresh Rc whose content matches a registered stylesheet but
        // whose pointer hasn't been seen. Hashes the full StyleRules.
        let _t_slow = crate::phase_timer::PhaseTimer::start("mint_style_class_content_lookup");
        let key = style.content_key();
        let result = self.pregen.get(&key).map(|entry| entry.name.clone());
        drop(_t_slow);

        let _t2 = crate::phase_timer::PhaseTimer::start(if result.is_some() {
            "mint_style_class_hit"
        } else {
            "mint_style_class_miss"
        });
        drop(_t2);
        result
    }

    /// Execute a [`BackendBatch`] in one wasm→JS round-trip via the
    /// `__idealystExecuteBatch` shim. Returns a Vec sized to
    /// `batch.node_count`, indexed by `local_id`.
    ///
    /// First call lazily injects the JS shim (`runtime/js/batch.js`)
    /// and caches the function handle so subsequent calls skip the
    /// `Reflect::get` lookup.
    pub(crate) fn execute_batch_impl(&mut self, batch: runtime_shared::BackendBatch) -> Vec<Node> {
        self.execute_batch_inner(batch, None)
    }

    /// Execute the batch AND parent the row tops in one FFI round-trip.
    ///
    /// Folds what used to be `execute_batch` + `insert_many` into one
    /// shim invocation. The savings come from the per-child
    /// `appendChild` calls — previously N FFI hops, now N pure JS
    /// loop iterations inside the shim. Measured ~10 ms reduction
    /// per 100 k-row transition (~115 ms across the rebuild bench
    /// suite via the debug-stats phase counters). The benefit
    /// surfaces more clearly in the `worstFrame` metric than in
    /// `apply_p50`, because per-frame apply noise (±15 ms at 100 k)
    /// can mask a 7 ms improvement.
    ///
    /// `parent` must be a real DOM node (the same kind you'd pass to
    /// `insert_many`); `attach_locals` must reference valid
    /// `local_id`s from `batch`.
    pub(crate) fn execute_batch_with_attach_impl(
        &mut self,
        batch: runtime_shared::BackendBatch,
        parent: &mut Node,
        attach_locals: &[u32],
    ) -> Vec<Node> {
        self.execute_batch_inner(batch, Some((parent, attach_locals)))
    }

    /// Web handles interaction states via native CSS selectors —
    /// pseudo-classes (`:hover`, `:active`, `:focus`) for the live
    /// states the browser tracks itself, and the `[disabled]` attribute
    /// selector for the disabled state (`set_disabled` sets the
    /// attribute; a `<div>` pressable never matches the `:disabled`
    /// pseudo). No Rust-side state signal is needed. The framework calls
    /// `apply_styled_states` instead of `apply_style` when this returns
    /// true.
    pub(crate) fn handles_states_natively_impl(&self) -> bool {
        true
    }

    /// Web emits `var(--token, fallback)` for every `Tokenized<T>`
    /// value and `update_tokens` mutates `:root` in place. The
    /// browser's cascade propagates the new values to every node
    /// referencing them — no per-node re-apply needed for theme
    /// value changes. Saves O(N) work per theme swap.
    pub(crate) fn token_updates_propagate_via_cascade_impl(&self) -> bool {
        true
    }

    pub(crate) fn apply_styled_states_impl(
        &mut self,
        node: &Node,
        base: &Rc<StyleRules>,
        overlays: &[(runtime_shared::StateBits, Rc<StyleRules>)],
    ) {
        self.impl_apply_styled_states(node, base, overlays)
    }

    pub(crate) fn apply_styled_variants_impl(
        &mut self,
        node: &Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(runtime_shared::StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(runtime_shared::Breakpoint, Rc<StyleRules>)],
        container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        self.impl_apply_styled_variants(
            node,
            base,
            state_overlays,
            breakpoint_overlays,
            container_overlays,
        )
    }

    pub(crate) fn mark_container_impl(&mut self, node: &Node) {
        self.impl_mark_container(node)
    }

    pub(crate) fn on_node_unstyled_impl(&mut self, node: &Node) {
        self.impl_on_node_unstyled(node)
    }

    pub(crate) fn set_disabled_impl(&mut self, node: &Node, disabled: bool) {
        // Mark the node with the HTML `disabled` *attribute*. Form
        // controls (button, input, select) treat it as inert natively;
        // for a `<div>` pressable it's the hook the disabled-state CSS
        // matches on — the overlay is emitted under the `[disabled]`
        // attribute selector (see `style.rs`), NOT the `:disabled`
        // pseudo-class, precisely so a `<div disabled>` styles correctly.
        let Ok(element) = node.clone().dyn_into::<web_sys::Element>() else {
            return;
        };
        if disabled {
            let _ = element.set_attribute("disabled", "");
        } else {
            let _ = element.remove_attribute("disabled");
        }
    }

    /// Web state styling uses native CSS selectors (`:hover`,
    /// `:active`, `:focus`, and the `[disabled]` attribute selector)
    /// rather than reactive JS listeners. That happens at CSS-emit time
    /// in `apply_style` (see `rules_to_css` / state rule generation),
    /// not here. We
    /// override `attach_states` to a no-op so the framework's
    /// signal-driven state machinery doesn't fire on web.
    ///
    /// Why not listeners + signal-driven re-style? It causes wasm-
    /// bindgen `WasmRefCell` re-entry crashes when DOM events fire
    /// while a style is being applied, and the CSS path is both
    /// simpler and faster (browser tracks the state natively, no
    /// per-event Rust↔JS round trip).
    pub(crate) fn attach_states_impl(
        &mut self,
        _node: &Node,
        _setter: Rc<dyn Fn(runtime_shared::StateBits, bool)>,
    ) {
        // intentional no-op on web; CSS pseudo-classes drive states.
    }

    pub(crate) fn make_button_handle_impl(&self, node: &Node) -> ButtonHandle {
        primitives::button::make_handle(node)
    }

    pub(crate) fn make_pressable_handle_impl(
        &self,
        node: &Node,
    ) -> runtime_shared::PressableHandle {
        primitives::pressable::make_handle(node)
    }

    pub(crate) fn make_view_handle_impl(&self, node: &Node) -> runtime_shared::ViewHandle {
        // Wrap the actual `web_sys::Node` (not the trait-default
        // `Rc<()>`), so framework helpers like `LayoutPlan` can
        // downcast back to the concrete node and operate on it.
        runtime_shared::ViewHandle::new(Rc::new(node.clone()), &WebViewOps)
    }

    pub(crate) fn make_text_handle_impl(&self, node: &Node) -> runtime_shared::TextHandle {
        // Same plumbing as `make_view_handle` for the text element so
        // author-level animation drivers (welcome's `drive_color_text_av`)
        // can downcast `text_ref.as_any()` to `web_sys::Node` and write
        // `style.color` directly. Without this the typed handle stores
        // the trait-default `Rc<()>` and the downcast silently fails,
        // leaving text color frozen at its stylesheet value.
        runtime_shared::TextHandle::new(Rc::new(node.clone()), &WebTextOps)
    }

    pub(crate) fn make_text_input_handle_impl(
        &self,
        node: &Node,
    ) -> runtime_shared::primitives::text_input::TextInputHandle {
        primitives::text_input::make_handle(node)
    }

    pub(crate) fn make_scroll_view_handle_impl(
        &self,
        node: &Node,
    ) -> runtime_shared::primitives::scroll_view::ScrollViewHandle {
        primitives::scroll_view::make_handle(node)
    }

    pub(crate) fn finish_impl(&mut self, root: Node) {
        #[cfg(feature = "hydrate")]
        if self.hydrating {
            // The initial adoption pass is done; subsequent reactive
            // rebuilds create fresh nodes through the normal path.
            self.hydrating = false;
            crate::scheduler::end_hydration_buffering();
            // Navigator cursor-steering state is scoped to this pass. A
            // steered outlet whose layout build never adopted it (its
            // own create diverged) would otherwise sit in
            // `consumed_outlets` forever, and a post-`finish` rebuild
            // that happened to reuse the node would wrongly skip its
            // subtree. Balanced begin/end pairs leave `nav_saved` empty;
            // clearing is the belt to that suspenders.
            self.hydration_nav_saved.clear();
            self.hydration_consumed_outlets.clear();
            // Safety net: a remount whose fresh root was parented by a path
            // that somehow bypassed `hydrate_resync_remount` would leave its
            // stale SSR node orphaned in the DOM, rendering the diverged
            // subtree twice. Detach any such leftover now so "diverged →
            // remounted → stale removed" holds on EVERY path. Normally the
            // field is already `None` (the resync took it during `insert*`).
            // EXCEPTION: when the tree `root` itself is the pending remount
            // root, the stale belongs to the root-swap branch below (it
            // `replace_child`s root into the stale's slot), so leave it.
            let root_is_pending_remount = self
                .hydration_remount_root
                .as_ref()
                .map(|r| r.is_same_node(Some(&root)))
                .unwrap_or(false);
            if !root_is_pending_remount {
                if let Some(stale) = self.hydration_remount_stale.take() {
                    if let Some(sp) = stale.parent_node() {
                        let _ = sp.remove_child(&stale);
                    }
                    self.hydration_remount_root = None;
                    self.hydration_suppress = false;
                }
            }
            // Clean / subtree-local outcome: the root was adopted (or
            // remounted in place by `insert`), so it's already `#app`'s
            // child — nothing to swap. Diverging subtrees were already
            // replaced in place; the server's DOM is the live DOM.
            if root
                .parent_node()
                .map(|p| p.is_same_node(Some(self.mount.as_ref())))
                .unwrap_or(false)
            {
                return;
            }
            // The ROOT itself was a remount (it's never `insert`ed — it
            // comes straight here). Swap it for the stale SSR root.
            if let Some(stale) = self.hydration_remount_stale.take() {
                if let Some(sp) = stale.parent_node() {
                    let _ = sp.replace_child(&root, &stale);
                    self.hydration_remount_root = None;
                    self.hydration_suppress = false;
                    return;
                }
            }
            // Defensive fall-through (e.g. the navigator built its root
            // outside the adoption cursor): clear + append.
        }
        // Replace any prior contents of the mount point before attaching
        // the live tree. On a normal boot `#app` is empty and this is a
        // no-op; with SSR-without-hydration it holds the server-rendered
        // first-paint markup, which the booting bundle owns and replaces.
        while let Some(child) = self.mount.first_child() {
            let _ = self.mount.remove_child(&child);
        }
        self.mount
            .append_child(&root)
            .expect("mount append failed");
    }

    pub(crate) fn update_accessibility_impl(
        &mut self,
        node: &Node,
        a11y: &runtime_shared::accessibility::AccessibilityProps,
        inferred_role: Option<runtime_shared::accessibility::Role>,
    ) {
        // Reactive prop updates funnel through here. `a11y::apply` is
        // idempotent and clears attributes that drop to None, so the
        // same code path that builds the DOM also reconciles it.
        a11y::apply(node, a11y, inferred_role);
    }

    pub(crate) fn announce_for_accessibility_impl(
        &mut self,
        msg: &str,
        priority: runtime_shared::accessibility::LiveRegionPriority,
    ) {
        a11y::announce(msg, priority);
    }

    // dump_accessibility_tree: not implemented on web. Browsers walk
    // the live DOM + ARIA attributes themselves, so a parallel
    // semantics tree would be redundant. The trait default (None) is
    // correct here — no override needed.
}

/// Marker ops for `ViewHandle`. Views don't have methods yet (no
/// scroll, no measure) — the trait is reserved for future
/// additions. We still need an instance to satisfy
/// `ViewHandle::new`'s `&'static dyn ViewOps` parameter.
struct WebViewOps;
impl runtime_shared::ViewOps for WebViewOps {
    fn rect(&self, node: &dyn std::any::Any) -> runtime_shared::ViewportRect {
        match view_rect_from_node(node) {
            Some(r) => r,
            None => runtime_shared::ViewportRect::default(),
        }
    }

    /// Parent-relative frame. `offsetLeft`/`offsetTop` give the
    /// top-left in the nearest positioned ancestor's coordinate
    /// system (the DOM equivalent of UIKit's `view.frame` /
    /// Taffy's per-node rect). Width and height come from
    /// `getBoundingClientRect` because `offsetWidth`/`offsetHeight`
    /// quantize to integers, which loses sub-pixel precision the
    /// physics paths and overlay anchors care about. Returns
    /// `None` when the element isn't attached to the document —
    /// matches the trait's "not yet laid out" contract.
    fn frame(
        &self,
        node: &dyn std::any::Any,
    ) -> Option<runtime_shared::primitives::portal::ViewportRect> {
        let el = element_from_any(node)?;
        if !el.is_connected() {
            return None;
        }
        let r = el.get_bounding_client_rect();
        let (ox, oy) = el
            .clone()
            .dyn_into::<web_sys::HtmlElement>()
            .map(|h| (h.offset_left() as f32, h.offset_top() as f32))
            // SVG / non-HTML elements have no `offsetLeft`; fall
            // back to viewport coords. Authors mixing those into
            // overlays already opt into that trade-off via the
            // primitives that emit them.
            .unwrap_or((r.x() as f32, r.y() as f32));
        Some(runtime_shared::primitives::portal::ViewportRect {
            x: ox,
            y: oy,
            width: r.width() as f32,
            height: r.height() as f32,
        })
    }

    /// Viewport-relative frame. Same as `rect`, but returns `None`
    /// when the element isn't connected so callers can tell "not
    /// mounted yet" from "mounted at the origin" — `rect`'s
    /// non-`Option` shape can't.
    fn absolute_frame(
        &self,
        node: &dyn std::any::Any,
    ) -> Option<runtime_shared::primitives::portal::ViewportRect> {
        let el = element_from_any(node)?;
        if !el.is_connected() {
            return None;
        }
        let r = el.get_bounding_client_rect();
        Some(runtime_shared::primitives::portal::ViewportRect {
            x: r.x() as f32,
            y: r.y() as f32,
            width: r.width() as f32,
            height: r.height() as f32,
        })
    }

    /// Route `AnimatedValue::bind` writes through the crate-level
    /// [`set_animated_f32`] free function so author code doesn't
    /// need a `cfg(target_arch = "wasm32")` block to dispatch to
    /// the right backend. Downcasts `node` to `web_sys::Node`;
    /// silently no-ops if the cast fails.
    fn set_animated_f32(
        &self,
        node: &dyn std::any::Any,
        prop: runtime_shared::animation::AnimProp,
        value: f32,
    ) {
        if let Some(n) = node.downcast_ref::<web_sys::Node>() {
            crate::set_animated_f32(n, prop, value);
        }
    }

    /// Color-family analog of [`Self::set_animated_f32`].
    fn set_animated_color(
        &self,
        node: &dyn std::any::Any,
        prop: runtime_shared::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<web_sys::Node>() {
            crate::set_animated_color(n, prop, value);
        }
    }

    /// Compositor-driven keyframe loop — the web twin of the macOS
    /// render-server path. See `keyframes.rs` for the mapping table
    /// and contract; unmapped props return `false` (per-frame clock
    /// fallback).
    fn install_keyframe_animation(
        &self,
        node: &dyn std::any::Any,
        prop: runtime_shared::animation::AnimProp,
        keyframes: &[(f32, f32)],
        duration_ms: u32,
        repeat_forever: bool,
        autoreverse: bool,
    ) -> bool {
        let Some(el) = element_from_any(node) else {
            return false;
        };
        crate::keyframes::install(&el, prop, keyframes, duration_ms, repeat_forever, autoreverse)
    }

    /// Layout-change callback. Backed by `ResizeObserver`, which
    /// fires the callback whenever the element's box dimensions
    /// change — naturally re-fires on content changes, parent
    /// reflow, or window resize. The returned `LayoutSubscription`
    /// holds the `ResizeObserver` + the JS callback closure alive;
    /// dropping it calls `.disconnect()` to remove the listener.
    fn subscribe_layout(
        &self,
        node: &dyn std::any::Any,
        callback: Box<dyn Fn(f32, f32)>,
    ) -> runtime_shared::LayoutSubscription {
        let Some(el) = element_from_any(node) else {
            return runtime_shared::LayoutSubscription::noop();
        };
        // Wrap the user callback into a JS callback that reads the
        // first observed entry's contentRect. Single-element
        // observers always emit a one-entry array; we still bounds-
        // check in case the spec evolves.
        let cb = wasm_bindgen::closure::Closure::wrap(Box::new(
            move |entries: js_sys::Array, _observer: web_sys::ResizeObserver| {
                let Some(first) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>().ok()
                else {
                    return;
                };
                let rect = first.content_rect();
                callback(rect.width() as f32, rect.height() as f32);
            },
        )
            as Box<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>);
        let Ok(observer) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) else {
            return runtime_shared::LayoutSubscription::noop();
        };
        observer.observe(&el);
        // Move both into the cleanup closure so they live as long as
        // the subscription. Dropping the subscription drops the
        // observer (auto-disconnects via DOM ownership) and the JS
        // closure (so the wasm-bindgen ref doesn't leak).
        runtime_shared::LayoutSubscription::new(move || {
            observer.disconnect();
            drop(cb);
        })
    }
}

fn element_from_any(node: &dyn std::any::Any) -> Option<web_sys::Element> {
    let n = node.downcast_ref::<web_sys::Node>()?;
    n.clone().dyn_into::<web_sys::Element>().ok()
}

/// `TextOps` impl. The framework's animated-color binding routes
/// here so author code can write
/// `welcome_color.bind_text_color(text_ref, AnimProp::ForegroundColor)`
/// without a per-platform downcast block — same shape as
/// [`WebViewOps::set_animated_color`].
struct WebTextOps;
impl runtime_shared::TextOps for WebTextOps {
    fn set_animated_color(
        &self,
        node: &dyn std::any::Any,
        prop: runtime_shared::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<web_sys::Node>() {
            crate::set_animated_color(n, prop, value);
        }
    }
}

fn view_rect_from_node(node: &dyn std::any::Any) -> Option<runtime_shared::ViewportRect> {
    let el = element_from_any(node)?;
    let r = el.get_bounding_client_rect();
    Some(runtime_shared::ViewportRect {
        x: r.x() as f32,
        y: r.y() as f32,
        width: r.width() as f32,
        height: r.height() as f32,
    })
}

/// Build a "not supported" placeholder element for an unregistered
/// external primitive. Visible in dev so missing SDK bindings on this
/// platform are obvious; user-space `has_external::<T>()` discovery is
/// the supported way to render custom degradation instead.
fn external_placeholder_element(
    doc: &web_sys::Document,
    type_name: &'static str,
) -> web_sys::Element {
    let div = doc
        .create_element("div")
        .expect("create_element failed for external placeholder");
    let _ = div.set_attribute("data-external-unsupported", type_name);
    let _ = div.set_attribute(
        "style",
        "display: inline-block; padding: 8px 12px; \
         border: 1px dashed #c0392b; color: #c0392b; \
         font-family: monospace; font-size: 12px; \
         background: #fdecea;",
    );
    div.set_text_content(Some(&format!(
        "External \"{type_name}\" not supported on web"
    )));
    div
}
