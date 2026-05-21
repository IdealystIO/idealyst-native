//! CSS conversion + shared stylesheet management for `WebBackend`.
//!
//! Two responsibilities:
//!
//! 1. **Value → CSS string converters.** Each framework enum
//!    (`FlexDirection`, `AlignItems`, …) has a tiny `fn _css(v) ->
//!    &'static str` that maps it to the matching CSS keyword. The
//!    top-level [`rules_to_css`] walks every `StyleRules` field and
//!    produces a CSS body suitable for a single class.
//!
//! 2. **Stylesheet rule index bookkeeping.** Rules are inserted into
//!    the shared `<style>` element via `insert_rule`; the returned
//!    index lets us delete them later. CSSOM shifts every existing
//!    index when one is inserted/deleted, so we maintain in-process
//!    mirrors (`pregen`, `dynamic` maps on `WebBackend`) and shift
//!    them in lockstep.

use crate::phase_timer::PhaseTimer;
use crate::{DynamicPtrEntry, DynamicRule, DynamicSlot, PregenEntry, WebBackend};
use framework_core::StyleRules;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use wasm_bindgen::JsCast;

// ---------------------------------------------------------------------------
// Stylesheet rule-index management — split out of `impl WebBackend`.
// ---------------------------------------------------------------------------

impl WebBackend {
    /// Lazily creates the shared `<style>` element in document.head.
    ///
    /// On first creation we seed it with a UA-baseline reset for
    /// `<button>` — strips the browser's default outset border, gray
    /// background, and inherited-font override. The reset is scoped
    /// with `:where(button)`, which CSS gives a specificity of zero.
    /// Any author class rule the framework attaches via `apply_style`
    /// (specificity 0,1,0) wins automatically, so authors that want
    /// to put a border back on a Button just declare `border_width:
    /// 1.0` in their stylesheet and it works.
    ///
    /// Without this reset, the framework's `<button>` element comes
    /// in with the browser's chunky outset border showing through
    /// any class rule that doesn't explicitly zero out `border`.
    pub(crate) fn ensure_style_element(&mut self) -> web_sys::HtmlStyleElement {
        if self.style_element.is_none() {
            let elem = self
                .doc
                .create_element("style")
                .expect("create style")
                .unchecked_into::<web_sys::HtmlStyleElement>();
            let head = self.doc.head().expect("document has head");
            head.append_child(&elem).expect("append style to head");
            self.style_element = Some(elem);

            // Seed the resets at indices 0–1. We never delete or
            // shift these, so the rule recycler in `insert_rule`
            // doesn't need to know about them — it appends or
            // recycles at index ≥ 2.
            let sheet = self
                .style_element
                .as_ref()
                .unwrap()
                .sheet()
                .expect("sheet")
                .unchecked_into::<web_sys::CssStyleSheet>();

            // Index 0 — universal `box-sizing: border-box`.
            //
            // The framework's box model is React-Native-style:
            // `padding`/`border_width` live INSIDE an element's
            // declared `width`/`height`. The browser's default
            // `content-box` adds padding OUTSIDE the declared size,
            // which silently breaks any percent-height layout
            // (e.g. a 100%-height sidebar with `padding: lg` ends
            // up 100vh + 2*lg tall and overflows the viewport).
            //
            // Apply universally — every framework-rendered element
            // expects border-box semantics. The specificity-0
            // selector loses to any author class rule that
            // explicitly sets `box-sizing`, so opting out per
            // element is still possible.
            let _ = sheet.insert_rule_with_index(
                "*, *::before, *::after { box-sizing: border-box; }",
                0,
            );

            // Index 1 — `<button>` element reset.
            //
            // `:where(button)` has CSS specificity 0, so any author
            // class rule the framework attaches via `apply_style`
            // (specificity 0,1,0) wins automatically. Without this
            // reset, the browser's chunky outset border shows
            // through any class rule that doesn't explicitly zero
            // out `border`. Authors that want the border back just
            // set `border_width: 1.0` in their stylesheet.
            let button_reset = ":where(button) { \
                                all: unset; \
                                box-sizing: border-box; \
                                cursor: pointer; \
                                font: inherit; \
                                color: inherit; \
                                display: inline-flex; \
                                align-items: center; \
                                justify-content: center; \
                                }";
            let _ = sheet.insert_rule_with_index(button_reset, 1);
        }
        self.style_element.as_ref().unwrap().clone()
    }

    pub(crate) fn sheet(&mut self) -> web_sys::CssStyleSheet {
        let elem = self.ensure_style_element();
        elem.sheet()
            .expect("style element has no sheet")
            .unchecked_into::<web_sys::CssStyleSheet>()
    }

    /// Insert a CSS rule into the shared sheet and return its index.
    ///
    /// O(1) regardless of how many rules are live. Two cases:
    ///
    /// - **Recycle slot.** If a previously-deleted rule index is
    ///   available, replace at that index via `deleteRule(idx)` +
    ///   `insertRule(rule, idx)`. CSSOM shifts everything after `idx`
    ///   down by one on the delete, then back up by one on the
    ///   insert — net zero change to other indices, so no
    ///   bookkeeping is needed on `pregen`/`dynamic` entries.
    /// - **Append.** Otherwise insert at the sheet's current
    ///   length. No existing index moves, no bookkeeping needed.
    ///
    /// This replaces the previous insert-at-0 path, which shifted
    /// every existing index up by 1 on every insertion. With 1000+
    /// dynamic rules churning under theme toggles, that was O(N²)
    /// per toggle and pegged the main thread.
    pub(crate) fn insert_rule(&mut self, class_name: &str, body: &str) -> u32 {
        // Manual concatenation to avoid the `format!` machinery,
        // which monomorphizes a path through `Display` and pulls
        // more code into the binary than this simple join needs.
        let mut rule = String::with_capacity(class_name.len() + body.len() + 6);
        rule.push('.');
        rule.push_str(class_name);
        rule.push_str(" { ");
        rule.push_str(body);
        rule.push_str(" }");
        let sheet = self.sheet();
        if let Some(idx) = self.free_rule_indices.pop() {
            // Replace the deleted slot in place. The deleteRule
            // shifts indices > idx down by 1; the immediately-
            // following insertRule(rule, idx) shifts them back up.
            // Net: identical, so nothing else needs updating.
            let _ = sheet.delete_rule(idx);
            let new_idx = sheet
                .insert_rule_with_index(&rule, idx)
                .expect("insert_rule_with_index (recycle) failed");
            // CSSOM returns the inserted-at index; should equal `idx`.
            debug_assert_eq!(new_idx, idx);
            new_idx
        } else {
            // Append at sheet length. No existing index changes.
            let end = sheet.css_rules().map(|r| r.length()).unwrap_or(0);
            sheet
                .insert_rule_with_index(&rule, end)
                .expect("insert_rule_with_index (append) failed")
        }
    }

