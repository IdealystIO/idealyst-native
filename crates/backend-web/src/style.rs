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
use crate::{DynamicSlot, PregenEntry, WebBackend};
use framework_core::{Color, StyleRules};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use wasm_bindgen::JsCast;

// ---------------------------------------------------------------------------
// Stylesheet rule-index management — split out of `impl WebBackend`.
// ---------------------------------------------------------------------------

impl WebBackend {
    /// Lazily creates the shared `<style>` element in document.head.
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
        let Ok(element) = node.clone().dyn_into::<web_sys::Element>() else {
            return;
        };
        let id = self.node_id(node);
        let key = style.content_key();

        // Path 1: pre-generated cache hit.
        if let Some(entry) = self.pregen.get(&key) {
            let class_name = entry.name.clone();
            element.set_attribute("class", &class_name).expect("set class");
            // If we had a dynamic class previously, remove it now —
            // the pre-generated one is what's active.
            self.drop_dynamic_slot(id);
            return;
        }

        // Path 2: dynamic mint. One class per node, replace atomically.
        let class_name = hash_class_name(&key);
        let body = rules_to_css(style);
        let new_index = self.insert_rule(&class_name, &body);
        element.set_attribute("class", &class_name).expect("set class");

        // Now remove the previously-applied dynamic class for this node,
        // if any. Order matters: we inserted before deleting so the
        // sheet always has the active class through the swap.
        let prev = self.dynamic.insert(
            id,
            DynamicSlot {
                name: class_name,
                rule_index: new_index,
                state_rule_indices: Vec::new(),
            },
        );
        if let Some(old) = prev {
            self.delete_rule(old.rule_index);
            // Delete any state-overlay rules from the previous slot.
            for idx in old.state_rule_indices {
                self.delete_rule(idx);
            }
        }
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

        let Ok(element) = node.clone().dyn_into::<web_sys::Element>() else {
            return;
        };
        let id = self.node_id(node);

        // Fast-fast path: pointer-keyed pregen hit. When the
        // framework's resolution cache returns the same
        // `Rc<StyleRules>` for many nodes (the 10k-row case, where
        // every "even" row gets one shared Rc), we can identify the
        // class by Rc identity without computing `content_key()` at
        // all. The pointer is stable for the Rc's lifetime; the
        // pregen_by_ptr map is populated alongside content-keyed
        // pregen during `register_stylesheet`.
        if overlays.is_empty() {
            let ptr = std::rc::Rc::as_ptr(base);
            let ptr_hit = {
                let _t = PhaseTimer::start("pregen_lookup_ptr");
                self.pregen_by_ptr.get(&ptr).cloned()
            };
            if let Some(class_name) = ptr_hit {
                {
                    let _t = PhaseTimer::start("set_attribute_fast");
                    element.set_attribute("class", &class_name).expect("set class");
                }
                {
                    let _t = PhaseTimer::start("drop_dynamic_slot");
                    self.drop_dynamic_slot(id);
                }
                return;
            }
        }

