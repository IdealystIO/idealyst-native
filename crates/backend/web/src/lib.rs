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
mod batch_queue;
#[cfg(test)]
mod tests;
#[cfg(feature = "async-driver")]
pub mod async_executor;
#[cfg(feature = "async-driver")]
pub mod dynlink;
mod assets;
mod defaults;
#[cfg(feature = "runtime-server")]
pub mod dev_transport;
pub mod drop_deferral;
pub mod logger;
mod phase_timer;
mod primitives;
#[cfg(feature = "async-driver")]
pub mod render_loop;
pub mod scheduler;
mod style;
pub mod time_source;
mod viewport_observer;

#[cfg(feature = "async-driver")]
pub use async_executor::install_async_executor;
#[cfg(feature = "async-driver")]
pub use dynlink::{host_reserve, install_dynlink_loader};
#[cfg(feature = "runtime-server")]
pub use dev_transport::{connect_web, WebClientHandle};
pub use drop_deferral::install_drop_deferral;
pub use logger::install_logger;
#[cfg(feature = "async-driver")]
pub use render_loop::install_render_loop;
pub use scheduler::install_scheduler;
pub use time_source::install_time_source;
pub use viewport_observer::{install_viewport_observer, page_is_prerendered, ssr_viewport};

/// Install a `Weak` self-handle for the active `WebBackend`. Required
/// by any code path that needs `&mut WebBackend` from outside the
/// build walker:
///  - [`AnimatedValue::bind`](runtime_core::animation::AnimatedValue::bind)
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
    prop: runtime_core::animation::AnimProp,
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
        use runtime_core::Backend;
        b.set_animated_f32(node, prop, value);
    };
}