    /// Mark a previously-inserted rule's slot free for re-use.
    /// O(1). Does not actually call `deleteRule` — that would shift
    /// every later index down and force us to walk every entry.
    /// Instead we record the slot in `free_rule_indices`; the next
    /// `insert_rule` recycles it via the `deleteRule + insertRule`
    /// trick above.
    ///
    /// Side effect: the rule physically remains in the sheet until
    /// recycled. Its class name is no longer applied to any
    /// element, so it's inert; the browser parses it once on
    /// insert and ignores it thereafter.
    pub(crate) fn delete_rule(&mut self, idx: u32) {
        self.free_rule_indices.push(idx);
    }
}

// ---------------------------------------------------------------------------
// register / unregister / apply / on_node_unstyled — the style-effect
// surface of the Backend trait. Lives here because it operates on the
// same `pregen` / `dynamic` maps as the rule-management above.
// ---------------------------------------------------------------------------

impl WebBackend {
    /// Pre-generation: for each rule, look up or mint a class.
    /// Pre-generated classes have a `refcount` that bumps once per
    /// registration; they're removed when refcount hits zero via
    /// `unregister_stylesheet`.
    pub(crate) fn impl_register_stylesheet(&mut self, rules: &[std::rc::Rc<StyleRules>]) {
        for r in rules {
            let key = r.content_key();
            let class_name = if let Some(entry) = self.pregen.get_mut(&key) {
                entry.refcount += 1;
                entry.name.clone()
            } else {
                let class_name = hash_class_name(&key);
                let body = rules_to_css(r);
                let rule_index = self.insert_rule(&class_name, &body);
                self.pregen.insert(
                    key,
                    PregenEntry {
                        name: class_name.clone(),
                        rule_index,
                        refcount: 1,
                    },
                );
                class_name
            };
            // Pointer-index this Rc so the hot apply path can skip
            // `content_key()` when the framework's resolution cache
            // hands us this exact Rc back. The pointer is stable for
            // the Rc's lifetime; we clear the entry in
            // `impl_unregister_stylesheet` when the matching
            // registration refcount hits zero.
            let ptr = std::rc::Rc::as_ptr(r);
            self.pregen_by_ptr.insert(ptr, class_name);
        }
    }

    /// Install or update the active theme's tokens as CSS custom
    /// properties on `:root`. First call inserts a single
    /// `:root { --t1: v1; --t2: v2; ... }` rule; subsequent calls
    /// reach into the same rule's `style` declaration and call
    /// `setProperty` for each token. Theme swap is then **N
    /// `setProperty` calls** for N tokens — no rule churn, no
    /// `className` mutation on any node, no resolution-cache wipe
    /// felt by the DOM.
    pub(crate) fn impl_install_theme_variables(
        &mut self,
        tokens: &[framework_core::TokenEntry],
    ) {
        use framework_core::TokenValue;
        // Format helpers — one match per variant, kept inline so we
        // never allocate a temp Vec for the value-to-string step.
        fn token_value_css(v: &TokenValue) -> String {
            match v {
                TokenValue::Color(c) => c.0.clone(),
                TokenValue::Length(l) => length_css(*l),
                TokenValue::Number(n) => n.to_string(),
            }
        }

        if let Some(idx) = self.theme_root_rule_index {
            // Update path: reach into the existing :root rule and
            // setProperty each token. Browsers treat this as a single
            // style invalidation per changed value; no rule object is
            // created or destroyed.
            let sheet = self.sheet();
            if let Ok(rules) = sheet.css_rules() {
                if let Some(rule) = rules.get(idx) {
                    if let Ok(style_rule) = rule.dyn_into::<web_sys::CssStyleRule>() {
                        let decl = style_rule.style();
                        for entry in tokens {
                            let prop = format!("--{}", entry.name);
                            let value = token_value_css(&entry.value);
                            // The third arg ("priority") is empty —
                            // we never want `!important` on token
                            // declarations.
                            let _ = decl.set_property(&prop, &value);
                        }
                        return;
                    }
                }
            }
            // Fall through to insertion if anything went wrong (rule
            // disappeared underneath us). This shouldn't happen, but
            // keeps us from getting stuck pointing at an invalid
            // index.
            self.theme_root_rule_index = None;
        }

        // Insert path: emit a single `:root { ... }` rule. We bypass
        // `insert_rule` (which prepends `.` for class selectors) and
        // call the CSSOM directly, then track the index.
        let mut rule = String::with_capacity(tokens.len() * 32 + 16);
        rule.push_str(":root { ");
        for entry in tokens {
            rule.push_str("--");
            rule.push_str(entry.name);
            rule.push_str(": ");
            rule.push_str(&token_value_css(&entry.value));
            rule.push_str("; ");
        }
        rule.push('}');
        let sheet = self.sheet();
        let end = sheet.css_rules().map(|r| r.length()).unwrap_or(0);
        let idx = sheet
            .insert_rule_with_index(&rule, end)
            .expect("insert :root rule (append)");
        self.theme_root_rule_index = Some(idx);
    }

