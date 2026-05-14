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

use framework_core::{Backend, ButtonHandle, ButtonOps, Color, StyleRules};
use std::any::Any;
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

/// Compile a `StyleRules` to a CSS body. RN-style: every styled node
/// is implicitly `display: flex`, so the emitter always prepends that.
/// Per-side padding/margin/border are emitted as their CSS long-form
/// (`padding-top`, etc.) — the browser handles them just like the
/// shorthand, but we get exact-match cache keys.
fn rules_to_css(rules: &StyleRules) -> String {
    let mut parts: Vec<String> = Vec::new();

    // RN-style: every styled view is a flex container.
    parts.push("display: flex".to_string());

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

    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        // The node was created via `create_element("button")` then
        // upcast to `Node`. Cast it back to `HtmlElement` so the ops
        // table can call `.click()` on it. The clone is cheap — it's
        // a wasm-bindgen JsValue clone (refcount bump on the JS
        // object handle, no DOM duplication).
        let html: web_sys::HtmlElement = node
            .clone()
            .dyn_into()
            .expect("button node is not an HtmlElement");
        ButtonHandle::new(Rc::new(html), &WebButtonOps)
    }

    fn finish(&mut self, root: Self::Node) {
        self.mount
            .append_child(&root)
            .expect("mount append failed");
    }
}

/// `ButtonOps` impl for the web backend. The `node` parameter comes
/// from the `ButtonHandle`'s internal `Rc<dyn Any>`, which we built
/// out of an `HtmlElement` in `make_button_handle`. Downcast back to
/// invoke the DOM API.
struct WebButtonOps;
impl ButtonOps for WebButtonOps {
    fn click(&self, node: &dyn Any) {
        if let Some(html) = node.downcast_ref::<web_sys::HtmlElement>() {
            html.click();
        }
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