        // Content-keyed fast path. Used when the Rc identity didn't
        // hit (e.g. the user passed `.override_*` builder methods
        // that produce a fresh Rc each time, but with content
        // identical to a pre-gen entry).
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
                {
                    let _t = PhaseTimer::start("set_attribute_fast");
                    element.set_attribute("class", &class_name).expect("set class");
                }
                {
                    let _t = PhaseTimer::start("drop_dynamic_slot");
                    self.drop_dynamic_slot(id);
                }
                return;
            }
        }

        // Slow path: state overlays present, or no pregen hit. Mint a
        // dedicated dynamic class for the base rules, with
        // pseudo-class overlay rules attached for each declared state.
        // Even when the base content key matches a pre-generated
        // class, the presence of state overlays forces us to mint a
        // fresh class because the pregen path only emits base classes.
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

        let class_name = {
            let _t = PhaseTimer::start("hash_class_name");
            hash_class_name(&key)
        };
        // Insert the base rule.
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
            // We emit just the overlay's rules — the browser already
            // applies the base class, and pseudo-class rules with
            // matching specificity layered on top override only the
            // properties they declare.
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

        {
            let _t = PhaseTimer::start("set_attribute_slow");
            let _ = element.set_attribute("class", &class_name);
        }

        // Swap in the new dynamic slot; delete the previous one's
        // rules (base + states).
        let prev = self.dynamic.insert(
            id,
            DynamicSlot {
                name: class_name,
                rule_index: base_idx,
                state_rule_indices: state_indices,
            },
        );
        if let Some(old) = prev {
            let _t = PhaseTimer::start("delete_rule");
            self.delete_rule(old.rule_index);
            for idx in old.state_rule_indices {
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

/// Compile a `StyleRules` to a CSS body. RN-style: every styled node
/// is implicitly `display: flex`, so the emitter always prepends that.
/// Per-side padding/margin/border are emitted as their CSS long-form
/// (`padding-top`, etc.) — the browser handles them just like the
/// shorthand, but we get exact-match cache keys.
pub(crate) fn rules_to_css(rules: &StyleRules) -> String {
    let mut parts: Vec<String> = Vec::new();

    // RN-style: every styled view is a flex container. We also force
    // `flex-direction: column` when the rules don't pin it themselves
    // — CSS's own default is `row`, which would diverge from the
    // framework's mobile-first default. Either the explicit rule (set
    // below) or this default applies, never both.
    parts.push("display: flex".to_string());
    if rules.flex_direction.is_none() {
        parts.push("flex-direction: column".to_string());
    }

    // Color + text.
    if let Some(Color(c)) = &rules.background { parts.push(format!("background: {}", c)); }
    if let Some(Color(c)) = &rules.color { parts.push(format!("color: {}", c)); }
    if let Some(v) = rules.font_size { parts.push(format!("font-size: {}", length_css(v))); }

    // Flex container.
    if let Some(v) = rules.flex_direction { parts.push(format!("flex-direction: {}", flex_direction_css(v))); }
    if let Some(v) = rules.flex_wrap { parts.push(format!("flex-wrap: {}", flex_wrap_css(v))); }
    if let Some(v) = rules.justify_content { parts.push(format!("justify-content: {}", justify_content_css(v))); }
    if let Some(v) = rules.align_items { parts.push(format!("align-items: {}", align_items_css(v))); }
    if let Some(v) = rules.align_content { parts.push(format!("align-content: {}", align_content_css(v))); }
    if let Some(v) = rules.gap { parts.push(format!("gap: {}", length_css(v))); }
    if let Some(v) = rules.row_gap { parts.push(format!("row-gap: {}", length_css(v))); }
    if let Some(v) = rules.column_gap { parts.push(format!("column-gap: {}", length_css(v))); }

    // Flex item.
    if let Some(v) = rules.flex_grow { parts.push(format!("flex-grow: {}", v)); }
    if let Some(v) = rules.flex_shrink { parts.push(format!("flex-shrink: {}", v)); }
    if let Some(v) = rules.flex_basis { parts.push(format!("flex-basis: {}", length_css(v))); }
    if let Some(v) = rules.align_self { parts.push(format!("align-self: {}", align_self_css(v))); }

    // Sizing.
    if let Some(v) = rules.width { parts.push(format!("width: {}", length_css(v))); }
    if let Some(v) = rules.height { parts.push(format!("height: {}", length_css(v))); }
    if let Some(v) = rules.min_width { parts.push(format!("min-width: {}", length_css(v))); }
    if let Some(v) = rules.min_height { parts.push(format!("min-height: {}", length_css(v))); }
    if let Some(v) = rules.max_width { parts.push(format!("max-width: {}", length_css(v))); }
    if let Some(v) = rules.max_height { parts.push(format!("max-height: {}", length_css(v))); }

    // Per-side padding.
    if let Some(v) = rules.padding_top { parts.push(format!("padding-top: {}", length_css(v))); }
    if let Some(v) = rules.padding_right { parts.push(format!("padding-right: {}", length_css(v))); }
    if let Some(v) = rules.padding_bottom { parts.push(format!("padding-bottom: {}", length_css(v))); }
    if let Some(v) = rules.padding_left { parts.push(format!("padding-left: {}", length_css(v))); }

    // Per-side margin.
    if let Some(v) = rules.margin_top { parts.push(format!("margin-top: {}", length_css(v))); }
    if let Some(v) = rules.margin_right { parts.push(format!("margin-right: {}", length_css(v))); }
    if let Some(v) = rules.margin_bottom { parts.push(format!("margin-bottom: {}", length_css(v))); }
    if let Some(v) = rules.margin_left { parts.push(format!("margin-left: {}", length_css(v))); }

    // Per-corner border radius.
    if let Some(v) = rules.border_top_left_radius { parts.push(format!("border-top-left-radius: {}", length_css(v))); }
    if let Some(v) = rules.border_top_right_radius { parts.push(format!("border-top-right-radius: {}", length_css(v))); }
    if let Some(v) = rules.border_bottom_left_radius { parts.push(format!("border-bottom-left-radius: {}", length_css(v))); }
    if let Some(v) = rules.border_bottom_right_radius { parts.push(format!("border-bottom-right-radius: {}", length_css(v))); }

    // Per-side border width + color. Emit `solid` style so the browser
    // actually paints the line.
    if let Some(v) = rules.border_top_width { parts.push(format!("border-top-width: {}px", v)); parts.push("border-top-style: solid".to_string()); }
    if let Some(v) = rules.border_right_width { parts.push(format!("border-right-width: {}px", v)); parts.push("border-right-style: solid".to_string()); }
    if let Some(v) = rules.border_bottom_width { parts.push(format!("border-bottom-width: {}px", v)); parts.push("border-bottom-style: solid".to_string()); }
    if let Some(v) = rules.border_left_width { parts.push(format!("border-left-width: {}px", v)); parts.push("border-left-style: solid".to_string()); }
    if let Some(Color(c)) = &rules.border_top_color { parts.push(format!("border-top-color: {}", c)); }
    if let Some(Color(c)) = &rules.border_right_color { parts.push(format!("border-right-color: {}", c)); }
    if let Some(Color(c)) = &rules.border_bottom_color { parts.push(format!("border-bottom-color: {}", c)); }
    if let Some(Color(c)) = &rules.border_left_color { parts.push(format!("border-left-color: {}", c)); }

    // Position.
    if let Some(v) = rules.position { parts.push(format!("position: {}", position_css(v))); }
    if let Some(v) = rules.top { parts.push(format!("top: {}", length_css(v))); }
    if let Some(v) = rules.right { parts.push(format!("right: {}", length_css(v))); }
    if let Some(v) = rules.bottom { parts.push(format!("bottom: {}", length_css(v))); }
    if let Some(v) = rules.left { parts.push(format!("left: {}", length_css(v))); }

    // Typography.
    if let Some(ff) = &rules.font_family { parts.push(format!("font-family: {}", ff)); }
    if let Some(v) = rules.font_weight { parts.push(format!("font-weight: {}", font_weight_css(v))); }
    if let Some(v) = rules.font_style { parts.push(format!("font-style: {}", font_style_css(v))); }
    if let Some(v) = rules.line_height { parts.push(format!("line-height: {}px", v)); }
    if let Some(v) = rules.letter_spacing { parts.push(format!("letter-spacing: {}px", v)); }
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
    if let Some(v) = rules.opacity { parts.push(format!("opacity: {}", v)); }
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