    pub(crate) fn impl_unregister_stylesheet(&mut self, rules: &[std::rc::Rc<StyleRules>]) {
        for r in rules {
            // Always drop the pointer-keyed entry — the Rc is going
            // away with this unregister, so its pointer will no
            // longer be valid for future apply-time lookups.
            let ptr = std::rc::Rc::as_ptr(r);
            self.pregen_by_ptr.remove(&ptr);

            let key = r.content_key();
            let should_drop = if let Some(entry) = self.pregen.get_mut(&key) {
                entry.refcount = entry.refcount.saturating_sub(1);
                entry.refcount == 0
            } else {
                false
            };
            if should_drop {
                if let Some(entry) = self.pregen.remove(&key) {
                    self.delete_rule(entry.rule_index);
                }
            }
        }
    }

    /// Apply a resolved style to a node.
    ///
    /// - If the rule's content matches a pre-generated class, set
    ///   `className` to it and clear any dynamic slot the node had.
    /// - Else, mint a fresh per-node dynamic class, replacing this
    ///   node's previous dynamic class atomically.
    pub(crate) fn impl_apply_style(
        &mut self,
        node: &web_sys::Node,
        style: &std::rc::Rc<StyleRules>,
    ) {
        let id = self.node_id(node);
        let key = style.content_key();

        // Path 1: pre-generated cache hit.
        if let Some(entry) = self.pregen.get(&key) {
            let class_name = entry.name.clone();
            self.queue_class_apply(node, &class_name);
            // If we had a dynamic class previously, remove it now —
            // the pre-generated one is what's active.
            self.drop_dynamic_slot(id);
            return;
        }

        // Path 2: deduped dynamic mint via the same shared-Rc design
        // as `impl_apply_styled_states`. Bump-before-release ordering
        // preserves rule liveness when the new and old keys match.
        let shared = if let Some(entry) = self.dynamic_by_content.get(&key) {
            entry.shared.refcount.set(entry.shared.refcount.get() + 1);
            entry.shared.clone()
        } else {
            let class_name = hash_class_name(&key);
            let body = rules_to_css(style);
            let new_index = self.insert_rule(&class_name, &body);
            let shared = std::rc::Rc::new(DynamicPtrEntry {
                class_name,
                content_key: key.clone(),
                refcount: std::cell::Cell::new(1),
            });
            self.dynamic_by_content.insert(
                key,
                DynamicRule {
                    shared: shared.clone(),
                    rule_index: new_index,
                    state_rule_indices: Vec::new(),
                },
            );
            self.dynamic_by_ptr
                .insert(std::rc::Rc::as_ptr(style), shared.clone());
            shared
        };

        let class_for_queue = shared.class_name.clone();
        let prev = self.dynamic.insert(id, DynamicSlot { shared });
        if let Some(old) = prev {
            self.release_dynamic_rule(&old.shared);
        }
        // Queue AFTER the slot bookkeeping so the borrow doesn't
        // span the queue call (which itself takes `&mut self`).
        self.queue_class_apply(node, &class_for_queue);
    }