/// Color-family counterpart of [`set_animated_f32`]. Routes through
/// the global backend's `set_animated_color`. `value` is sRGB
/// `[r, g, b, a]` with channels in `0..=1`.
pub fn set_animated_color(
    node: &web_sys::Node,
    prop: runtime_core::animation::AnimProp,
    value: [f32; 4],
) {
    let weak = WEB_BACKEND_HANDLE.with(|s| s.borrow().clone());
    let Some(weak) = weak else { return };
    let Some(rc) = weak.upgrade() else { return };
    if let Ok(mut b) = rc.try_borrow_mut() {
        use runtime_core::Backend;
        b.set_animated_color(node, prop, value);
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
    static FONT_FACES_PRESENT: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

use runtime_core::{
    AssetId, AssetSource, AssetTag, Backend, ButtonHandle, StyleRules, SystemFallback,
    TypefaceFace, TypefaceId,
};
use std::collections::HashMap;
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
impl runtime_core::primitives::navigator::NavigatorOps for NoopNavOps {}
static NOOP_NAV_OPS: NoopNavOps = NoopNavOps;

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
    pub(crate) _click_closures: Vec<Closure<dyn FnMut()>>,
    /// Keyboard handlers for `Element::Pressable` (Enter/Space →
    /// click). Held so JS doesn't drop them while the element is in
    /// the layout tree. The click handler itself lives in
    /// `_click_closures` (shared shape: `FnMut()` no-arg).
    pub(crate) _pressable_key_closures: Vec<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
    /// Closures attached to `<a>` elements for `Element::Link`.
    /// Held so JS doesn't drop them while the anchor is still in
    /// the layout tree. Same posture as `_click_closures`.
    pub(crate) _link_click_closures: Vec<Closure<dyn FnMut(web_sys::MouseEvent)>>,
    /// Pointer Events closures installed by
    /// [`primitives::touch::install`] — one per (node × pointer
    /// event type) attachment. Type-erased to `JsValue` so the four
    /// `pointer{down,move,up,cancel}` listeners and any future
    /// pointer event types share storage. Held so JS doesn't free
    /// them while the element is still in the layout tree;
    /// freed wholesale on backend drop.
    pub(crate) _touch_closures: Vec<wasm_bindgen::JsValue>,
    /// Per-node interaction-event closures. Keyed by node-id so we
    /// can drop them when `on_node_unstyled` fires. Each entry holds
    /// the listeners for one node (pointerenter, pointerleave,
    /// pointerdown, pointerup, focusin, focusout) plus the
    /// pointer-event-type closures so the JS side keeps them alive.
    pub(crate) state_listeners: HashMap<u32, Vec<Closure<dyn FnMut(web_sys::Event)>>>,
    /// Has the `@keyframes ui-spin` rule been injected? First
    /// ActivityIndicator creation injects it; later creations skip
    /// the work.
    pub(crate) spinner_keyframes_injected: bool,
    /// Has the virtualizer JS shim been injected? First Virtualizer
    /// creation injects `runtime/js/virtualizer.js` into a
    /// `<script>` tag in the document head.
    pub(crate) virtualizer_shim_injected: bool,
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
    pub(crate) class_nodes_registered: std::collections::HashSet<u32>,
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
    pub(crate) virtualizer_instances: HashMap<u32, primitives::virtualizer::VirtualizerInstance>,
    /// Monotonic id counter for virtualizer containers, written as
    /// `data-virtualizer-id` on the container `<div>`. Same trick as
    /// `data-graphics-id`: lets `release_virtualizer` look up the
    /// instance from a `&Node` without going through `node_ids`,
    /// which gets cleared by `on_node_unstyled` before our cleanup
    /// hook runs (style effects drop before the virtualizer cleanup
    /// effect within a single `Scope::drop` batch).
    pub(crate) next_virtualizer_id: u32,
    /// Per-Graphics-canvas runtime state — wgpu device, user closures,
    /// pending-paint flag, etc. Keyed by node id so `make_handle` can
    /// look up the matching instance after `create`. The `Rc` is the
    /// shared owner; the handle wraps the same Rc so `request_redraw`
    /// reaches the scheduler with no backend round-trip.
    pub(crate) graphics_instances:
        HashMap<u32, std::rc::Rc<std::cell::RefCell<primitives::graphics::GraphicsInstance>>>,
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
    pub(crate) pregen: HashMap<String, PregenEntry>,
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
    pub(crate) pregen_by_ptr: HashMap<*const runtime_core::StyleRules, String>,
    /// Per-node dynamic class slot — `node_id -> (class_name, content_key)`.
    /// At most one dynamic class per node. Replaced atomically when
    /// the node's resolved style changes.
    pub(crate) dynamic: HashMap<u32, DynamicSlot>,
    /// Content-keyed pool of dynamic CSS rules, refcounted across the
    /// cohort of nodes that resolved to the same `(base + overlays)`
    /// content. Populated lazily on `apply_styled_states` slow-path
    /// misses; collapsed when the last `DynamicSlot` referencing a
    /// key drops. The reactive-style cohort (one signal fanning out
    /// to N styled nodes) is the canonical user — pre-dedupe, every
    /// fan-out minted N identical rules + did N `insert_rule` / N
    /// `delete_rule` calls; deduped, the first node mints and the
    /// rest just bump the refcount.
    pub(crate) dynamic_by_content: HashMap<String, DynamicRule>,
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
    pub(crate) dynamic_by_ptr: HashMap<*const runtime_core::StyleRules, std::rc::Rc<DynamicPtrEntry>>,
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
    pub(crate) asset_urls: HashMap<AssetId, String>,
    /// Ids whose `asset_urls` entry is a `blob:` URL backed by
    /// `URL.createObjectURL` (i.e. `AssetSource::Embedded`). Used by
    /// `unregister_asset` to call `URL.revokeObjectURL` and free the
    /// Blob's backing storage. Bundled / Remote URLs are owned by
    /// the page / CDN — not in this set, never revoked.
    pub(crate) blob_asset_urls: std::collections::HashSet<AssetId>,
    /// Typeface id → indices into the shared `<style>` sheet for the
    /// `@font-face` rules emitted at registration. Lets
    /// `unregister_typeface` reclaim the slots through the regular
    /// `delete_rule` recycle path.
    pub(crate) font_face_rule_indices: HashMap<TypefaceId, Vec<u32>>,
    /// Registry of third-party `Element::External` handlers,
    /// populated by `register_external::<T>(...)` calls from
    /// per-platform leaf crates (e.g. `idealyst-maps-web::register`).
    /// `create_external` looks the handler up by payload TypeId;
    /// unregistered kinds fall through to a "not supported" placeholder.
    pub(crate) external_handlers:
        runtime_core::ExternalRegistry<WebBackend>,
    /// Registry of `Element::Navigator` handler factories,
    /// populated by `register_navigator::<P, _>(...)` calls from
    /// SDK leaf crates (e.g. `stack_navigator::register`).
    /// `create_navigator` looks the factory up by presentation
    /// TypeId; unregistered kinds panic at create time.
    pub(crate) navigator_handlers:
        runtime_core::NavigatorRegistry<WebBackend>,
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
    pub(crate) nav_handler_instances:
        HashMap<u32, std::rc::Rc<std::cell::RefCell<Box<dyn runtime_core::NavigatorHandler<WebBackend>>>>>,
    /// Per-node animated-property state. Tracks the most recent
    /// values written via `Backend::set_animated_f32` /
    /// `set_animated_color` so compound properties like CSS
    /// `translate: <x> <y>` and `scale: <x> <y>` can be re-emitted
    /// without clobbering unrelated axes. See [`animated`] module
    /// for the per-property routing.
    pub(crate) animated_states: animated::AnimatedStateMap,
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
    /// Additional rule indices for per-state pseudo-class overlays
    /// (`.cls:hover`, `:active`, `:focus`, `:disabled`). Empty for
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
    /// `runtime_core::set_viewport_size(...)` with the SSR-assumed viewport
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
            self.hydration_cursor = Self::next_preorder(&el, &self.mount);
            Some(el)
        } else {
            // Mismatch — leave the cursor on the stale node; the next
            // fresh node the caller builds becomes the remount root.
            self.hydration_pending_fresh = true;
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
                     adopt).\n  branch: {}\n  stale SSR node: {}",
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
        Self {
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
            hydration_remount_root: None,
            #[cfg(feature = "hydrate")]
            hydration_remount_stale: None,
            #[cfg(feature = "hydrate")]
            hydration_remount_resume: None,
            _click_closures: Vec::new(),
            _pressable_key_closures: Vec::new(),
            _link_click_closures: Vec::new(),
            _touch_closures: Vec::new(),
            state_listeners: HashMap::new(),
            spinner_keyframes_injected: false,
            virtualizer_shim_injected: false,
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
            class_nodes_registered: std::collections::HashSet::new(),
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
            virtualizer_instances: HashMap::new(),
            next_virtualizer_id: 0,
            graphics_instances: HashMap::new(),
            next_graphics_id: 0,
            style_element: None,
            pregen: HashMap::new(),
            pregen_by_ptr: HashMap::new(),
            dynamic: HashMap::new(),
            dynamic_by_content: HashMap::new(),
            dynamic_by_ptr: HashMap::new(),
            free_rule_indices: Vec::new(),
            theme_root_rule_index: None,
            portal_instances: HashMap::new(),
            next_portal_id: 0,
            asset_urls: HashMap::new(),
            blob_asset_urls: std::collections::HashSet::new(),
            font_face_rule_indices: HashMap::new(),
            external_handlers: runtime_core::ExternalRegistry::new(),
            navigator_handlers: runtime_core::NavigatorRegistry::new(),
            nav_handler_instances: HashMap::new(),
            animated_states: HashMap::new(),
        }
    }

    /// Register a handler for the third-party external primitive
    /// whose payload type is `T`. Called by per-platform leaf crates
    /// (e.g. `idealyst_maps_web::register`) during app bootstrap. The
    /// handler receives the typed payload + a mutable borrow of the
    /// backend and produces the `web_sys::Element` to mount.
    ///
    /// The backend's `Node` type is `web_sys::Node` (the supertype);
    /// we wrap the user's `Element`-returning handler to upcast,
    /// so third-party code can return the natural type without
    /// thinking about the Node/Element distinction.
    pub fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&std::rc::Rc<T>, &mut WebBackend) -> web_sys::Element + 'static,
    {
        self.external_handlers
            .register::<T, _>(move |props, backend| handler(props, backend).into());
    }

    /// Register a navigator-kind handler factory for the per-backend
    /// `NavigatorRegistry`. SDK leaf crates (`stack_navigator::register`,
    /// `tab_navigator::register`, etc.) call this once per app
    /// bootstrap. `P` is the SDK's presentation payload type; the
    /// factory produces a fresh handler per
    /// `Element::Navigator { type_id: TypeId::of::<P>(), .. }`
    /// mounted in the tree.
    pub fn register_navigator<P, F>(&mut self, factory: F)
    where
        P: 'static,
        F: Fn() -> Box<dyn runtime_core::NavigatorHandler<WebBackend>> + 'static,
    {
        self.navigator_handlers.register::<P, _>(factory);
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
        runtime_core::register_signal_js_notifier(sid_raw, move || {
            let value = stringifier();
            if let Some(rc) = weak.upgrade() {
                rc.borrow_mut().ship_signal_change_to_js(sid_raw, &value);
            }
        });
    }

    /// Ship a `(signal_id, new_value)` notification to the JS-side
    /// reactive layer. Single FFI hop — JS handles the per-binding
    /// fan-out internally. Called from the notifier closure
    /// installed by [`Self::register_signal_for_js`].
    fn ship_signal_change_to_js(&mut self, sid_raw: u64, value: &str) {
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
    ///                       [`Backend::create_text_with_id`](runtime_core::Backend::create_text_with_id).
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
    /// [`runtime_core::Backend::register_reactive_text_binding`]
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
            if !runtime_core::signal_has_js_notifier(*sid) {
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
        runtime_core::register_signal_js_notifier(signal_id, move || {
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
        runtime_core::schedule_microtask(move || {
            flag.set(false);
            if let Some(rc) = weak.upgrade() {
                rc.borrow_mut().flush_pending_text();
            }
        });
    }

    /// `true` if a handler for payload type `T` has been registered.
    /// Useful for opt-in graceful degradation in user code (render a
    /// static image if the SDK isn't available on this platform).
    pub fn has_external<T: 'static>(&self) -> bool {
        self.external_handlers.has::<T>()
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
        runtime_core::schedule_microtask(move || {
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
    ///   `HashMap<*const Node, u32>` fast cache had a stale-id
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
        batch: runtime_core::BackendBatch,
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
                runtime_core::BatchOp::CreateView { local_id } => {
                    u32s.extend_from_slice(&[0, *local_id, 0, 0]);
                }
                runtime_core::BatchOp::CreateText { local_id, content } => {
                    if string_count > 0 {
                        strings.push('\0');
                    }
                    strings.push_str(content);
                    u32s.extend_from_slice(&[1, *local_id, 0, string_count]);
                    string_count += 1;
                }
                runtime_core::BatchOp::ApplyStyleStatic {
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
                runtime_core::BatchOp::Insert { parent, child } => {
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

/// Backend-neutral external registration (the `RegisterExternal` trait)
/// so SDKs can `register<B: RegisterExternal>(b)` without naming
/// `WebBackend`. Forwards to the same `external_handlers` registry as the
/// inherent `register_external`; here the handler returns `Self::Node`
/// (a `Node`) directly — the generic-handler shape.
impl runtime_core::RegisterExternal for WebBackend {
    fn register_external<T, F>(&mut self, handler: F)
    where
        T: 'static,
        F: Fn(&std::rc::Rc<T>, &mut WebBackend) -> Node + 'static,
    {
        self.external_handlers.register::<T, _>(handler);
    }
}

impl Backend for WebBackend {
    type Node = Node;

    fn platform(&self) -> runtime_core::Platform {
        runtime_core::Platform::Web
    }

    fn url_opener(&self) -> Option<std::rc::Rc<dyn Fn(&str)>> {
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

    fn color_scheme(&self) -> runtime_core::ColorScheme {
        let window = match self.doc.default_view() {
            Some(w) => w,
            None => return runtime_core::ColorScheme::Auto,
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
            runtime_core::ColorScheme::Dark
        } else if prefers_light {
            runtime_core::ColorScheme::Light
        } else {
            runtime_core::ColorScheme::Auto
        }
    }

    fn create_view(
        &mut self,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::view::create(self);
        a11y::apply(&node, a11y, None);
        node
    }

    fn create_element(&mut self, tag: &str) -> Self::Node {
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

    fn is_hydrating(&self) -> bool {
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
    fn attach_html_id(&self, node: &Self::Node, id: &str) {
        use wasm_bindgen::JsCast;
        if let Some(el) = node.dyn_ref::<web_sys::Element>() {
            let _ = el.set_attribute("id", id);
        }
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        primitives::view::create_reactive_anchor(self)
    }

    fn create_text(
        &mut self,
        content: &str,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::text::create(self, content);
        // Text role has no first-class ARIA equivalent — the helper
        // emits nothing for it. Hint/identifier/live_region still apply.
        a11y::apply(&node, a11y, None);
        node
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &runtime_core::Action,
        leading_icon: Option<&runtime_core::IconData>,
        trailing_icon: Option<&runtime_core::IconData>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node =
            primitives::button::create(self, label, on_click.fire.clone(), leading_icon, trailing_icon);
        // `<button>` has implicit ARIA role; skip inferring one so we
        // don't write `role="button"` redundantly. Author overrides
        // via `props.role` still apply.
        a11y::apply(&node, a11y, None);
        node
    }

    fn create_pressable(
        &mut self,
        on_click: Rc<dyn Fn()>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::pressable::create(self, on_click);
        // Pressable is a `<div>` with click — explicit `role="button"`
        // is what tells the AX walker it's interactive.
        a11y::apply(
            &node,
            a11y,
            Some(runtime_core::accessibility::Role::Button),
        );
        node
    }

    fn install_touch_handler(
        &mut self,
        node: &Self::Node,
        handler: runtime_core::TouchHandler,
    ) {
        primitives::touch::install(self, node, handler);
    }

    // `claim_touch` keeps the default no-op. On web, claims happen
    // inline in the pointer-event listener closure (where we have
    // the live `PointerEvent` to pass to `setPointerCapture`). The
    // trait method exists for symmetry with iOS / Android where the
    // framework dispatches events externally.

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        #[cfg(feature = "hydrate")]
        if self.hydrating {
            // Subtree-remount resync: the fresh remount root is being
            // inserted → swap it in for the stale SSR node *in place*,
            // restore the cursor to the stale node's next sibling, and
            // leave the fresh subtree so siblings adopt again.
            if self
                .hydration_remount_root
                .as_ref()
                .map(|r| r.is_same_node(Some(&child)))
                .unwrap_or(false)
            {
                if let Some(stale) = self.hydration_remount_stale.take() {
                    if let Some(sp) = stale.parent_node() {
                        let _ = sp.replace_child(&child, &stale);
                    } else {
                        let _ = parent.append_child(&child);
                    }
                }
                self.hydration_cursor = self.hydration_remount_resume.take();
                self.hydration_remount_root = None;
                self.hydration_suppress = false;
                return;
            }
            // Outside a remount subtree: adopted nodes are already
            // parent↔child in the SSR DOM, so inserting is a no-op.
            if !self.hydration_suppress {
                if let Some(p) = child.parent_node() {
                    if p.is_same_node(Some(&*parent)) {
                        return;
                    }
                }
            }
            // Inside a remount subtree (suppress, not the root): a fresh
            // node → fall through to a normal append.
        }
        primitives::view::insert(parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        primitives::view::insert_many(self, parent, children)
    }

    // Child-splicing: the DOM does `insertBefore` / `removeChild`
    // directly, so keyed `Each` reconciliation runs in place — unchanged
    // rows keep their nodes (and their render scope), removed rows are
    // detached one-by-one, and reorders move existing nodes rather than
    // rebuilding. Without this the framework falls back to full rebuild.
    fn supports_child_splice(&self) -> bool {
        true
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        primitives::view::insert_at(parent, child, index)
    }

    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        primitives::view::remove_child(parent, child)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        primitives::text::update_text(node, content)
    }

    fn create_text_with_id(
        &mut self,
        content: &str,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Option<(Self::Node, u32)> {
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

    fn update_text_by_id(&mut self, id: u32, content: String) {
        let _t = crate::phase_timer::PhaseTimer::start("text_update_by_id");
        self.append_pending_text(id, |buf| buf.push_str(&content));
        self.schedule_text_flush();
    }

    fn release_text_id(&mut self, id: u32) {
        self.text_release_batch.push(id);
        // Piggy-back on the same flush microtask the updates use.
        // Releases without queued updates are rare (scope teardown
        // without a triggering signal change) but cheap to schedule.
        self.schedule_text_flush();
    }

    fn supports_js_text_bindings(&self) -> bool {
        // True iff the variant has installed the text batcher (which
        // also pre-injects the bindings shim and sets
        // `WEB_BACKEND_HANDLE`). Without that, the signal-change
        // notifier closure has no way back to `&mut self` and the
        // JS-side update path wouldn't fire — better to fall back
        // to the Rust Effect.
        WEB_BACKEND_HANDLE.with(|s| s.borrow().is_some())
    }

    fn register_reactive_text_binding(
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

    fn release_reactive_text_binding(&mut self, text_id: u32) {
        WebBackend::release_reactive_text_binding(self, text_id)
    }

    fn supports_js_class_bindings(&self) -> bool {
        // Same gate as text bindings — both rely on
        // `WEB_BACKEND_HANDLE` being set (the signal-changed notifier
        // needs the self-handle to call back into the backend) and on
        // the shims being injected. Without the handle, the framework
        // falls back to a per-node Effect via the spec's compute
        // closure.
        WEB_BACKEND_HANDLE.with(|s| s.borrow().is_some())
    }

    fn register_reactive_class_binding(
        &mut self,
        node: &Self::Node,
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

    fn release_reactive_class_binding(&mut self, binding_id: u32) {
        WebBackend::release_reactive_class_binding(self, binding_id)
    }

    fn mint_class_for_app(
        &mut self,
        app: &runtime_core::StyleApplication,
    ) -> Option<String> {
        Some(self.impl_mint_class_for_app(app))
    }

    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::image::create(self, src, alt);
        // `<img>` carries implicit `role="img"` — don't infer.
        // `alt` and `a11y.label` both target accessibility text; the
        // helper's aria-label takes precedence when both are set.
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        primitives::image::update_src(self, node, src)
    }

    fn create_icon(
        &mut self,
        data: &runtime_core::primitives::icon::IconData,
        color: Option<&runtime_core::Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::icon::create(self, data, color);
        // SVG icons get explicit `role="img"` since `<svg>` doesn't
        // have an implicit role by default; the helper writes it when
        // the inferred role is supplied.
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::Image));
        node
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &runtime_core::Color) {
        primitives::icon::update_color(node, color)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        primitives::icon::update_stroke(node, progress)
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: runtime_core::Easing,
        infinite: bool,
        _autoreverses: bool,
    ) {
        primitives::icon::animate_stroke(node, from, to, duration_ms, easing, infinite)
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node =
            primitives::text_input::create(self, initial_value, placeholder, on_change, on_key_down);
        // `<input>` has implicit textbox role; no inference needed.
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        primitives::text_input::update_value(node, value)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node =
            primitives::text_area::create(self, initial_value, placeholder, on_change, on_key_down);
        // `<textarea>` is implicitly a multiline textbox; no inference.
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        primitives::text_area::update_value(node, value)
    }

    fn make_text_area_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::text_area::TextAreaHandle {
        primitives::text_area::make_handle(node)
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::toggle::create(self, initial_value, on_change);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::Switch));
        node
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        primitives::toggle::update_value(node, value)
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::scroll_view::create(self, horizontal, on_scroll);
        // ScrollView has no first-class ARIA role — it's a generic
        // container; the platform handles scroll affordances. Author
        // can override.
        a11y::apply(&node, a11y, None);
        node
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::slider::create(self, initial_value, min, max, step, on_change);
        // `<input type=range>` is implicitly `role=slider`; skip inference.
        a11y::apply(&node, a11y, None);
        node
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        primitives::slider::update_value(node, value)
    }

    fn create_activity_indicator(
        &mut self,
        size: runtime_core::primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&runtime_core::Color>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::activity_indicator::create(self, size, color);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::Spinner));
        node
    }

    fn create_virtualizer(
        &mut self,
        callbacks: runtime_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::virtualizer::create(self, callbacks, overscan, horizontal);
        a11y::apply(&node, a11y, Some(runtime_core::accessibility::Role::List));
        node
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        primitives::virtualizer::data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        primitives::virtualizer::release(self, node)
    }

    fn create_graphics(
        &mut self,
        on_ready: runtime_core::primitives::graphics::OnReady,
        on_resize: runtime_core::primitives::graphics::OnResize,
        on_lost: runtime_core::primitives::graphics::OnLost,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::graphics::create(self, on_ready, on_resize, on_lost);
        // `<canvas>` has no implicit ARIA role; author code MUST set
        // `props.label` for screen-reader users to know what's
        // rendered. We don't infer a role here — let author decide
        // (canvas + label is enough; explicit role="img" is also
        // common).
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        primitives::graphics::release(self, node)
    }

    fn make_graphics_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::graphics::GraphicsHandle {
        primitives::graphics::make_handle(self, node)
    }

    //
    // Navigator dispatch routes through the SDK handler stored on
    // `nav_handler_instances` at create time. The handler's
    // attach_initial / release / make_handle / apply_slot_style
    // methods are the kind-specific entry points; web's three
    // first-party SDKs (stack/tab/drawer) all forward to the shared
    // `web-navigator-helpers` crate, but third-party kinds can do
    // whatever DOM work they need without going through the backend.
    // ------------------------------------------------------------------

    fn create_navigator(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        presentation: Rc<dyn std::any::Any>,
        host: runtime_core::NavigatorHost<Self::Node>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Resolve the factory the SDK installed at app-bootstrap time.
        let factory = self
            .navigator_handlers
            .get(type_id)
            .unwrap_or_else(|| {
                panic!(
                    "WebBackend::create_navigator: navigator kind '{}' \
                     is not registered. Did the app forget to call \
                     `<navigator-sdk>::register(&mut backend)` during bootstrap?",
                    type_name
                )
            });
        let mut handler = factory();
        let node = handler.init(self, host, presentation);
        // Stash the handler under the nav id stamped on the container
        // so subsequent dispatch (attach_initial / release / make_handle
        // / apply_slot_style) can find it. The id is set by the SDK
        // (web-navigator-helpers stamps `data-navigator-id`); if the
        // SDK doesn't set one, the handler is simply dropped here —
        // post-create dispatch won't reach it.
        if let Some(id) = nav_id_from_node(&node) {
            self.nav_handler_instances
                .insert(id, std::rc::Rc::new(std::cell::RefCell::new(handler)));
        }
        node
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn std::any::Any>,
    ) {
        let Some(id) = nav_id_from_node(navigator) else { return };
        let handler = self.nav_handler_instances.get(&id).cloned();
        let Some(handler) = handler else { return };
        handler.borrow_mut().attach_initial(self, screen, scope_id, options);
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        let Some(id) = nav_id_from_node(node) else { return };
        let handler = self.nav_handler_instances.remove(&id);
        let Some(handler) = handler else { return };
        handler.borrow_mut().release(self);
    }

    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::NavigatorHandle {
        let handler = nav_id_from_node(node)
            .and_then(|id| self.nav_handler_instances.get(&id).cloned());
        match handler {
            Some(h) => h.borrow().make_handle(),
            None => runtime_core::NavigatorHandle::new(Rc::new(()), &NOOP_NAV_OPS),
        }
    }

    fn apply_navigator_slot_style(
        &mut self,
        navigator: &Self::Node,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let Some(id) = nav_id_from_node(navigator) else { return };
        let handler = self.nav_handler_instances.get(&id).cloned();
        let Some(handler) = handler else { return };
        handler.borrow_mut().apply_slot_style(self, slot, style);
    }

    fn create_link(
        &mut self,
        config: runtime_core::primitives::link::LinkConfig,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::link::create(self, config);
        // `<a>` has implicit role="link"; skip inference.
        a11y::apply(&node, a11y, None);
        node
    }

    fn make_link_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::link::LinkHandle {
        primitives::link::make_handle(node)
    }

    fn create_portal(
        &mut self,
        target: runtime_core::primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let node = primitives::portal::create(self, target, on_dismiss, trap_focus);
        // Portal containers are transparent (the mounted content
        // carries its own role); pass None for inferred role and let
        // author opt into `Dialog`/`AlertDialog` via props.role.
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_portal(&mut self, node: &Self::Node) {
        primitives::portal::release(self, node)
    }

    fn make_portal_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::portal::PortalHandle {
        primitives::portal::make_handle(node)
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
        a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> Self::Node {
        // Look up the handler; clone the Rc so we can drop the
        // registry borrow before calling the handler (which needs
        // `&mut self`).
        let node = if let Some(handler) = self.external_handlers.get(type_id) {
            handler(payload, self)
        } else {
            // No handler registered → render a "not supported" placeholder
            // so the dev/user sees that something is missing rather than
            // a silent hole in the UI.
            external_placeholder_element(&self.doc, type_name).into()
        };
        // External handlers don't know their semantic role; the author
        // supplies it via props.role. No inferred role here.
        a11y::apply(&node, a11y, None);
        node
    }

    fn release_external(&mut self, _node: &Self::Node) {
        // The web backend has no per-external bookkeeping today.
        // Future hooks (e.g. per-instance event-listener cleanup)
        // would land here, queried by `data-external-id` like
        // portals/virtualizers/graphics.
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: runtime_core::PresenceState,
        transition: Option<(u32, runtime_core::Easing)>,
    ) {
        primitives::presence::apply(self, node, state, transition)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        primitives::view::clear_children(node)
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        self.impl_register_stylesheet(rules)
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        self.impl_unregister_stylesheet(rules)
    }

    fn install_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        self.impl_install_theme_variables(tokens)
    }

    fn update_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        // Same machinery handles both — the impl detects whether
        // the :root rule already exists and either inserts or
        // setProperty's.
        self.impl_install_theme_variables(tokens)
    }

    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        self.impl_register_asset(id, kind, source)
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        self.impl_unregister_asset(id, kind)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        self.impl_register_typeface(id, family_name, faces, fallback)
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        self.impl_unregister_typeface(id)
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        self.impl_apply_style(node, style)
    }

    fn set_animated_f32(
        &mut self,
        node: &Self::Node,
        prop: runtime_core::animation::AnimProp,
        value: f32,
    ) {
        self.impl_set_animated_f32(node, prop, value);
    }

    fn set_animated_color(
        &mut self,
        node: &Self::Node,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        self.impl_set_animated_color(node, prop, value);
    }

    /// Opt into the walker's batched-Repeat path. When the walker sees
    /// a `Element::Repeat` whose rows are pure View+Text+static-style,
    /// it builds a [`BackendBatch`] and ships it through
    /// [`execute_batch`] instead of issuing per-row backend calls.
    fn supports_batched_repeat(&self) -> bool {
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
    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
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
    fn execute_batch(&mut self, batch: runtime_core::BackendBatch) -> Vec<Self::Node> {
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
    fn execute_batch_with_attach(
        &mut self,
        batch: runtime_core::BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node> {
        self.execute_batch_inner(batch, Some((parent, attach_locals)))
    }

    /// Web handles interaction states via CSS pseudo-classes
    /// (`:hover`, `:active`, `:focus`, `:disabled`) — the browser
    /// tracks transitions natively and no Rust-side state signal is
    /// needed. The framework calls `apply_styled_states` instead of
    /// `apply_style` when this returns true.
    fn handles_states_natively(&self) -> bool {
        true
    }

    /// Web emits `var(--token, fallback)` for every `Tokenized<T>`
    /// value and `update_tokens` mutates `:root` in place. The
    /// browser's cascade propagates the new values to every node
    /// referencing them — no per-node re-apply needed for theme
    /// value changes. Saves O(N) work per theme swap.
    fn token_updates_propagate_via_cascade(&self) -> bool {
        true
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(runtime_core::StateBits, Rc<StyleRules>)],
    ) {
        self.impl_apply_styled_states(node, base, overlays)
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(runtime_core::StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(runtime_core::Breakpoint, Rc<StyleRules>)],
    ) {
        self.impl_apply_styled_variants(node, base, state_overlays, breakpoint_overlays)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        self.impl_on_node_unstyled(node)
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        // Most disable-able elements (button, input, select) accept
        // the `disabled` attribute. We set/remove it as appropriate.
        // For non-form elements, this is a no-op visually but doesn't
        // hurt.
        let Ok(element) = node.clone().dyn_into::<web_sys::Element>() else {
            return;
        };
        if disabled {
            let _ = element.set_attribute("disabled", "");
        } else {
            let _ = element.remove_attribute("disabled");
        }
    }

    /// Web state styling uses native CSS pseudo-classes (`:hover`,
    /// `:active`, `:focus`, `:disabled`) rather than reactive JS
    /// listeners. That happens at CSS-emit time in `apply_style` (see
    /// `rules_to_css` / pseudo-class rule generation), not here. We
    /// override `attach_states` to a no-op so the framework's
    /// signal-driven state machinery doesn't fire on web.
    ///
    /// Why not listeners + signal-driven re-style? It causes wasm-
    /// bindgen `WasmRefCell` re-entry crashes when DOM events fire
    /// while a style is being applied, and the CSS path is both
    /// simpler and faster (browser tracks the state natively, no
    /// per-event Rust↔JS round trip).
    fn attach_states(
        &mut self,
        _node: &Self::Node,
        _setter: Rc<dyn Fn(runtime_core::StateBits, bool)>,
    ) {
        // intentional no-op on web; CSS pseudo-classes drive states.
    }

    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        primitives::button::make_handle(node)
    }

    fn make_pressable_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::PressableHandle {
        primitives::pressable::make_handle(node)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        // Wrap the actual `web_sys::Node` (not the trait-default
        // `Rc<()>`), so framework helpers like `LayoutPlan` can
        // downcast back to the concrete node and operate on it.
        runtime_core::ViewHandle::new(Rc::new(node.clone()), &WebViewOps)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        // Same plumbing as `make_view_handle` for the text element so
        // author-level animation drivers (welcome's `drive_color_text_av`)
        // can downcast `text_ref.as_any()` to `web_sys::Node` and write
        // `style.color` directly. Without this the typed handle stores
        // the trait-default `Rc<()>` and the downcast silently fails,
        // leaving text color frozen at its stylesheet value.
        runtime_core::TextHandle::new(Rc::new(node.clone()), &WebTextOps)
    }

    fn make_text_input_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::text_input::TextInputHandle {
        primitives::text_input::make_handle(node)
    }

    fn make_scroll_view_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_core::primitives::scroll_view::ScrollViewHandle {
        primitives::scroll_view::make_handle(node)
    }

    fn finish(&mut self, root: Self::Node) {
        #[cfg(feature = "hydrate")]
        if self.hydrating {
            // The initial adoption pass is done; subsequent reactive
            // rebuilds create fresh nodes through the normal path.
            self.hydrating = false;
            crate::scheduler::end_hydration_buffering();
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

    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &runtime_core::accessibility::AccessibilityProps,
        inferred_role: Option<runtime_core::accessibility::Role>,
    ) {
        // Reactive prop updates funnel through here. `a11y::apply` is
        // idempotent and clears attributes that drop to None, so the
        // same code path that builds the DOM also reconciles it.
        a11y::apply(node, a11y, inferred_role);
    }

    fn announce_for_accessibility(
        &mut self,
        msg: &str,
        priority: runtime_core::accessibility::LiveRegionPriority,
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
impl runtime_core::ViewOps for WebViewOps {
    fn rect(&self, node: &dyn std::any::Any) -> runtime_core::ViewportRect {
        match view_rect_from_node(node) {
            Some(r) => r,
            None => runtime_core::ViewportRect::default(),
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
    ) -> Option<runtime_core::primitives::portal::ViewportRect> {
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
        Some(runtime_core::primitives::portal::ViewportRect {
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
    ) -> Option<runtime_core::primitives::portal::ViewportRect> {
        let el = element_from_any(node)?;
        if !el.is_connected() {
            return None;
        }
        let r = el.get_bounding_client_rect();
        Some(runtime_core::primitives::portal::ViewportRect {
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
        prop: runtime_core::animation::AnimProp,
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
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<web_sys::Node>() {
            crate::set_animated_color(n, prop, value);
        }
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
impl runtime_core::TextOps for WebTextOps {
    fn set_animated_color(
        &self,
        node: &dyn std::any::Any,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<web_sys::Node>() {
            crate::set_animated_color(n, prop, value);
        }
    }
}

fn view_rect_from_node(node: &dyn std::any::Any) -> Option<runtime_core::ViewportRect> {
    let el = element_from_any(node)?;
    let r = el.get_bounding_client_rect();
    Some(runtime_core::ViewportRect {
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
