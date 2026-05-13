//! Web backend: drives DOM nodes via web-sys/wasm-bindgen.
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

use framework_core::{Backend, Border, Color, StyleRules};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Document, Node};

pub struct WebBackend {
    doc: Document,
    mount: web_sys::Element,
    _click_closures: Vec<Closure<dyn FnMut()>>,
    /// Shared `<style>` element holding every active CSS rule.
    style_element: Option<web_sys::HtmlStyleElement>,
    /// Pre-generated classes from `register_stylesheet`. Content-keyed,
    /// shared, refcounted (refcount tracks how many active
    /// registrations hold them — not how many nodes apply them).
    pregen: HashMap<String, PregenEntry>,
    /// Per-node dynamic class slot — `node_id -> (class_name, rule_index)`.
    /// At most one dynamic class per node. Replaced atomically when
    /// the node's resolved style changes.
    dynamic: HashMap<u32, DynamicSlot>,
    /// Stable per-Node id derived from the Node's pointer.
    next_node_id: u32,
    node_ids: HashMap<*const web_sys::Node, u32>,
}

struct PregenEntry {
    name: String,
    rule_index: u32,
    refcount: u32,
}

struct DynamicSlot {
    /// Kept for debugging — same hash that's set on the element's class.
    #[allow(dead_code)]
    name: String,
    rule_index: u32,
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
            style_element: None,
            pregen: HashMap::new(),
            dynamic: HashMap::new(),
            next_node_id: 0,
            node_ids: HashMap::new(),
        }
    }

    /// Assigns a stable per-Node id we use as a key in `dynamic`.
    fn node_id(&mut self, node: &Node) -> u32 {
        let p: *const web_sys::Node = node;
        if let Some(&id) = self.node_ids.get(&p) {
            return id;
        }
        let id = self.next_node_id;
        self.next_node_id += 1;
        self.node_ids.insert(p, id);
        id
    }

    /// Lazily creates the shared `<style>` element in document.head.
    fn ensure_style_element(&mut self) -> web_sys::HtmlStyleElement {
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

    fn sheet(&mut self) -> web_sys::CssStyleSheet {
        let elem = self.ensure_style_element();
        elem.sheet()
            .expect("style element has no sheet")
            .unchecked_into::<web_sys::CssStyleSheet>()
    }

    /// Insert a CSS rule into the shared sheet. Returns the rule's
    /// index (always 0 — `CSSStyleSheet.insertRule` defaults to
    /// inserting at the beginning). Shifts every previously-recorded
    /// index up by 1 to stay in sync with the live sheet.
    fn insert_rule(&mut self, class_name: &str, body: &str) -> u32 {
        // Manual concatenation to avoid the `format!` machinery, which
        // monomorphizes a path through `Display` and pulls more code
        // into the binary than this simple join needs.
        let mut rule = String::with_capacity(class_name.len() + body.len() + 6);
        rule.push('.');
        rule.push_str(class_name);
        rule.push_str(" { ");
        rule.push_str(body);
        rule.push_str(" }");
        let new_index = self.sheet().insert_rule(&rule).expect("insert_rule failed");
        // Every existing rule's index shifted up by `new_index + 1`.
        // For insertRule with no index argument, new_index is always 0,
        // so every existing rule shifts up by 1.
        for e in self.pregen.values_mut() {
            e.rule_index += 1;
        }
        for s in self.dynamic.values_mut() {
            s.rule_index += 1;
        }
        new_index
    }

    /// Delete a CSS rule at the given index, then shift every
    /// recorded index above it down by 1 to stay in sync.
    fn delete_rule(&mut self, idx: u32) {
        let _ = self.sheet().delete_rule(idx);
        for e in self.pregen.values_mut() {
            if e.rule_index > idx {
                e.rule_index -= 1;
            }
        }
        for s in self.dynamic.values_mut() {
            if s.rule_index > idx {
                s.rule_index -= 1;
            }
        }
    }
}