    pub(crate) fn impl_apply_styled_states(
        &mut self,
        node: &web_sys::Node,
        base: &std::rc::Rc<StyleRules>,
        overlays: &[(framework_core::StateBits, std::rc::Rc<StyleRules>)],
    ) {
        // Outer phase covers the whole call — comparing this against
        // the sum of the sub-phases lets us see how much time is
        // unaccounted for (allocator, etc.).
        let _t_total = PhaseTimer::start("apply_styled_states");

        // No `dyn_ref` cast here: the class apply goes through
        // `queue_class_apply` which uses the Node directly (the
        // JS-side shim registers the Node as an Element on first
        // call) and only the rare fallback path needs the cast.
        let id = self.node_id(node);

        // Fast-fast path: pointer-keyed pregen hit. When the
        // framework's resolution cache returns the same
        // `Rc<StyleRules>` for many nodes (the 10k-row case, where
        // every "even" row gets one shared Rc), we can identify the
        // class by Rc identity without computing `content_key()` at
        // all. The pointer is stable for the Rc's lifetime; the
        // pregen_by_ptr map is populated alongside content-keyed
        // pregen during `register_stylesheet`, AND refreshed below
        // on the content-key-hit branch so post-theme-swap Rcs
        // (which carry new pointers) get cached after the first
        // row in the cohort pays the content-key lookup.
        if overlays.is_empty() {
            let ptr = std::rc::Rc::as_ptr(base);
            let ptr_hit = {
                let _t = PhaseTimer::start("pregen_lookup_ptr");
                self.pregen_by_ptr.get(&ptr).cloned()
            };
            if let Some(class_name) = ptr_hit {
                self.queue_class_apply(node, &class_name);
                {
                    let _t = PhaseTimer::start("drop_dynamic_slot");
                    self.drop_dynamic_slot(id);
                }
                return;
            }
            // Second fast path: pointer-keyed DYNAMIC hit. Same idea,
            // but for content the framework didn't pre-register —
            // typically `.override_*` builder methods on a reactive
            // style closure. The framework's `RESOLUTION_CACHE` hands
            // us the same `Rc<StyleRules>` for every node that
            // resolved to the same `(sheet, variants, overrides)`,
            // so after the first node in the cohort pays the
            // content-key path + mints + populates this map, every
            // subsequent node short-circuits here.
            //
            // Saves `content_key()` (~300-byte string format on every
            // call) for the hot reactive-style-cohort case.
            let dyn_ptr_hit = {
                let _t = PhaseTimer::start("dynamic_lookup_ptr");
                self.dynamic_by_ptr.get(&ptr).cloned()
            };
            if let Some(shared) = dyn_ptr_hit {
                // Slot-already-has-this-class short-circuit. When
                // the SAME row's Effect re-fires but resolves to
                // the same content as last time (e.g. POINT bump on
                // an unrelated signal), the class hasn't changed —
                // skip the setAttribute round-trip entirely.
                let already_applied = self
                    .dynamic
                    .get(&id)
                    .map(|slot| std::rc::Rc::ptr_eq(&slot.shared, &shared))
                    .unwrap_or(false);
                if already_applied {
                    return;
                }

                // Refcount bump via interior mutability on the
                // shared `Rc<DynamicPtrEntry>` — no HashMap lookup,
                // no `content_key` hash on the hot path.
                shared.refcount.set(shared.refcount.get() + 1);
                let class_for_queue = shared.class_name.clone();
                let prev = self.dynamic.insert(id, DynamicSlot { shared });
                if let Some(old) = prev {
                    self.release_dynamic_rule(&old.shared);
                }
                self.queue_class_apply(node, &class_for_queue);
                return;
            }
        }

        // Content-keyed fast path. Used when the Rc identity didn't
        // hit (e.g. the user passed `.override_*` builder methods
        // that produce a fresh Rc each time, but with content
        // identical to a pre-gen entry — including, critically,
        // every post-theme-swap Rc whose rules only reference
        // tokens via `var()`).
        let base_key = {
            let _t = PhaseTimer::start("content_key");
            base.content_key()
        };

        if overlays.is_empty() {
            let pregen_hit = {
                let _t = PhaseTimer::start("pregen_lookup");
                self.pregen.get(&base_key).map(|entry| entry.name.clone())
            };
            if let Some(class_name) = pregen_hit {
                self.queue_class_apply(node, &class_name);
                {
                    let _t = PhaseTimer::start("drop_dynamic_slot");
                    self.drop_dynamic_slot(id);
                }
                return;
            }
        }

        // Slow path: state overlays present, or no pregen hit. The
        // dynamic-by-content cache is the second line of defense —
        // when N reactive-styled nodes resolve to the same
        // `(base + overlays)` content (e.g. one shared signal driving
        // every row's background), they ALL share one minted class.
        // First node mints + inserts the rule; subsequent nodes bump
        // the refcount and just `setAttribute("class", …)`.
        //
        // Without this cache, every reactive-style fan-out paid
        // `rules_to_css + insert_rule + delete_rule` per node — at
        // 2000 rows that's ~17ms of pure rule churn for a one-signal
        // bump that visually flips two colors. Catastrophic for any
        // N rows × shared-signal pattern.
        //
        // Key: include the overlay states so distinct (base, overlays)
        // combinations get distinct class names. We concat each
        // overlay's content_key with a pseudo-class tag.
        let mut key = base_key;
        for (bit, ov) in overlays {
            key.push(';');
            key.push_str(state_bit_tag(*bit));
            key.push(':');
            key.push_str(&ov.content_key());
        }

        // Cache lookup. Refcount-bump-on-hit MUST happen before we
        // release the old slot below — if the previous slot pointed
        // at THIS same key, releasing first would briefly drop the
        // refcount to zero and delete the rules we're about to use.
        let shared = if let Some(entry) = self.dynamic_by_content.get(&key) {
            let _t = PhaseTimer::start("dynamic_cache_hit");
            entry.shared.refcount.set(entry.shared.refcount.get() + 1);
            entry.shared.clone()
        } else {
            // Cache miss: mint fresh class + insert rules.
            let class_name = {
                let _t = PhaseTimer::start("hash_class_name");
                hash_class_name(&key)
            };
            let base_body = {
                let _t = PhaseTimer::start("rules_to_css");
                rules_to_css(base)
            };
            let base_idx = {
                let _t = PhaseTimer::start("insert_rule");
                self.insert_rule(&class_name, &base_body)
            };

            // Insert each state overlay as a pseudo-class scoped rule.
            let mut state_indices: Vec<u32> = Vec::with_capacity(overlays.len());
            for (bit, overlay) in overlays {
                let pseudo = match *bit {
                    framework_core::StateBits::HOVERED => ":hover",
                    framework_core::StateBits::PRESSED => ":active",
                    framework_core::StateBits::FOCUSED => ":focus",
                    framework_core::StateBits::DISABLED => ":disabled",
                    _ => continue,
                };
                let selector = format!("{}{}", class_name, pseudo);
                let body = {
                    let _t = PhaseTimer::start("rules_to_css");
                    rules_to_css(overlay)
                };
                let idx = {
                    let _t = PhaseTimer::start("insert_rule");
                    self.insert_rule(&selector, &body)
                };
                state_indices.push(idx);
            }

            let shared = std::rc::Rc::new(DynamicPtrEntry {
                class_name,
                content_key: key.clone(),
                refcount: std::cell::Cell::new(1),
            });
            self.dynamic_by_content.insert(
                key,
                DynamicRule {
                    shared: shared.clone(),
                    rule_index: base_idx,
                    state_rule_indices: state_indices,
                },
            );
            // Mirror into the pointer cache only for the empty-overlay
            // path — when overlays are present, distinct base Rcs can
            // share a base content but require distinct combined keys,
            // so pointer identity isn't a safe shortcut.
            if overlays.is_empty() {
                self.dynamic_by_ptr
                    .insert(std::rc::Rc::as_ptr(base), shared.clone());
            }
            shared
        };

        let class_for_queue = shared.class_name.clone();
        let prev = self.dynamic.insert(id, DynamicSlot { shared });
        if let Some(old) = prev {
            self.release_dynamic_rule(&old.shared);
        }
        self.queue_class_apply(node, &class_for_queue);
    }

