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
//! - `primitives/` — one module per `Primitive` kind. Each owns its
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

mod animated;
#[cfg(feature = "async-driver")]
pub mod async_executor;
mod assets;
mod defaults;
#[cfg(feature = "aas-shell")]
pub mod dev_transport;
mod phase_timer;
mod primitives;
#[cfg(feature = "async-driver")]
pub mod render_loop;
pub mod scheduler;
mod style;
pub mod time_source;

#[cfg(feature = "async-driver")]
pub use async_executor::install_async_executor;
#[cfg(feature = "aas-shell")]
pub use dev_transport::{connect_web, WebClientHandle};
#[cfg(feature = "async-driver")]
pub use render_loop::install_render_loop;
pub use scheduler::install_scheduler;
pub use time_source::install_time_source;

/// Install a self-handle so the batched text-update path
/// ([`Backend::create_text_with_id`] / [`Backend::update_text_by_id`])
/// can schedule its microtask flush. Must be called once after the
/// app's `Rc<RefCell<WebBackend>>` is constructed; if it's never
/// called, `create_text_with_id` returns `None` and the framework
/// falls back to the unbatched `update_text` path automatically.
///
/// The handle is held as a `Weak` so the backend Rc still drops
/// cleanly on app teardown — once it drops, queued microtasks
/// upgrade to `None` and become no-ops.
pub fn install_text_batcher(backend: &std::rc::Rc<std::cell::RefCell<WebBackend>>) {
    WEB_BACKEND_HANDLE.with(|s| *s.borrow_mut() = Some(std::rc::Rc::downgrade(backend)));
    // Pre-inject the JS-side reactive-binding shim so it's
    // available for console-driven smoke tests (`__idealystBindingsSmokeTest()`)
    // before any text binding is actually registered through the
    // framework. Cheap (~0.5 ms for the eval); same pattern as
    // the batched-text shim's lazy injection on first use, just
    // pulled forward.
    backend.borrow_mut().ensure_text_bindings_shim();
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
}

use framework_core::{
    AssetId, AssetSource, AssetTag, Backend, ButtonHandle, StyleRules, SystemFallback,
    TypefaceFace, TypefaceId,
};
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Document, Node};