/// Derive a deterministic class name from a content key. Same content
/// always produces the same name across sessions. 16 hex chars from
/// std DefaultHasher.
fn hash_class_name(content_key: &str) -> String {
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

fn rules_to_css(rules: &StyleRules) -> String {
    let mut parts = Vec::new();
    if let Some(Color(c)) = &rules.background {
        parts.push(format!("background: {}", c));
    }
    if let Some(Color(c)) = &rules.color {
        parts.push(format!("color: {}", c));
    }
    if let Some(p) = rules.padding {
        parts.push(format!("padding: {}px", p));
    }
    if let Some(s) = rules.font_size {
        parts.push(format!("font-size: {}px", s));
    }
    if let Some(r) = rules.border_radius {
        parts.push(format!("border-radius: {}px", r));
    }
    if let Some(Border { width, color }) = &rules.border {
        parts.push(format!("border: {}px solid {}", width, color.0));
    }
    parts.join("; ")
}

impl Backend for WebBackend {
    type Node = Node;

    fn create_view(&mut self) -> Self::Node {
        self.doc
            .create_element("div")
            .expect("create_element failed")
            .unchecked_into::<Node>()
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
        // Wrap text in a `<span>` so style application via `class` works
        // uniformly. A raw DOM text node has no `class`/`style`
        // attributes, so styling would be silently dropped.
        let span = self
            .doc
            .create_element("span")
            .expect("create_element span failed");
        span.set_text_content(Some(content));
        span.unchecked_into::<Node>()
    }

    fn create_button(&mut self, label: &str, on_click: Rc<dyn Fn()>) -> Self::Node {
        let button = self
            .doc
            .create_element("button")
            .expect("create button")
            .unchecked_into::<web_sys::HtmlElement>();
        button.set_text_content(Some(label));
        let closure = Closure::<dyn FnMut()>::new(move || on_click());
        button.set_onclick(Some(closure.as_ref().unchecked_ref()));
        self._click_closures.push(closure);
        button.unchecked_into::<Node>()
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        parent.append_child(&child).expect("append_child failed");
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        // Works for both Element (e.g. our <span>) and Text node cases.
        node.set_text_content(Some(content));
    }

    fn clear_children(&mut self, node: &Self::Node) {
        while let Some(child) = node.first_child() {
            node.remove_child(&child).expect("remove_child failed");
        }
    }

    /// Pre-generation: for each rule, look up or mint a class.
    /// Pre-generated classes have a `refcount` that bumps once per
    /// registration; they're removed when refcount hits zero via
    /// `unregister_stylesheet`.
    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        for r in rules {
            let key = r.content_key();
            if let Some(entry) = self.pregen.get_mut(&key) {
                entry.refcount += 1;
                continue;
            }
            let class_name = hash_class_name(&key);
            let body = rules_to_css(r);
            let rule_index = self.insert_rule(&class_name, &body);
            self.pregen.insert(
                key,
                PregenEntry {
                    name: class_name,
                    rule_index,
                    refcount: 1,
                },
            );
        }
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        for r in rules {
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
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
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
            },
        );
        if let Some(old) = prev {
            self.delete_rule(old.rule_index);
        }
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        // Look up the node's id without minting a new one (we don't
        // want spurious id allocations during teardown).
        let p: *const web_sys::Node = node;
        if let Some(&id) = self.node_ids.get(&p) {
            // Drop the dynamic slot (deletes its CSS rule if any).
            self.drop_dynamic_slot(id);
            // Remove the node-id mapping itself.
            self.node_ids.remove(&p);
        }
    }

    fn finish(&mut self, root: Self::Node) {
        self.mount
            .append_child(&root)
            .expect("mount append failed");
    }
}

impl WebBackend {
    /// Removes a node's dynamic slot, if any, and deletes its CSS rule.
    fn drop_dynamic_slot(&mut self, id: u32) {
        if let Some(slot) = self.dynamic.remove(&id) {
            self.delete_rule(slot.rule_index);
        }
    }
}