    /// Drop one refcount on a dynamic-by-content entry. If the
    /// refcount hits zero, delete the CSS rules + clear both the
    /// content-keyed and pointer-keyed caches. Called by
    /// `drop_dynamic_slot` (node teardown) and by
    /// `impl_apply_styled_states` when a node's slot is replaced.
    ///
    /// The hot path decrements `shared.refcount` directly (interior
    /// mutability) and only consults the maps on the cold
    /// hit-zero branch — when we actually need `rule_index` + the
    /// rule indices to call `delete_rule`. Pre-this, every release
    /// hashed `content_key` to find the `dynamic_by_content` slot
    /// just to bump a counter; at 20 k Effects per SHARED fan-out
    /// that was ~6 ms of needless HashMap probing per bump.
    pub(crate) fn release_dynamic_rule(&mut self, shared: &std::rc::Rc<DynamicPtrEntry>) {
        let new_count = shared.refcount.get().saturating_sub(1);
        shared.refcount.set(new_count);
        if new_count != 0 {
            return;
        }
        // Cold path: the last slot referencing this rule just
        // dropped. Walk the maps to free the CSS rules + their
        // index slots. The pointer-cache `retain` is O(N) on the
        // map size, but the map only holds one entry per unique
        // resolved-Rc currently in use, so this stays tiny.
        if let Some(rule) = self.dynamic_by_content.remove(shared.content_key.as_str()) {
            self.dynamic_by_ptr
                .retain(|_, v| !std::rc::Rc::ptr_eq(v, &rule.shared));
            let _t = PhaseTimer::start("delete_rule");
            self.delete_rule(rule.rule_index);
            for idx in rule.state_rule_indices {
                self.delete_rule(idx);
            }
        }
    }

    pub(crate) fn impl_on_node_unstyled(&mut self, node: &web_sys::Node) {
        // Look up the node's id without minting a new one (we don't
        // want spurious id allocations during teardown).
        let p: *const web_sys::Node = node;
        if let Some(&id) = self.node_ids.get(&p) {
            // Drop the dynamic slot (deletes its CSS rule if any).
            self.drop_dynamic_slot(id);
            // Drop any state-listener closures (so they stop firing
            // on the now-removed DOM element).
            self.state_listeners.remove(&id);
            // Drop any per-node animation state.
            self.impl_drop_animated_state(id);
            // Drop the JS-side `__idealystStyledNodes` entry so the
            // Element handle can be GC'd. No-op if the node was
            // never registered (e.g. unstyled View).
            self.release_styled_node(id);
            // Remove the node-id mapping itself.
            self.node_ids.remove(&p);
        }
    }
}

// ---------------------------------------------------------------------------
// CSS value converters — free functions, no backend state.
// ---------------------------------------------------------------------------

/// Derive a deterministic class name from a content key. Same content
/// always produces the same name across sessions. 16 hex chars from
/// std DefaultHasher.
pub(crate) fn hash_class_name(content_key: &str) -> String {
    let mut h = DefaultHasher::new();
    content_key.hash(&mut h);
    let n = h.finish();
    // Manual hex encoding to skip `format!` / `Debug` machinery. 16
    // hex chars = 8 bytes of hash.
    let mut s = String::with_capacity(19);
    s.push_str("ui-");
    push_u64_hex(&mut s, n);
    s
}

/// Writes the 16-char lowercase hex representation of `n` to `out`.
fn push_u64_hex(out: &mut String, n: u64) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for shift in (0..16).rev() {
        let nibble = ((n >> (shift * 4)) & 0xf) as usize;
        out.push(HEX[nibble] as char);
    }
}

/// Render a `Length` as a CSS value string.
fn length_css(l: framework_core::Length) -> String {
    use framework_core::Length;
    match l {
        Length::Px(v) => format!("{}px", v),
        Length::Percent(v) => format!("{}%", v),
        Length::Auto => "auto".to_string(),
    }
}

/// Render a tokenized color: literal as the raw color string, token as
/// `var(--name, fallback)`. The fallback is what the browser uses if
/// the variable hasn't been installed (theme not yet booted, SSR
/// without a `:root` declaration, etc.) — we always emit it so first
/// paint never shows an unstyled element.
fn tokenized_color_css(t: &framework_core::Tokenized<framework_core::Color>) -> String {
    use framework_core::Tokenized;
    match t {
        Tokenized::Literal(c) => c.0.clone(),
        Tokenized::Token { name, fallback } => {
            format!("var(--{}, {})", name, fallback.0)
        }
    }
}

/// Render a tokenized length: literal as `{n}px` / `{n}%` / `auto`,
/// token as `var(--name, fallback)`.
fn tokenized_length_css(t: &framework_core::Tokenized<framework_core::Length>) -> String {
    use framework_core::Tokenized;
    match t {
        Tokenized::Literal(l) => length_css(*l),
        Tokenized::Token { name, fallback } => {
            format!("var(--{}, {})", name, length_css(*fallback))
        }
    }
}

/// Render a tokenized raw number (used for `opacity`, `flex_grow`).
/// The literal is just the number; tokens emit `var(--name, fallback)`.
fn tokenized_f32_css(t: &framework_core::Tokenized<f32>) -> String {
    use framework_core::Tokenized;
    match t {
        Tokenized::Literal(v) => v.to_string(),
        Tokenized::Token { name, fallback } => {
            format!("var(--{}, {})", name, fallback)
        }
    }
}

/// Render a tokenized number with the `px` suffix (border widths,
/// line-height, letter-spacing). Literal becomes `{n}px`; token
/// becomes `calc(var(--name, fallback) * 1px)` so the unit applies
/// regardless of how the variable is resolved.
fn tokenized_border_width_css(t: &framework_core::Tokenized<f32>) -> String {
    use framework_core::Tokenized;
    match t {
        Tokenized::Literal(v) => format!("{}px", v),
        Tokenized::Token { name, fallback } => {
            format!("calc(var(--{}, {}) * 1px)", name, fallback)
        }
    }
}

/// Same shape as `tokenized_border_width_css` — kept as a separate
/// helper so semantic call sites read clearly. (Both line-height and
/// letter-spacing want the `px` unit attached.)
fn tokenized_px_f32_css(t: &framework_core::Tokenized<f32>) -> String {
    tokenized_border_width_css(t)
}

