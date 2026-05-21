//! Global stylesheet baselines: the `.ui-default` class every node
//! gets, the spinner keyframes, the virtualizer JS shim, and the
//! per-node dynamic-slot teardown helper.
//!
//! These live in their own `impl WebBackend` block so they're separate
//! from per-primitive create code and from the CSS converter helpers
//! in [`crate::style`].

use crate::WebBackend;
use wasm_bindgen::prelude::*;

impl WebBackend {
    // The framework used to stamp every framework-created element
    // with `class="ui-default"` and inject a
    // `.ui-default { display: flex; flex-direction: column }`
    // baseline. Both removed for perf: at 10k+ rows the per-node
    // flex-container tracking cost dominated post-mount layout.
    // Flex semantics now happen at CSS-emit time —
    // `rules_to_css` auto-promotes a style to `display: flex`
    // when the rules use any flex-container property.

    /// Inject the virtualizer JS shim into the document on first
    /// use. The shim defines `window.__idealystVirtualizer` (the
    /// recycler class the backend then constructs). Inlined via
    /// `include_str!` so consumers don't need to ship a separate
    /// JS file or set up a build pipeline.
    ///
    /// We use `Function::new_no_args(src).call0()` (which evals the
    /// source in the global scope) rather than appending a `<script>`
    /// element — the latter has subtle browser-specific quirks
    /// around when dynamically-inserted scripts execute, and some
    /// configurations (CSP, certain WASM hosts) don't run them at
    /// all. Eval-via-Function is unambiguous and reliable.
    pub(crate) fn ensure_virtualizer_shim(&mut self) {
        if self.virtualizer_shim_injected {
            return;
        }
        let src = include_str!("../runtime/js/virtualizer.js");
        // Wrap in a function that returns nothing and call it. The
        // shim's body is wrapped in an IIFE itself; this outer
        // Function::new_no_args is just our way of executing it.
        let f = js_sys::Function::new_no_args(src);
        let _ = f.call0(&JsValue::NULL);
        self.virtualizer_shim_injected = true;
    }

    /// Inject the local-render batch executor (`__idealystExecuteBatch`)
    /// into the document on first use. Same evaluation strategy as
    /// [`ensure_virtualizer_shim`] — bundle the JS via
    /// `include_str!` and run it inside a `Function::new_no_args`
    /// call so we don't depend on `<script>` injection semantics.
    pub(crate) fn ensure_batch_shim(&mut self) {
        if self.batch_shim_injected {
            return;
        }
        let src = include_str!("../runtime/js/batch.js");
        let f = js_sys::Function::new_no_args(src);
        let _ = f.call0(&JsValue::NULL);
        self.batch_shim_injected = true;
    }

    /// Inject the batched text-update shim
    /// (`__idealystRegisterText` / `__idealystReleaseText` /
    /// `__idealystUpdateTextBatch`) on first use. Same evaluation
    /// strategy as [`ensure_batch_shim`]. Lazy so apps that never
    /// hit the reactive-text path (e.g. pages with only static
    /// labels) don't pay the injection cost.
    pub(crate) fn ensure_text_batch_shim(&mut self) {
        if self.text_batch_shim_injected {
            return;
        }
        let src = include_str!("../runtime/js/text_batch.js");
        let f = js_sys::Function::new_no_args(src);
        let _ = f.call0(&JsValue::NULL);
        self.text_batch_shim_injected = true;
    }

    /// Inject the JS-side reactive binding shim
    /// (`__idealystRegisterBinding` / `__idealystReleaseBinding` /
    /// `__idealystOnSignalChanged`). Companion to
    /// [`ensure_text_batch_shim`] — they share the text-id space,
    /// so a node registered for batched-text updates can ALSO have
    /// a binding registered against its id without conflict.
    /// Lazy: only injected when a backend op needs it.
    pub(crate) fn ensure_text_bindings_shim(&mut self) {
        if self.text_bindings_shim_injected {
            return;
        }
        let src = include_str!("../runtime/js/text_bindings.js");
        let f = js_sys::Function::new_no_args(src);
        let _ = f.call0(&JsValue::NULL);
        self.text_bindings_shim_injected = true;
    }

    /// Inject the JS-side batched class-attribute shim
    /// (`__idealystRegisterStyledNode` / `__idealystApplyClassesBatch` /
    /// `__idealystReleaseStyledNode`). Lazy: only injected when the
    /// style apply path actually needs to queue a class update.
    /// First-apply path also caches the `js_sys::Function` handles
    /// so subsequent applies skip the `Reflect::get` lookup.
    pub(crate) fn ensure_class_batch_shim(&mut self) {
        if self.class_batch_shim_injected {
            return;
        }
        let src = include_str!("../runtime/js/class_batch.js");
        let f = js_sys::Function::new_no_args(src);
        let _ = f.call0(&JsValue::NULL);
        self.class_batch_shim_injected = true;
    }

    /// Inject the JS-side reactive class-binding shim
    /// (`__idealystRegisterClassBinding` /
    /// `__idealystReleaseClassBindingsBatch`). Depends on the
    /// text-bindings shim (taps its `__idealystOnSignalChanged`)
    /// and the class-batch shim (uses `__idealystStyledNodes` as
    /// its registry). Both are pre-injected by
    /// `install_text_batcher`, so by the time a `SignalClass`
    /// binding registers, the dependencies are already present.
    pub(crate) fn ensure_class_bindings_shim(&mut self) {
        if self.class_bindings_shim_injected {
            return;
        }
        // The class-binding dispatcher TAPS into the text-bindings
        // signal-changed handler, so that shim must be present
        // first. The class-batch shim provides the
        // `__idealystStyledNodes` registry the dispatcher reads
        // for node lookups.
        self.ensure_text_bindings_shim();
        self.ensure_class_batch_shim();
        let src = include_str!("../runtime/js/class_bindings.js");
        let f = js_sys::Function::new_no_args(src);
        let _ = f.call0(&JsValue::NULL);
        self.class_bindings_shim_injected = true;
    }

    /// Inject `@keyframes ui-spin` into the stylesheet on first use.
    /// Subsequent ActivityIndicator constructions reuse the same
    /// keyframes — the rule is identity-stable, no need to re-create.
    pub(crate) fn ensure_spinner_keyframes(&mut self) {
        if self.spinner_keyframes_injected {
            return;
        }
        let rule = "@keyframes ui-spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }";
        // Append at the sheet's end. Doesn't shift any existing
        // index, so no bookkeeping needed.
        let sheet = self.sheet();
        let end = sheet.css_rules().map(|r| r.length()).unwrap_or(0);
        let _ = sheet.insert_rule_with_index(rule, end);
        self.spinner_keyframes_injected = true;
    }

    /// Removes a node's dynamic slot, if any, and drops its
    /// refcount on the shared dynamic-by-content rule. If the node
    /// was the last user, the rule is deleted.
    pub(crate) fn drop_dynamic_slot(&mut self, id: u32) {
        if let Some(slot) = self.dynamic.remove(&id) {
            self.release_dynamic_rule(&slot.shared);
        }
    }
}