pub struct WebBackend {
    pub(crate) doc: Document,
    pub(crate) mount: web_sys::Element,
    pub(crate) _click_closures: Vec<Closure<dyn FnMut()>>,
    /// Keyboard handlers for `Primitive::Pressable` (Enter/Space →
    /// click). Held so JS doesn't drop them while the element is in
    /// the layout tree. The click handler itself lives in
    /// `_click_closures` (shared shape: `FnMut()` no-arg).
    pub(crate) _pressable_key_closures: Vec<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
    /// Closures attached to `<a>` elements for `Primitive::Link`.
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
    /// been injected? First batched `Primitive::Repeat` triggers
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
    /// Cached handle to `window.__idealystUpdateTextBatch`. Set on
    /// first flush.
    pub(crate) text_update_batch_fn: Option<js_sys::Function>,
    /// Cached handle to `window.__idealystRegisterText`. Set on first
    /// `create_text_with_id` call.
    pub(crate) text_register_fn: Option<js_sys::Function>,
    /// Cached handle to `window.__idealystReleaseText`. Set on first
    /// `release_text_id` call. (Releases happen at scope-teardown
    /// time; they're rare enough that lazy lookup is fine.)
    pub(crate) text_release_fn: Option<js_sys::Function>,
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
    /// Pending text-update ids accumulated since the last flush.
    /// Parallel to [`Self::pending_text_lengths`] — the i-th
    /// segment in [`Self::pending_text_buffer`] has length
    /// `pending_text_lengths[i]` and updates the node at
    /// `pending_text_ids[i]`. Drained by
    /// [`Self::flush_pending_text`].
    pub(crate) pending_text_ids: Vec<u32>,
    /// UTF-16 code-unit length of each pending segment. The JS
    /// shim walks the joined buffer with `substring(offset,
    /// offset+len)`, which uses UTF-16 indices; tracking lengths
    /// in code units (not bytes) keeps the offsets correct when
    /// the buffer contains non-ASCII content. For ASCII (the
    /// common case — bench leaves, simple labels), code-unit
    /// length equals byte length and the computation is O(1).
    pub(crate) pending_text_lengths: Vec<u32>,
    /// One growing UTF-8 buffer containing every pending update's
    /// new content, segments back-to-back (NO separator — the JS
    /// shim walks them with the parallel-array `pending_text_lengths`
    /// instead of splitting on a sentinel).
    ///
    /// Cleared (not dropped) after each flush so its capacity
    /// survives across flushes — at hierarchy scale the buffer
    /// stabilizes at the size of the largest fan-out and never
    /// re-allocs.
    pub(crate) pending_text_buffer: String,
    /// Pending releases (ids whose registry slot should be cleared).
    /// Drained alongside `pending_text_ids` at flush time.
    pub(crate) pending_text_releases: Vec<u32>,
    /// `true` while a microtask flush is queued. Coalesces many
    /// `update_text_by_id` calls in the same synchronous turn into a
    /// single flush. Cleared by the flush microtask before it runs
    /// (so any updates queued *during* the flush schedule another
    /// flush rather than getting lost).
    pub(crate) text_flush_scheduled: std::rc::Rc<std::cell::Cell<bool>>,
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
    /// Per-Navigator state. Keyed by the navigator id stamped on the
    /// container's `data-navigator-id` attribute so `make_handle` and
    /// `release_navigator` can find the right entry on lookup.
    pub(crate) navigator_instances: primitives::navigator::NavigatorInstances,
    /// Monotonic id counter for navigator containers. Same pattern as
    /// `next_graphics_id` — written as a data attribute on the
    /// container element.
    pub(crate) next_navigator_id: u32,
    /// Has the navigator CSS (`.ui-nav-root` + show/hide rules) been
    /// injected this session? Idempotent; first navigator create
    /// stamps it.
    pub(crate) navigator_css_injected: bool,
    /// Monotonic id counter for Graphics canvases. Written as the
    /// `data-graphics-id` attribute on each `<canvas>` so
    /// `make_handle` / `release` can look the instance up from a
    /// fresh `&Node` after the create call returned. Distinct from
    /// `next_node_id` — that one is keyed by Rust pointer identity,
    /// which doesn't survive return-by-value.
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
    pub(crate) pregen_by_ptr: HashMap<*const framework_core::StyleRules, String>,
    /// Per-node dynamic class slot — `node_id -> (class_name, rule_index)`.
    /// At most one dynamic class per node. Replaced atomically when
    /// the node's resolved style changes.
    pub(crate) dynamic: HashMap<u32, DynamicSlot>,
    /// Stable per-Node id derived from the Node's pointer.
    pub(crate) next_node_id: u32,
    pub(crate) node_ids: HashMap<*const web_sys::Node, u32>,
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
    /// Registry of third-party `Primitive::External` handlers,
    /// populated by `register_external::<T>(...)` calls from
    /// per-platform leaf crates (e.g. `idealyst-maps-web::register`).
    /// `create_external` looks the handler up by payload TypeId;
    /// unregistered kinds fall through to a "not supported" placeholder.
    pub(crate) external_handlers:
        framework_core::ExternalRegistry<WebBackend>,
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
    pub node_ids: usize,
    pub dynamic: usize,
    pub state_listeners: usize,
    pub pregen: usize,
    pub pregen_by_ptr: usize,
    pub free_rule_indices: usize,
    pub next_node_id: u32,
}

pub(crate) struct PregenEntry {
    #[allow(dead_code)]
    pub(crate) name: String,
    pub(crate) rule_index: u32,
    pub(crate) refcount: u32,
}

