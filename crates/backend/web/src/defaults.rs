//! Global stylesheet baselines: the `.ui-default` class every node
//! gets, the spinner keyframes, the JS runtime shims, and the
//! per-node dynamic-slot teardown helper.
//!
//! # The JS shims
//!
//! The eight hand-written shims in `runtime/js/` (batch executor, text
//! and class batching/bindings, node ids, the two virtualizers) ship as
//! web-glue JS modules (`web_glue::js_module!`): their source travels in
//! the wasm's `__idealyst_glue` section and the build writes it into
//! `pkg/__idealyst_glue.js` as ordinary code. Each `ensure_*_shim`
//! evaluates its module once (`G.m(name)`), which runs the shim's IIFE and
//! installs its `window.__idealyst*` globals — the surface the call sites
//! still read in phase 2a (they become `import!`s in 2b).
//!
//! This replaced evaluating the source at run time with
//! `web_glue::js::Function::new_no_args(src).call0()`: no `eval`-class call (a
//! CSP with no `unsafe-eval` now runs every shim), and the shim text is no
//! longer a data-section string in the shipped wasm. The embedded copies
//! are still build.rs-minified (`OUT_DIR/js-min/`).
//!
//! These live in their own `impl WebBackend` block so they're separate
//! from per-primitive create code and from the CSS converter helpers
//! in [`crate::style`].

use crate::WebBackend;

/// Declare each shim as a glue module and give it an `ensure` that
/// evaluates it. The anchor call keeps the module's record linked (see
/// `web_glue::js_module!`'s anchor rule) exactly when its `ensure` is.
macro_rules! shim_modules {
    ($($anchor:ident / $eval:ident = $name:literal, $file:literal;)*) => {
        $(
            web_glue::js_module!(fn $anchor = $name, include_str!(concat!(env!("OUT_DIR"), "/js-min/", $file)));
            fn $eval() {
                $anchor();
                let (p, l) = web_glue::string::abi($name);
                unsafe { js_eval_module(p, l) }
            }
        )*
    };
}

web_glue::import! {
    fn js_eval_module(p: usize, l: usize) = "(p, l) => { G.m(G.str(p, l)); }";
}

shim_modules! {
    virtualizer_module / eval_virtualizer = "backend-web/virtualizer", "virtualizer.js";
    virtual_grid_module / eval_virtual_grid = "backend-web/virtual_grid", "virtual_grid.js";
    batch_module / eval_batch = "backend-web/batch", "batch.js";
    text_batch_module / eval_text_batch = "backend-web/text_batch", "text_batch.js";
    text_bindings_module / eval_text_bindings = "backend-web/text_bindings", "text_bindings.js";
    class_batch_module / eval_class_batch = "backend-web/class_batch", "class_batch.js";
    class_bindings_module / eval_class_bindings = "backend-web/class_bindings", "class_bindings.js";
    node_ids_module / eval_node_ids = "backend-web/node_ids", "node_ids.js";
}

impl WebBackend {
    // The framework used to stamp every framework-created element
    // with `class="ui-default"` and inject a
    // `.ui-default { display: flex; flex-direction: column }`
    // baseline. Both removed for perf: at 10k+ rows the per-node
    // flex-container tracking cost dominated post-mount layout.
    // Flex semantics now happen at CSS-emit time —
    // `rules_to_css` auto-promotes a style to `display: flex`
    // when the rules use any flex-container property.

    /// Evaluate the virtualizer JS shim on first use. The shim defines
    /// `window.__idealystVirtualizer` (the recycler class the backend
    /// then constructs). See the module docs for how it ships.
    pub(crate) fn ensure_virtualizer_shim(&mut self) {
        if self.virtualizer_shim_injected {
            return;
        }
        // `OUT_DIR/js-min/` holds build.rs-minified copies of
        // `runtime/js/*.js` — comments/indentation stripped so the shim
        // source doesn't ship inside the wasm (~33-54 KB of commentary).
        // Edit the originals under `runtime/js/`; build.rs re-emits.
        eval_virtualizer();
        self.virtualizer_shim_injected = true;
    }

    /// Inject the two-axis grid shim (`window.__idealystVirtualGrid`)
    /// on first use. Same strategy and same laziness as
    /// [`ensure_virtualizer_shim`]: an app that never mounts a
    /// `virtual_grid` never pays the injection.
    pub(crate) fn ensure_virtual_grid_shim(&mut self) {
        if self.virtual_grid_shim_injected {
            return;
        }
        eval_virtual_grid();
        self.virtual_grid_shim_injected = true;
    }

    /// Inject the local-render batch executor (`__idealystExecuteBatch`)
    /// on first use. Same strategy as [`ensure_virtualizer_shim`].
    pub(crate) fn ensure_batch_shim(&mut self) {
        if self.batch_shim_injected {
            return;
        }
        eval_batch();
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
        eval_text_batch();
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
        eval_text_bindings();
        self.text_bindings_shim_injected = true;
    }

    /// Inject the JS-side batched class-attribute shim
    /// (`__idealystRegisterStyledNode` / `__idealystApplyClassesBatch` /
    /// `__idealystReleaseStyledNode`). Lazy: only injected when the
    /// style apply path actually needs to queue a class update.
    /// First-apply path also caches the `web_glue::js::Function` handles
    /// so subsequent applies skip the `Reflect::get` lookup.
    pub(crate) fn ensure_class_batch_shim(&mut self) {
        if self.class_batch_shim_injected {
            return;
        }
        eval_class_batch();
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
        eval_class_bindings();
        self.class_bindings_shim_injected = true;
        // The shim just WRAPPED `window.__idealystOnSignalChanged`
        // (text_bindings' handler) with the class dispatcher. Drop any
        // cached handle `ship_signal_change_to_js` captured BEFORE the
        // wrap — on the new core a text-binding notifier can fire before
        // the first class binding registers, and shipping through the
        // stale pre-wrap handle would bypass class bindings forever
        // (sclass rows freeze; found by the js-framework-bench gate).
        self.signal_changed_fn = None;
    }

    /// Inject the JS-side stable-node-id shim
    /// (`__idealystNodeId(node) -> u32`). Backs
    /// [`WebBackend::node_id`] with a `WeakMap` so the same JS DOM
    /// object always resolves to the same `u32` regardless of which
    /// Rust `web_glue::dom::Node` wrapper holds a reference to it.
    /// Auto-injected on first `node_id` call.
    pub(crate) fn ensure_node_id_shim(&mut self) {
        if self.node_id_shim_injected {
            return;
        }
        eval_node_ids();
        self.node_id_shim_injected = true;
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