fn flex_direction_css(v: framework_core::FlexDirection) -> &'static str {
    use framework_core::FlexDirection;
    match v {
        FlexDirection::Row => "row",
        FlexDirection::Column => "column",
        FlexDirection::RowReverse => "row-reverse",
        FlexDirection::ColumnReverse => "column-reverse",
    }
}

fn flex_wrap_css(v: framework_core::FlexWrap) -> &'static str {
    use framework_core::FlexWrap;
    match v {
        FlexWrap::NoWrap => "nowrap",
        FlexWrap::Wrap => "wrap",
        FlexWrap::WrapReverse => "wrap-reverse",
    }
}

fn justify_content_css(v: framework_core::JustifyContent) -> &'static str {
    use framework_core::JustifyContent;
    match v {
        JustifyContent::FlexStart => "flex-start",
        JustifyContent::FlexEnd => "flex-end",
        JustifyContent::Center => "center",
        JustifyContent::SpaceBetween => "space-between",
        JustifyContent::SpaceAround => "space-around",
        JustifyContent::SpaceEvenly => "space-evenly",
    }
}

fn align_items_css(v: framework_core::AlignItems) -> &'static str {
    use framework_core::AlignItems;
    match v {
        AlignItems::FlexStart => "flex-start",
        AlignItems::FlexEnd => "flex-end",
        AlignItems::Center => "center",
        AlignItems::Stretch => "stretch",
        AlignItems::Baseline => "baseline",
    }
}

fn align_content_css(v: framework_core::AlignContent) -> &'static str {
    use framework_core::AlignContent;
    match v {
        AlignContent::FlexStart => "flex-start",
        AlignContent::FlexEnd => "flex-end",
        AlignContent::Center => "center",
        AlignContent::Stretch => "stretch",
        AlignContent::SpaceBetween => "space-between",
        AlignContent::SpaceAround => "space-around",
    }
}

fn align_self_css(v: framework_core::AlignSelf) -> &'static str {
    use framework_core::AlignSelf;
    match v {
        AlignSelf::Auto => "auto",
        AlignSelf::FlexStart => "flex-start",
        AlignSelf::FlexEnd => "flex-end",
        AlignSelf::Center => "center",
        AlignSelf::Stretch => "stretch",
        AlignSelf::Baseline => "baseline",
    }
}

fn position_css(v: framework_core::Position) -> &'static str {
    use framework_core::Position;
    match v {
        Position::Relative => "relative",
        Position::Absolute => "absolute",
    }
}

fn font_weight_css(v: framework_core::FontWeight) -> &'static str {
    use framework_core::FontWeight;
    match v {
        FontWeight::Thin => "100",
        FontWeight::ExtraLight => "200",
        FontWeight::Light => "300",
        FontWeight::Normal => "400",
        FontWeight::Medium => "500",
        FontWeight::SemiBold => "600",
        FontWeight::Bold => "700",
        FontWeight::ExtraBold => "800",
        FontWeight::Black => "900",
    }
}

fn font_style_css(v: framework_core::FontStyle) -> &'static str {
    use framework_core::FontStyle;
    match v {
        FontStyle::Normal => "normal",
        FontStyle::Italic => "italic",
    }
}

fn text_align_css(v: framework_core::TextAlign) -> &'static str {
    use framework_core::TextAlign;
    match v {
        TextAlign::Left => "left",
        TextAlign::Right => "right",
        TextAlign::Center => "center",
        TextAlign::Justify => "justify",
    }
}

fn text_transform_css(v: framework_core::TextTransform) -> &'static str {
    use framework_core::TextTransform;
    match v {
        TextTransform::None => "none",
        TextTransform::Uppercase => "uppercase",
        TextTransform::Lowercase => "lowercase",
        TextTransform::Capitalize => "capitalize",
    }
}

fn overflow_css(v: framework_core::Overflow) -> &'static str {
    use framework_core::Overflow;
    match v {
        Overflow::Visible => "visible",
        Overflow::Hidden => "hidden",
    }
}

fn transform_css(t: &framework_core::Transform) -> String {
    use framework_core::Transform;
    match t {
        Transform::TranslateX(l) => format!("translateX({})", length_css(*l)),
        Transform::TranslateY(l) => format!("translateY({})", length_css(*l)),
        Transform::Scale(v) => format!("scale({})", v),
        Transform::ScaleXY { x, y } => format!("scale({}, {})", x, y),
        Transform::Rotate(v) => format!("rotate({}deg)", v),
        Transform::SkewX(v) => format!("skewX({}deg)", v),
        Transform::SkewY(v) => format!("skewY({}deg)", v),
    }
}

fn easing_css(e: framework_core::Easing) -> String {
    use framework_core::Easing;
    match e {
        Easing::Linear => "linear".to_string(),
        Easing::Ease => "ease".to_string(),
        Easing::EaseIn => "ease-in".to_string(),
        Easing::EaseOut => "ease-out".to_string(),
        Easing::EaseInOut => "ease-in-out".to_string(),
        Easing::CubicBezier(a, b, c, d) => {
            format!("cubic-bezier({}, {}, {}, {})", a, b, c, d)
        }
    }
}

/// Short string tag for a `StateBits` flag, used as part of the
/// content key for state-bearing dynamic slots. Distinct tags ensure
/// distinct keys (and thus distinct minted class names) for
/// different state combinations.
fn state_bit_tag(b: framework_core::StateBits) -> &'static str {
    match b {
        framework_core::StateBits::HOVERED => "h",
        framework_core::StateBits::PRESSED => "p",
        framework_core::StateBits::FOCUSED => "f",
        framework_core::StateBits::DISABLED => "d",
        _ => "?",
    }
}