pub(crate) struct DynamicSlot {
    /// Kept for debugging — same hash that's set on the element's class.
    #[allow(dead_code)]
    pub(crate) name: String,
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
            text_update_batch_fn: None,
            text_register_fn: None,
            text_release_fn: None,
            signal_changed_fn: None,
            binding_register_fn: None,
            binding_release_fn: None,
            next_text_id: 0,
            // Pre-allocate the pending buffers so the first
            // fan-out at hierarchy scale doesn't pay ~12 grow-
            // reallocs walking from cap 0 up to a few thousand
            // entries. 256 covers most apps' steady-state fan-
            // outs in a single allocation; beyond that, the
            // doubling growth kicks in as normal.
            pending_text_ids: Vec::with_capacity(256),
            pending_text_lengths: Vec::with_capacity(256),
            // 8 KiB initial buffer covers most fan-outs without
            // grow-realloc. The buffer stabilizes after a few
            // flushes at the size of the largest fan-out.
            pending_text_buffer: String::with_capacity(8192),
            pending_text_releases: Vec::with_capacity(64),
            text_flush_scheduled: std::rc::Rc::new(std::cell::Cell::new(false)),
            virtualizer_instances: HashMap::new(),
            next_virtualizer_id: 0,
            graphics_instances: HashMap::new(),
            next_graphics_id: 0,
            navigator_instances: HashMap::new(),
            next_navigator_id: 0,
            navigator_css_injected: false,
            style_element: None,
            pregen: HashMap::new(),
            pregen_by_ptr: HashMap::new(),
            dynamic: HashMap::new(),
            next_node_id: 0,
            node_ids: HashMap::new(),
            free_rule_indices: Vec::new(),
            theme_root_rule_index: None,
            portal_instances: HashMap::new(),
            next_portal_id: 0,
            asset_urls: HashMap::new(),
            blob_asset_urls: std::collections::HashSet::new(),
            font_face_rule_indices: HashMap::new(),
            external_handlers: framework_core::ExternalRegistry::new(),
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
        framework_core::register_signal_js_notifier(sid_raw, move || {
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
    ///                       [`Backend::create_text_with_id`](framework_core::Backend::create_text_with_id).
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
    /// **Each signal in `signal_ids` must have had
    /// [`Self::register_signal_for_js`] called on it first** —
    /// otherwise the binding registers but no signal change will
    /// ever flow through to update it. This split (register signal
    /// once, register binding per text node) avoids re-registering
    /// the same per-signal stringifier on every binding.
    pub fn register_reactive_text_binding(
        &mut self,
        text_id: u32,
        signal_ids: &[u64],
        template_parts: &[&str],
        initial_values: &[&str],
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
        self.ensure_text_bindings_shim();
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
        use wasm_bindgen::JsValue;
        let _t_total = crate::phase_timer::PhaseTimer::start("text_flush_total");

        // Process unregisters first as one batched FFI call. Scope
        // teardown can drop 2 k+ text effects at once (e.g. a
        // switch-arm flip in the hierarchy bench); ship them as a
        // single `Uint32Array` to `__idealystReleaseTextBatch`
        // instead of one `call1` per id.
        if !self.pending_text_releases.is_empty() {
            if self.text_release_fn.is_none() {
                let window = web_sys::window().expect("no window");
                let f_val = js_sys::Reflect::get(
                    &window,
                    &JsValue::from_str("__idealystReleaseTextBatch"),
                )
                .expect("Reflect::get for __idealystReleaseTextBatch failed");
                self.text_release_fn = Some(
                    f_val
                        .dyn_into::<js_sys::Function>()
                        .expect(
                            "__idealystReleaseTextBatch is not a Function — shim missing",
                        ),
                );
            }
            let release_ids = std::mem::take(&mut self.pending_text_releases);
            let release_buf = js_sys::Uint32Array::from(&release_ids[..]);
            let _ = self
                .text_release_fn
                .as_ref()
                .expect("set above")
                .call1(&JsValue::NULL, &release_buf)
                .expect("__idealystReleaseTextBatch call failed");
            // Restore the empty Vec so its allocation survives.
            self.pending_text_releases = release_ids;
            self.pending_text_releases.clear();
        }

        let n = self.pending_text_ids.len();
        if n == 0 {
            return;
        }

        // Lazily resolve + cache the JS-side batch fn. First flush
        // pays the lookup; subsequent flushes reuse the handle.
        if self.text_update_batch_fn.is_none() {
            self.ensure_text_batch_shim();
            let window = web_sys::window().expect("no window");
            let f_val = js_sys::Reflect::get(
                &window,
                &JsValue::from_str("__idealystUpdateTextBatch"),
            )
            .expect("Reflect::get for __idealystUpdateTextBatch failed");
            self.text_update_batch_fn = Some(
                f_val
                    .dyn_into::<js_sys::Function>()
                    .expect("__idealystUpdateTextBatch is not a Function — shim missing"),
            );
        }

        // Three FFI args: ids, lengths, big string buffer. The JS
        // shim walks `bigString.substring(offset, offset+len)` per
        // entry — `substring` is O(1) (creates a SlicedString) so
        // there's no upfront split-and-allocate cost like the
        // previous NUL-separated design had. The three Vec/String
        // are `take()`-d so we can do the FFI calls without holding
        // `&mut self.pending_*`, then restored as empty so their
        // capacity survives the next fan-out.
        let ids = std::mem::take(&mut self.pending_text_ids);
        let lengths = std::mem::take(&mut self.pending_text_lengths);
        let buffer = std::mem::take(&mut self.pending_text_buffer);

        let (ids_buf, lengths_buf, strings_buf) = {
            let _t = crate::phase_timer::PhaseTimer::start("text_flush_marshal");
            let ids_buf = js_sys::Uint32Array::from(&ids[..]);
            let lengths_buf = js_sys::Uint32Array::from(&lengths[..]);
            let strings_buf = JsValue::from_str(&buffer);
            (ids_buf, lengths_buf, strings_buf)
        };

        {
            let _t = crate::phase_timer::PhaseTimer::start("text_flush_ffi_call");
            let _ = self
                .text_update_batch_fn
                .as_ref()
                .expect("set above")
                .call3(&JsValue::NULL, &ids_buf, &lengths_buf, &strings_buf)
                .expect("__idealystUpdateTextBatch call failed");
        }

        self.pending_text_ids = ids;
        self.pending_text_ids.clear();
        self.pending_text_lengths = lengths;
        self.pending_text_lengths.clear();
        self.pending_text_buffer = buffer;
        self.pending_text_buffer.clear();
    }

    /// Append a `(id, content)` pending entry. Bytes are written
    /// straight into the shared `pending_text_buffer` via
    /// `write_fn`; the segment's UTF-16 code-unit length is pushed
    /// into a parallel `pending_text_lengths` Vec so the JS shim
    /// can slice it back out with `substring(offset, offset+len)`.
    ///
    /// Why length-prefixed (not NUL-separated): the JS shim used
    /// to receive one big NUL-joined string and `split('\0')` it
    /// — at 20 k segments per flush, that split allocated 20 k
    /// JS String objects up front and burned ~2-5 ms per flush.
    /// `substring` is O(1) (creates a SlicedString view), so
    /// length-prefixed traversal does the same total work
    /// distributed across the per-segment loop with no upfront
    /// allocation.
    ///
    /// Why UTF-16 lengths: `substring` uses UTF-16 code units. For
    /// ASCII content (bench leaves, simple labels) UTF-8 bytes ==
    /// UTF-16 code units so the computation is O(1) via the
    /// `is_ascii()` fast path; for non-ASCII we walk the new
    /// segment once with `chars().map(c.len_utf16()).sum()`. Both
    /// paths produce a length the JS shim can use directly.
    pub(crate) fn append_pending_text<F: FnOnce(&mut String)>(
        &mut self,
        id: u32,
        write_fn: F,
    ) {
        let start_byte = self.pending_text_buffer.len();
        write_fn(&mut self.pending_text_buffer);
        let new_segment = &self.pending_text_buffer[start_byte..];
        let utf16_len: u32 = if new_segment.is_ascii() {
            new_segment.len() as u32
        } else {
            new_segment.chars().map(|c| c.len_utf16() as u32).sum()
        };
        self.pending_text_lengths.push(utf16_len);
        self.pending_text_ids.push(id);
    }

    /// Schedule a microtask-driven flush of `pending_text_*`. No-op
    /// if a flush is already scheduled this turn — the existing
    /// microtask will drain everything queued by the time it runs.
    /// Clearing the `text_flush_scheduled` flag inside the
    /// microtask (before `flush_pending_text` runs) means updates
    /// queued *during* the flush re-schedule a fresh microtask
    /// rather than getting lost.
    fn schedule_text_flush(&self) {
        if self.text_flush_scheduled.get() {
            return;
        }
        self.text_flush_scheduled.set(true);
        let weak = WEB_BACKEND_HANDLE
            .with(|s| s.borrow().clone())
            .expect("WEB_BACKEND_HANDLE must be set when batched text path is active");
        let flag = self.text_flush_scheduled.clone();
        framework_core::schedule_microtask(move || {
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
            node_ids: self.node_ids.len(),
            dynamic: self.dynamic.len(),
            state_listeners: self.state_listeners.len(),
            pregen: self.pregen.len(),
            pregen_by_ptr: self.pregen_by_ptr.len(),
            free_rule_indices: self.free_rule_indices.len(),
            next_node_id: self.next_node_id,
        }
    }

    /// Assigns a stable per-Node id we use as a key in `dynamic`.
    pub(crate) fn node_id(&mut self, node: &Node) -> u32 {
        let p: *const web_sys::Node = node;
        if let Some(&id) = self.node_ids.get(&p) {
            return id;
        }
        let id = self.next_node_id;
        self.next_node_id += 1;
        self.node_ids.insert(p, id);
        id
    }
}

// ---------------------------------------------------------------------------
// Backend trait impl. Each method delegates to the matching primitive
// module (or to one of the style/defaults helpers on `WebBackend`).
// Keep this thin — anything substantial belongs in the primitive's file.
// ---------------------------------------------------------------------------

impl Backend for WebBackend {
    type Node = Node;

    fn color_scheme(&self) -> framework_core::ColorScheme {
        let window = match self.doc.default_view() {
            Some(w) => w,
            None => return framework_core::ColorScheme::Auto,
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
            framework_core::ColorScheme::Dark
        } else if prefers_light {
            framework_core::ColorScheme::Light
        } else {
            framework_core::ColorScheme::Auto
        }
    }

    fn create_view(&mut self) -> Self::Node {
        primitives::view::create(self)
    }

    fn create_reactive_anchor(&mut self) -> Self::Node {
        primitives::view::create_reactive_anchor(self)
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
        primitives::text::create(self, content)
    }

    fn create_button(
        &mut self,
        label: &str,
        on_click: &framework_core::Action,
        leading_icon: Option<&framework_core::IconData>,
        trailing_icon: Option<&framework_core::IconData>,
    ) -> Self::Node {
        primitives::button::create(self, label, on_click.fire.clone(), leading_icon, trailing_icon)
    }

    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>) -> Self::Node {
        primitives::pressable::create(self, on_click)
    }

    fn install_touch_handler(
        &mut self,
        node: &Self::Node,
        handler: framework_core::TouchHandler,
    ) {
        primitives::touch::install(self, node, handler);
    }

    // `claim_touch` keeps the default no-op. On web, claims happen
    // inline in the pointer-event listener closure (where we have
    // the live `PointerEvent` to pass to `setPointerCapture`). The
    // trait method exists for symmetry with iOS / Android where the
    // framework dispatches events externally.

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        primitives::view::insert(parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        primitives::view::insert_many(self, parent, children)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        primitives::text::update_text(node, content)
    }

    fn create_text_with_id(&mut self, content: &str) -> Option<(Self::Node, u32)> {
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
        let (span, inner_text) = primitives::text::create_with_inner_text(self, content);
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
        Some((span, id))
    }

    fn update_text_by_id(&mut self, id: u32, content: String) {
        let _t = crate::phase_timer::PhaseTimer::start("text_update_by_id");
        self.append_pending_text(id, |buf| buf.push_str(&content));
        self.schedule_text_flush();
    }

    fn release_text_id(&mut self, id: u32) {
        self.pending_text_releases.push(id);
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
        )
    }

    fn release_reactive_text_binding(&mut self, text_id: u32) {
        WebBackend::release_reactive_text_binding(self, text_id)
    }

    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node {
        primitives::image::create(self, src, alt)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        primitives::image::update_src(self, node, src)
    }

    fn create_icon(
        &mut self,
        data: &framework_core::primitives::icon::IconData,
        color: Option<&framework_core::Color>,
    ) -> Self::Node {
        primitives::icon::create(self, data, color)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &framework_core::Color) {
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
        easing: framework_core::Easing,
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
    ) -> Self::Node {
        primitives::text_input::create(self, initial_value, placeholder, on_change)
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        primitives::text_input::update_value(node, value)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
    ) -> Self::Node {
        primitives::text_area::create(self, initial_value, placeholder, on_change)
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        primitives::text_area::update_value(node, value)
    }

    fn make_text_area_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::text_area::TextAreaHandle {
        primitives::text_area::make_handle(node)
    }

    fn create_code_block(
        &mut self,
        spans: &[(String, framework_core::Color)],
    ) -> Self::Node {
        primitives::code_block::create(self, spans)
    }

    fn update_code_block_spans(
        &mut self,
        node: &Self::Node,
        spans: &[(String, framework_core::Color)],
    ) {
        primitives::code_block::update_spans(self, node, spans)
    }

    fn create_toggle(&mut self, initial_value: bool, on_change: Rc<dyn Fn(bool)>) -> Self::Node {
        primitives::toggle::create(self, initial_value, on_change)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        primitives::toggle::update_value(node, value)
    }

    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
        primitives::scroll_view::create(self, horizontal)
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    ) -> Self::Node {
        primitives::slider::create(self, initial_value, min, max, step, on_change)
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        primitives::slider::update_value(node, value)
    }

