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
    /// Inject the `.ui-default` rule the first time a framework
    /// element is created. The rule encodes the framework's
    /// mobile-first defaults so every node gets `display: flex;
    /// flex-direction: column` even before any user style is applied.
    /// User-minted classes override on overlap because they're
    /// inserted later (later wins at equal specificity).
    pub(crate) fn ensure_defaults_class(&mut self) {
        if self.defaults_class_injected {
            return;
        }
        // Append at the sheet's end. This rule isn't owned by any
        // particular stylesheet — it's a global baseline. Appending
        // doesn't shift any existing index, so we don't need to
        // walk `pregen`/`dynamic` afterwards (which would defeat
        // the O(1) insert_rule contract).
        let rule = ".ui-default { display: flex; flex-direction: column; }";
        let sheet = self.sheet();
        let end = sheet.css_rules().map(|r| r.length()).unwrap_or(0);
        let _ = sheet.insert_rule_with_index(rule, end);
        self.defaults_class_injected = true;
    }

    /// Attach the framework's default class to a freshly created
    /// element. `apply_style` later concatenates the user-minted
    /// class alongside this one — see the className-merge logic
    /// inside `apply_style`.
    pub(crate) fn apply_default_class(&mut self, element: &web_sys::Element) {
        self.ensure_defaults_class();
        let _ = element.set_attribute("class", "ui-default");
    }

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

    /// Removes a node's dynamic slot, if any, and deletes its CSS rules
    /// (base + any per-state pseudo-class overlays).
    pub(crate) fn drop_dynamic_slot(&mut self, id: u32) {
        if let Some(slot) = self.dynamic.remove(&id) {
            self.delete_rule(slot.rule_index);
            for idx in slot.state_rule_indices {
                self.delete_rule(idx);
            }
        }
    }
}
