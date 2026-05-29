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
use runtime_core::StyleRules;
// CSS conversion lives in the shared, platform-neutral `css` crate so
// the web backend and the SSR backend emit byte-identical declarations.
use css::{hash_class_name, rules_to_css};
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
            // Shared with the SSR backend (which emits the same reset in
            // `<head>`) so both first-paints have border-box semantics.
            let _ = sheet.insert_rule_with_index(css::BOX_SIZING_RESET, 0);

            // Index 1 — `<button>` element reset.
            //
            // `:where(button)` has CSS specificity 0, so any author
            // class rule the framework attaches via `apply_style`
            // (specificity 0,1,0) wins automatically. Without this
            // reset, the browser's chunky outset border shows
            // through any class rule that doesn't explicitly zero
            // out `border`. Authors that want the border back just
            // set `border_width: 1.0` in their stylesheet.
            let _ = sheet.insert_rule_with_index(css::BUTTON_RESET, 1);
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
        tokens: &[runtime_core::TokenEntry],
    ) {
        // Token value → CSS string via the shared `css` crate, so web's
        // `:root` setProperty values and the SSR backend's `:root { … }`
        // block resolve a token identically (single source of truth).
        use css::token_value_css;

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

        // Insert path: emit a single `:root { ... }` rule via the shared
        // `css` crate (same string the SSR backend ships). We bypass
        // `insert_rule` (which prepends `.` for class selectors) and call
        // the CSSOM directly, then track the index.
        let rule = css::tokens_to_root_css(tokens);
        if rule.is_empty() {
            return; // no tokens → nothing to insert
        }
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

    /// Snapshot the resolved gradient onto the node's animation
    /// state so the per-frame `set_animated_color(GradientStopColor)`
    /// writer can rebuild inline CSS without re-walking the
    /// stylesheet. No-op when the rules carry no gradient.
    ///
    /// Called from BOTH `impl_apply_style` and
    /// `impl_apply_styled_states` — the walker picks one based on
    /// `Backend::handles_states_natively`, and we want the snapshot
    /// to land identically either way. Keeping the logic in one
    /// place prevents the two paths from drifting (every prior
    /// drift left animations broken on whichever path didn't get
    /// the latest changes).
    pub(crate) fn snapshot_gradient_for_animation(
        &mut self,
        id: u32,
        gradient: Option<&runtime_core::Gradient>,
    ) {
        let Some(g) = gradient else {
            return;
        };
        let mut stops = g.stops.clone();
        stops.sort_by(|a, b| {
            a.offset.partial_cmp(&b.offset).unwrap_or(std::cmp::Ordering::Equal)
        });
        let offsets: Vec<f32> = stops.iter().map(|s| s.offset).collect();
        let colors: Vec<[f32; 4]> = stops.iter().map(|s| color_to_srgb(&s.color)).collect();
        let shape = crate::animated::GradientShape {
            kind: match g.kind {
                runtime_core::GradientKind::Linear { angle_deg } => {
                    crate::animated::GradientShapeKind::Linear { angle_deg }
                }
                runtime_core::GradientKind::Radial { center, radius, extent } => {
                    crate::animated::GradientShapeKind::Radial { center, radius, extent }
                }
            },
            offsets,
        };
        let state = self.animated_states.entry(id).or_default();
        state.gradient_shape = Some(shape);
        state.gradient_stops = colors;
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

        // Background gradient: capture the shape (kind + per-stop
        // offsets) and the resolved stop colors onto the node's
        // animation state so the per-frame
        // `set_animated_color(GradientStopColor)` path can rebuild
        // the gradient CSS without re-walking the stylesheet. The
        // CSS class itself still carries the static gradient (so
        // initial paint hits the dedup cache); animation-driven
        // writes layer inline `style.backgroundImage` on top, which
        // CSS precedence resolves in favor of inline.
        self.snapshot_gradient_for_animation(id, style.background_gradient.as_ref());

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
        overlays: &[(runtime_core::StateBits, std::rc::Rc<StyleRules>)],
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

        // Background gradient: snapshot the shape (kind + offsets) +
        // resolved stop colors onto the node's animation state so the
        // per-frame `GradientStopColor` writer can rebuild inline CSS
        // without re-walking the stylesheet. The class itself still
        // carries the static gradient via the dedup cache; this just
        // priors the animated-states slot. Done BEFORE the fast-path
        // pregen returns because any of them might apply, and the
        // snapshot is independent of which class-application path
        // wins. (Mirrors the same block in `impl_apply_style` — the
        // walker dispatches to `apply_styled_states` whenever the
        // backend reports `handles_states_natively = true`, which is
        // every web view.)
        self.snapshot_gradient_for_animation(id, base.background_gradient.as_ref());

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
                    runtime_core::StateBits::HOVERED => ":hover",
                    runtime_core::StateBits::PRESSED => ":active",
                    runtime_core::StateBits::FOCUSED => ":focus",
                    runtime_core::StateBits::DISABLED => ":disabled",
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

    /// Mint a class name for a `StyleApplication` without applying
    /// it to any node. Used by the SignalClass walker path to
    /// pre-resolve the (value → class) table at mount.
    ///
    /// Same shape as the slow path in `impl_apply_styled_states`
    /// (resolve → check caches → mint fresh CSS rule if needed →
    /// stash in `dynamic_by_content` + `dynamic_by_ptr`), but
    /// stops short of the per-node bookkeeping (`DynamicSlot`,
    /// setAttribute). The caller — the JS-side binding dispatcher
    /// — does the actual `setAttribute` itself on signal writes.
    ///
    /// We bump the `refcount` on a hit so the rule survives until
    /// the binding releases it via `release_dynamic_rule`; that
    /// release happens through the binding's drop guard at scope
    /// teardown.
    pub(crate) fn impl_mint_class_for_app(
        &mut self,
        app: &runtime_core::StyleApplication,
    ) -> String {
        let resolved = runtime_core::resolve_style(app);
        let key = resolved.content_key();

        // 1. Existing dynamic-by-content hit: bump refcount + reuse.
        if let Some(entry) = self.dynamic_by_content.get(&key) {
            entry.shared.refcount.set(entry.shared.refcount.get() + 1);
            return entry.shared.class_name.clone();
        }

        // 2. Pregen hit (pre-registered stylesheet rules — usually
        // hit for unmodified static styles, rare for SignalClass
        // apps which typically carry `.override_*` content).
        if let Some(entry) = self.pregen.get(&key) {
            return entry.name.clone();
        }

        // 3. Mint fresh. State overlays aren't expressible through
        // the `signal_class` builder today, so we always emit just
        // the base rule (state_rule_indices empty).
        let class_name = hash_class_name(&key);
        let body = rules_to_css(&resolved);
        let rule_index = self.insert_rule(&class_name, &body);

        let shared = std::rc::Rc::new(DynamicPtrEntry {
            class_name: class_name.clone(),
            content_key: key.clone(),
            refcount: std::cell::Cell::new(1),
        });
        self.dynamic_by_content.insert(
            key,
            DynamicRule {
                shared: shared.clone(),
                rule_index,
                state_rule_indices: Vec::new(),
            },
        );
        self.dynamic_by_ptr
            .insert(std::rc::Rc::as_ptr(&resolved), shared);

        class_name
    }

    pub(crate) fn impl_on_node_unstyled(&mut self, node: &web_sys::Node) {
        // Resolve the JS-side id for this DOM node. `node_id` is
        // the single source of truth — going through it here means
        // teardown sees the same id `apply_style` stamped state
        // under, even though the Rust wrapper pointer has no
        // relationship to the underlying JS object identity.
        //
        // `node_id` always goes through the JS-side WeakMap (no
        // Rust-side cache, by design — see `WebBackend::node_id`)
        // so this costs one FFI hop per unstyle. Acceptable —
        // teardown isn't on the per-frame hot path.
        //
        // The sub-cleanups (`drop_dynamic_slot`, `state_listeners
        // .remove`, `impl_drop_animated_state`, `release_styled_node`)
        // are idempotent on missing keys, so unstyling a never-
        // styled node is harmlessly a no-op. The WeakMap entry
        // node_id allocates will auto-clear when the DOM element
        // is GC'd, so no explicit cleanup is needed for it either.
        let id = self.node_id(node);
        self.drop_dynamic_slot(id);
        self.state_listeners.remove(&id);
        self.impl_drop_animated_state(id);
        self.release_styled_node(id);
    }
}

// ---------------------------------------------------------------------------
// CSS value converters — free functions, no backend state.
// ---------------------------------------------------------------------------

/// Resolve a `runtime_core::Color` (a CSS-string wrapper) to a
/// concrete sRGB `[r, g, b, a]` in `0..=1`, used to seed the per-node
/// `gradient_stops` snapshot the animation path mutates. Parsing
/// lives in `runtime_core::color`; unknown shapes (named colors,
/// `hsl(...)`) fall back to opaque black — the CSS class still
/// renders them correctly; this is only the seed for the animation
/// state machine.
pub(crate) fn color_to_srgb(c: &runtime_core::Color) -> [f32; 4] {
    runtime_core::color::parse_or(&c.0, runtime_core::color::Rgba::BLACK).to_srgb_f32()
}


/// Short string tag for a `StateBits` flag, used as part of the
/// content key for state-bearing dynamic slots. Distinct tags ensure
/// distinct keys (and thus distinct minted class names) for
/// different state combinations.
fn state_bit_tag(b: runtime_core::StateBits) -> &'static str {
    match b {
        runtime_core::StateBits::HOVERED => "h",
        runtime_core::StateBits::PRESSED => "p",
        runtime_core::StateBits::FOCUSED => "f",
        runtime_core::StateBits::DISABLED => "d",
        _ => "?",
    }
}