    fn create_web_view(&mut self, url: &str) -> Self::Node {
        primitives::web_view::create(self, url)
    }

    fn update_web_view_url(&mut self, node: &Self::Node, url: &str) {
        primitives::web_view::update_url(node, url)
    }

    fn web_view_set_on_message(
        &mut self,
        node: &Self::Node,
        callback: Box<dyn Fn(String)>,
    ) {
        primitives::web_view::set_on_message(node, callback)
    }

    fn web_view_set_on_load(
        &mut self,
        node: &Self::Node,
        callback: Box<dyn Fn()>,
    ) {
        primitives::web_view::set_on_load(node, callback)
    }

    fn web_view_set_on_error(
        &mut self,
        node: &Self::Node,
        callback: Box<dyn Fn()>,
    ) {
        primitives::web_view::set_on_error(node, callback)
    }

    fn make_web_view_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::web_view::WebViewHandle {
        primitives::web_view::make_handle(node)
    }

    fn create_video(
        &mut self,
        src: &str,
        autoplay: bool,
        controls: bool,
        loop_playback: bool,
    ) -> Self::Node {
        primitives::video::create(self, src, autoplay, controls, loop_playback)
    }

    fn update_video_src(&mut self, node: &Self::Node, src: &str) {
        primitives::video::update_src(node, src)
    }