/// Compile a `StyleRules` to a CSS body. Per-side padding/margin/
/// border are emitted as their CSS long-form (`padding-top`, etc.) —
/// the browser handles them just like the shorthand, but we get
/// exact-match cache keys.
///
/// **Flex semantics** are auto-promoted: if the rules use any
/// flex-container property (`gap`, `flex_direction`, `align_items`,
/// `justify_content`, `align_content`, `flex_wrap`, `row_gap`,
/// `column_gap`), the emitter prepends `display: flex` so the
/// property actually works. When none of those are set, the rule
/// stays a normal block — fewer CSS rules for the browser to
/// track, and large lists of unstyled rows don't pay the flex-
/// layout cost.
///
/// The framework historically stamped every node with a
/// `.ui-default { display: flex; flex-direction: column }` class
/// to match React Native's "every View is a flex container" idiom.
/// That class is gone now: it gave large lists O(N) flex-tracker
/// overhead even when the rows did nothing flex-shaped. The
/// auto-promotion below preserves the *intent* of the RN-style
/// API — author writes `gap: 16` and gets flex semantics for free
/// — without paying for nodes that don't use it.
pub(crate) fn rules_to_css(rules: &StyleRules) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Auto-promote: any flex-container property implies the node
    // wants flex semantics. We also pin `flex-direction: column`
    // when the user didn't, matching RN's mobile-first default
    // (CSS's own default is `row`).
    let uses_flex = rules.flex_direction.is_some()
        || rules.flex_wrap.is_some()
        || rules.justify_content.is_some()
        || rules.align_items.is_some()
        || rules.align_content.is_some()
        || rules.gap.is_some()
        || rules.row_gap.is_some()
        || rules.column_gap.is_some();
    if uses_flex {
        parts.push("display: flex".to_string());
        if rules.flex_direction.is_none() {
            parts.push("flex-direction: column".to_string());
        }
    }

    // Color + text.
    if let Some(t) = &rules.background { parts.push(format!("background: {}", tokenized_color_css(t))); }
    if let Some(t) = &rules.color { parts.push(format!("color: {}", tokenized_color_css(t))); }
    if let Some(t) = &rules.font_size { parts.push(format!("font-size: {}", tokenized_length_css(t))); }

    // Flex container.
    if let Some(v) = rules.flex_direction { parts.push(format!("flex-direction: {}", flex_direction_css(v))); }
    if let Some(v) = rules.flex_wrap { parts.push(format!("flex-wrap: {}", flex_wrap_css(v))); }
    if let Some(v) = rules.justify_content { parts.push(format!("justify-content: {}", justify_content_css(v))); }
    if let Some(v) = rules.align_items { parts.push(format!("align-items: {}", align_items_css(v))); }
    if let Some(v) = rules.align_content { parts.push(format!("align-content: {}", align_content_css(v))); }
    if let Some(t) = &rules.gap { parts.push(format!("gap: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.row_gap { parts.push(format!("row-gap: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.column_gap { parts.push(format!("column-gap: {}", tokenized_length_css(t))); }

    // Flex item.
    if let Some(t) = &rules.flex_grow { parts.push(format!("flex-grow: {}", tokenized_f32_css(t))); }
    if let Some(t) = &rules.flex_shrink { parts.push(format!("flex-shrink: {}", tokenized_f32_css(t))); }
    if let Some(t) = &rules.flex_basis { parts.push(format!("flex-basis: {}", tokenized_length_css(t))); }
    if let Some(v) = rules.align_self { parts.push(format!("align-self: {}", align_self_css(v))); }

    // Sizing.
    if let Some(t) = &rules.width { parts.push(format!("width: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.height { parts.push(format!("height: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.min_width { parts.push(format!("min-width: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.min_height { parts.push(format!("min-height: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.max_width { parts.push(format!("max-width: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.max_height { parts.push(format!("max-height: {}", tokenized_length_css(t))); }

    // Per-side padding.
    if let Some(t) = &rules.padding_top { parts.push(format!("padding-top: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.padding_right { parts.push(format!("padding-right: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.padding_bottom { parts.push(format!("padding-bottom: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.padding_left { parts.push(format!("padding-left: {}", tokenized_length_css(t))); }

    // Per-side margin.
    if let Some(t) = &rules.margin_top { parts.push(format!("margin-top: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.margin_right { parts.push(format!("margin-right: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.margin_bottom { parts.push(format!("margin-bottom: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.margin_left { parts.push(format!("margin-left: {}", tokenized_length_css(t))); }

    // Per-corner border radius.
    if let Some(t) = &rules.border_top_left_radius { parts.push(format!("border-top-left-radius: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.border_top_right_radius { parts.push(format!("border-top-right-radius: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.border_bottom_left_radius { parts.push(format!("border-bottom-left-radius: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.border_bottom_right_radius { parts.push(format!("border-bottom-right-radius: {}", tokenized_length_css(t))); }

    // Per-side border width + color. Emit `solid` style so the browser
    // actually paints the line. Width is tokenized but the `px` suffix
    // is fixed — token fallbacks are in the same unit so `var(--w, 1)px`
    // would be wrong; we instead emit `calc(var(--w, 1) * 1px)` only
    // when the value is a token, so unit math works either way.
    if let Some(t) = &rules.border_top_width {
        parts.push(format!("border-top-width: {}", tokenized_border_width_css(t)));
        parts.push("border-top-style: solid".to_string());
    }
    if let Some(t) = &rules.border_right_width {
        parts.push(format!("border-right-width: {}", tokenized_border_width_css(t)));
        parts.push("border-right-style: solid".to_string());
    }
    if let Some(t) = &rules.border_bottom_width {
        parts.push(format!("border-bottom-width: {}", tokenized_border_width_css(t)));
        parts.push("border-bottom-style: solid".to_string());
    }
    if let Some(t) = &rules.border_left_width {
        parts.push(format!("border-left-width: {}", tokenized_border_width_css(t)));
        parts.push("border-left-style: solid".to_string());
    }
    if let Some(t) = &rules.border_top_color { parts.push(format!("border-top-color: {}", tokenized_color_css(t))); }
    if let Some(t) = &rules.border_right_color { parts.push(format!("border-right-color: {}", tokenized_color_css(t))); }
    if let Some(t) = &rules.border_bottom_color { parts.push(format!("border-bottom-color: {}", tokenized_color_css(t))); }
    if let Some(t) = &rules.border_left_color { parts.push(format!("border-left-color: {}", tokenized_color_css(t))); }

    // Position.
    if let Some(v) = rules.position { parts.push(format!("position: {}", position_css(v))); }
    if let Some(t) = &rules.top { parts.push(format!("top: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.right { parts.push(format!("right: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.bottom { parts.push(format!("bottom: {}", tokenized_length_css(t))); }
    if let Some(t) = &rules.left { parts.push(format!("left: {}", tokenized_length_css(t))); }

    // Typography. `Typeface` family-names are wrapped in quotes so
    // the CSS engine never confuses them with generic keywords
    // (`monospace`, `serif`); `System` strings are passed through
    // verbatim because they often contain the comma-separated stack
    // a CSS-savvy author wants.
    if let Some(ff) = &rules.font_family {
        match ff {
            framework_core::FontFamily::System(name) => {
                parts.push(format!("font-family: {}", name));
            }
            framework_core::FontFamily::Typeface(tf) => {
                parts.push(format!("font-family: \"{}\"", tf.family_name));
            }
        }
    }
    if let Some(v) = rules.font_weight { parts.push(format!("font-weight: {}", font_weight_css(v))); }
    if let Some(v) = rules.font_style { parts.push(format!("font-style: {}", font_style_css(v))); }
    if let Some(t) = &rules.line_height { parts.push(format!("line-height: {}", tokenized_px_f32_css(t))); }
    if let Some(t) = &rules.letter_spacing { parts.push(format!("letter-spacing: {}", tokenized_px_f32_css(t))); }
    if let Some(v) = rules.text_align { parts.push(format!("text-align: {}", text_align_css(v))); }
    // Underline + strikethrough are independent booleans; emit them as
    // a single CSS `text-decoration-line` shorthand combining both.
    let underline = rules.underline.unwrap_or(false);
    let strikethrough = rules.strikethrough.unwrap_or(false);
    if underline || strikethrough {
        let mut deco = String::new();
        if underline { deco.push_str("underline"); }
        if strikethrough {
            if !deco.is_empty() { deco.push(' '); }
            deco.push_str("line-through");
        }
        parts.push(format!("text-decoration-line: {}", deco));
    } else if rules.underline == Some(false) || rules.strikethrough == Some(false) {
        // Explicit override to remove decoration.
        parts.push("text-decoration-line: none".to_string());
    }
    if let Some(v) = rules.text_transform { parts.push(format!("text-transform: {}", text_transform_css(v))); }

    // Visual.
    if let Some(t) = &rules.opacity { parts.push(format!("opacity: {}", tokenized_f32_css(t))); }
    if let Some(v) = rules.overflow { parts.push(format!("overflow: {}", overflow_css(v))); }
    if let Some(sh) = &rules.shadow {
        parts.push(format!(
            "box-shadow: {}px {}px {}px {}",
            sh.x, sh.y, sh.blur, sh.color.0
        ));
    }
    if let Some(xs) = &rules.transform {
        if !xs.is_empty() {
            let joined: Vec<String> = xs.iter().map(transform_css).collect();
            parts.push(format!("transform: {}", joined.join(" ")));
        }
    }

    // Transitions: emit a single CSS `transition` declaration listing
    // every active per-property transition. The browser interpolates
    // the property whenever its value changes — no per-frame work on
    // the framework side. Comma-separated entries.
    let transitions = collect_transitions(rules);
    if !transitions.is_empty() {
        parts.push(format!("transition: {}", transitions.join(", ")));
    }

    parts.join("; ")
}

/// Walk every per-property transition field on `rules` and produce a
/// list of CSS transition entries (`"<prop> <duration>ms <easing>"`).
/// Property names use CSS hyphenation, not the Rust field names.
fn collect_transitions(rules: &StyleRules) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    macro_rules! tr {
        ($field:ident, $css_name:literal) => {
            if let Some(t) = rules.$field {
                out.push(format!(
                    "{} {}ms {}",
                    $css_name,
                    t.duration_ms,
                    easing_css(t.easing)
                ));
            }
        };
    }
    tr!(background_transition, "background");
    tr!(color_transition, "color");
    tr!(opacity_transition, "opacity");
    tr!(transform_transition, "transform");
    tr!(width_transition, "width");
    tr!(height_transition, "height");
    tr!(top_transition, "top");
    tr!(right_transition, "right");
    tr!(bottom_transition, "bottom");
    tr!(left_transition, "left");
    tr!(padding_top_transition, "padding-top");
    tr!(padding_right_transition, "padding-right");
    tr!(padding_bottom_transition, "padding-bottom");
    tr!(padding_left_transition, "padding-left");
    tr!(margin_top_transition, "margin-top");
    tr!(margin_right_transition, "margin-right");
    tr!(margin_bottom_transition, "margin-bottom");
    tr!(margin_left_transition, "margin-left");
    tr!(border_top_left_radius_transition, "border-top-left-radius");
    tr!(border_top_right_radius_transition, "border-top-right-radius");
    tr!(border_bottom_left_radius_transition, "border-bottom-left-radius");
    tr!(border_bottom_right_radius_transition, "border-bottom-right-radius");
    tr!(border_top_width_transition, "border-top-width");
    tr!(border_right_width_transition, "border-right-width");
    tr!(border_bottom_width_transition, "border-bottom-width");
    tr!(border_left_width_transition, "border-left-width");
    tr!(border_top_color_transition, "border-top-color");
    tr!(border_right_color_transition, "border-right-color");
    tr!(border_bottom_color_transition, "border-bottom-color");
    tr!(border_left_color_transition, "border-left-color");
    out
}