    fn create_activity_indicator(
        &mut self,
        size: framework_core::primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&framework_core::Color>,
    ) -> Self::Node {
        primitives::activity_indicator::create(self, size, color)
    }

    fn create_virtualizer(
        &mut self,
        callbacks: framework_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
    ) -> Self::Node {
        primitives::virtualizer::create(self, callbacks, overscan, horizontal)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        primitives::virtualizer::data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        primitives::virtualizer::release(self, node)
    }

    fn create_graphics(
        &mut self,
        on_ready: framework_core::primitives::graphics::OnReady,
        on_resize: framework_core::primitives::graphics::OnResize,
        on_lost: framework_core::primitives::graphics::OnLost,
    ) -> Self::Node {
        primitives::graphics::create(self, on_ready, on_resize, on_lost)
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        primitives::graphics::release(self, node)
    }

    fn make_graphics_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::graphics::GraphicsHandle {
        primitives::graphics::make_handle(self, node)
    }

    fn create_navigator(
        &mut self,
        callbacks: framework_core::NavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::NavigatorControl>,
    ) -> Self::Node {
        primitives::navigator::create(self, callbacks, control)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        _options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        // The framework's local-mode path runs the initial mount
        // via the microtask in `create_navigator` and never calls
        // this method directly (the trait default is a no-op).
        // AAS mode is the opposite: the create-time microtask
        // bails early on `defer_initial_mount = true`, and this
        // method is the one that actually mounts the screen,
        // using the wire-supplied DOM subtree + scope id.
        primitives::navigator::attach_initial(self, navigator, screen, scope_id)
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        primitives::navigator::release(self, node)
    }

    fn make_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::NavigatorHandle {
        primitives::navigator::make_handle(self, node)
    }

    // On web every navigator kind reduces to the same underlying
    // screen-swap-plus-layout machinery — the layout slot is where
    // tab bars and drawer panels actually render. So tab + drawer
    // creation just dispatches into the existing instance code with
    // a kind-appropriate command dispatcher; teardown reuses
    // `release` because the entry shape is identical.
    fn create_tab_navigator(
        &mut self,
        callbacks: framework_core::TabNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::NavigatorControl>,
    ) -> Self::Node {
        primitives::navigator::create_tab(self, callbacks, control)
    }

    fn release_tab_navigator(&mut self, node: &Self::Node) {
        primitives::navigator::release(self, node)
    }

    fn make_tab_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::TabsHandle {
        primitives::navigator::make_tab_handle(self, node)
    }

    fn tab_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        _options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        // Same wire-driven mount story as `navigator_attach_initial`,
        // and on web the three navigator kinds share one
        // `NavigatorInstance` machine — so route through the same
        // helper. Without this override the trait default eats the
        // command silently and the home screen never lands in the DOM.
        primitives::navigator::attach_initial(self, navigator, screen, scope_id)
    }

    fn create_drawer_navigator(
        &mut self,
        callbacks: framework_core::DrawerNavigatorCallbacks<Self::Node>,
        control: Rc<framework_core::NavigatorControl>,
    ) -> Self::Node {
        primitives::navigator::create_drawer(self, callbacks, control)
    }

    fn release_drawer_navigator(&mut self, node: &Self::Node) {
        primitives::navigator::release(self, node)
    }

    fn make_drawer_navigator_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::DrawerHandle {
        primitives::navigator::make_drawer_handle(self, node)
    }

    fn drawer_navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        _options: framework_core::primitives::navigator::ScreenOptions,
    ) {
        // See `tab_navigator_attach_initial`. The trait default is a
        // no-op, which is why AAS-driven drawer apps were rendering a
        // fully empty `.ui-nav-root` — `Command::NavigatorAttachInitial`
        // dispatched to drawer_navigator_attach_initial, the default
        // ate it, and the home screen never reached the DOM.
        primitives::navigator::attach_initial(self, navigator, screen, scope_id)
    }

    fn attach_navigator_layout(
        &mut self,
        navigator: &Self::Node,
        root: Self::Node,
        outlet: Self::Node,
    ) {
        primitives::navigator::attach_layout(self, navigator, root, outlet)
    }

    fn create_link(
        &mut self,
        config: framework_core::primitives::link::LinkConfig,
    ) -> Self::Node {
        primitives::link::create(self, config)
    }

    fn make_link_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::link::LinkHandle {
        primitives::link::make_handle(node)
    }

    fn create_portal(
        &mut self,
        target: framework_core::primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
    ) -> Self::Node {
        primitives::portal::create(self, target, on_dismiss, trap_focus)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        primitives::portal::release(self, node)
    }

    fn make_portal_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::portal::PortalHandle {
        primitives::portal::make_handle(node)
    }

    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn std::any::Any>,
    ) -> Self::Node {
        // Look up the handler; clone the Rc so we can drop the
        // registry borrow before calling the handler (which needs
        // `&mut self`).
        if let Some(handler) = self.external_handlers.get(type_id) {
            return handler(payload, self);
        }
        // No handler registered → render a "not supported" placeholder
        // so the dev/user sees that something is missing rather than
        // a silent hole in the UI.
        external_placeholder_element(&self.doc, type_name).into()
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
        state: framework_core::PresenceState,
        transition: Option<(u32, framework_core::Easing)>,
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

    fn install_tokens(&mut self, tokens: &[framework_core::TokenEntry]) {
        self.impl_install_theme_variables(tokens)
    }

    fn update_tokens(&mut self, tokens: &[framework_core::TokenEntry]) {
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
        prop: framework_core::animation::AnimProp,
        value: f32,
    ) {
        self.impl_set_animated_f32(node, prop, value);
    }

    fn set_animated_color(
        &mut self,
        node: &Self::Node,
        prop: framework_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        self.impl_set_animated_color(node, prop, value);
    }

    /// Opt into the walker's batched-Repeat path. When the walker sees
    /// a `Primitive::Repeat` whose rows are pure View+Text+static-style,
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
    fn execute_batch(&mut self, batch: framework_core::BackendBatch) -> Vec<Self::Node> {
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
        //
        // Why flat: the previous per-op `js_sys::Array::push` path
        // crossed the wasm→JS boundary ~3 times per op (about ~12k
        // crossings for a 4000-op batch). Flat shipping is **two**
        // input FFI calls regardless of op count: one `Uint32Array`
        // copy + one big-string transfer. The shim does all decoding
        // inside the JS function call.
        let _t_encode = crate::phase_timer::PhaseTimer::start("execute_batch_encode");
        let mut u32s: Vec<u32> = Vec::with_capacity(batch.ops.len() * 4);
        // Rough upper bound: ~16 chars per string × one string per
        // op. Over-allocates by ~2x for the rebuild workload, which
        // is cheap relative to per-byte realloc costs.
        let mut strings: String = String::with_capacity(batch.ops.len() * 16);
        let mut string_count: u32 = 0;
        for op in batch.ops.iter() {
            match op {
                framework_core::BatchOp::CreateView { local_id } => {
                    u32s.extend_from_slice(&[0, *local_id, 0, 0]);
                }
                framework_core::BatchOp::CreateText { local_id, content } => {
                    if string_count > 0 {
                        strings.push('\0');
                    }
                    strings.push_str(content);
                    u32s.extend_from_slice(&[1, *local_id, 0, string_count]);
                    string_count += 1;
                }
                framework_core::BatchOp::ApplyStyleStatic {
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
                framework_core::BatchOp::Insert { parent, child } => {
                    u32s.extend_from_slice(&[3, *parent, *child, 0]);
                }
            }
        }
        // Single FFI: copies the u32 slice's bytes into a fresh JS
        // `Uint32Array` via the wasm memory view. The whole buffer
        // moves in one operation regardless of op count.
        let u32_buf = js_sys::Uint32Array::from(&u32s[..]);
        // Single FFI: transfers the concatenated string buffer.
        let strings_buf = JsValue::from_str(&strings);
        drop(_t_encode);

        let _t_ffi = crate::phase_timer::PhaseTimer::start("execute_batch_ffi_call");
        let f = self.batch_fn.as_ref().expect("batch_fn set above");
        let node_count_val = JsValue::from(batch.node_count);
        let result = f
            .call3(&JsValue::NULL, &u32_buf, &strings_buf, &node_count_val)
            .expect("__idealystExecuteBatch call failed");
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
        overlays: &[(framework_core::StateBits, Rc<StyleRules>)],
    ) {
        self.impl_apply_styled_states(node, base, overlays)
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
        _setter: Rc<dyn Fn(framework_core::StateBits, bool)>,
    ) {
        // intentional no-op on web; CSS pseudo-classes drive states.
    }

    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        primitives::button::make_handle(node)
    }

    fn make_pressable_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::PressableHandle {
        primitives::pressable::make_handle(node)
    }

    fn make_view_handle(&self, node: &Self::Node) -> framework_core::ViewHandle {
        // Wrap the actual `web_sys::Node` (not the trait-default
        // `Rc<()>`), so framework helpers like `LayoutPlan` can
        // downcast back to the concrete node and operate on it.
        framework_core::ViewHandle::new(Rc::new(node.clone()), &WebViewOps)
    }

    fn make_text_input_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::text_input::TextInputHandle {
        primitives::text_input::make_handle(node)
    }

    fn make_scroll_view_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::scroll_view::ScrollViewHandle {
        primitives::scroll_view::make_handle(node)
    }

    fn make_video_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::video::VideoHandle {
        primitives::video::make_handle(node)
    }

    fn finish(&mut self, root: Self::Node) {
        self.mount
            .append_child(&root)
            .expect("mount append failed");
    }
}

/// Marker ops for `ViewHandle`. Views don't have methods yet (no
/// scroll, no measure) — the trait is reserved for future
/// additions. We still need an instance to satisfy
/// `ViewHandle::new`'s `&'static dyn ViewOps` parameter.
struct WebViewOps;
impl framework_core::ViewOps for WebViewOps {
    fn rect(&self, node: &dyn std::any::Any) -> framework_core::ViewportRect {
        match view_rect_from_node(node) {
            Some(r) => r,
            None => framework_core::ViewportRect::default(),
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
    ) -> Option<framework_core::primitives::portal::ViewportRect> {
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
        Some(framework_core::primitives::portal::ViewportRect {
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
    ) -> Option<framework_core::primitives::portal::ViewportRect> {
        let el = element_from_any(node)?;
        if !el.is_connected() {
            return None;
        }
        let r = el.get_bounding_client_rect();
        Some(framework_core::primitives::portal::ViewportRect {
            x: r.x() as f32,
            y: r.y() as f32,
            width: r.width() as f32,
            height: r.height() as f32,
        })
    }
}

fn element_from_any(node: &dyn std::any::Any) -> Option<web_sys::Element> {
    let n = node.downcast_ref::<web_sys::Node>()?;
    n.clone().dyn_into::<web_sys::Element>().ok()
}

fn view_rect_from_node(node: &dyn std::any::Any) -> Option<framework_core::ViewportRect> {
    let el = element_from_any(node)?;
    let r = el.get_bounding_client_rect();
    Some(framework_core::ViewportRect {
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
