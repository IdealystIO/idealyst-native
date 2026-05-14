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
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Document, Node};

pub struct WebBackend {
    doc: Document,
    mount: web_sys::Element,
    _click_closures: Vec<Closure<dyn FnMut()>>,
    /// Per-node interaction-event closures. Keyed by node-id so we
    /// can drop them when `on_node_unstyled` fires. Each entry holds
    /// the listeners for one node (pointerenter, pointerleave,
    /// pointerdown, pointerup, focusin, focusout) plus the
    /// pointer-event-type closures so the JS side keeps them alive.
    state_listeners: HashMap<u32, Vec<Closure<dyn FnMut(web_sys::Event)>>>,
    /// Has the `@keyframes ui-spin` rule been injected? First
    /// ActivityIndicator creation injects it; later creations skip
    /// the work.
    spinner_keyframes_injected: bool,
    /// Has the `.ui-default` rule been injected? This rule encodes
    /// the framework's mobile-first defaults (display: flex;
    /// flex-direction: column) and is applied to every framework-
    /// created element alongside any user-provided class. Inserted
    /// first so its specificity is identical to user classes but
    /// its position is earlier — user classes win on overlap, the
    /// default fills the gaps.
    defaults_class_injected: bool,
    /// Has the virtualizer JS shim been injected? First Virtualizer
    /// creation injects `runtime/js/virtualizer.js` into a
    /// `<script>` tag in the document head.
    virtualizer_shim_injected: bool,
    /// Per-virtualizer JS instance map — keyed by node id so we can
    /// route `virtualizer_data_changed` to the right instance.
    virtualizer_instances: HashMap<u32, JsValue>,
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
    /// CSS rule index for the base rule. Always set.
    rule_index: u32,
    /// Additional rule indices for per-state pseudo-class overlays
    /// (`.cls:hover`, `:active`, `:focus`, `:disabled`). Empty for
    /// nodes without `state` blocks.
    state_rule_indices: Vec<u32>,
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
            state_listeners: HashMap::new(),
            spinner_keyframes_injected: false,
            defaults_class_injected: false,
            virtualizer_shim_injected: false,
            virtualizer_instances: HashMap::new(),
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
            for sidx in s.state_rule_indices.iter_mut() {
                *sidx += 1;
            }
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
            for sidx in s.state_rule_indices.iter_mut() {
                if *sidx > idx {
                    *sidx -= 1;
                }
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
        let el = self
            .doc
            .create_element("div")
            .expect("create_element failed");
        self.apply_default_class(&el);
        el.unchecked_into::<Node>()
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

    fn create_image(&mut self, src: &str, alt: Option<&str>) -> Self::Node {
        let img = self
            .doc
            .create_element("img")
            .expect("create_element img failed");
        let _ = img.set_attribute("src", src);
        if let Some(a) = alt {
            let _ = img.set_attribute("alt", a);
        }
        img.unchecked_into::<Node>()
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        if let Ok(el) = node.clone().dyn_into::<web_sys::Element>() {
            let _ = el.set_attribute("src", src);
        }
    }

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
    ) -> Self::Node {
        let input: web_sys::HtmlInputElement = self
            .doc
            .create_element("input")
            .expect("create_element input failed")
            .unchecked_into();
        input.set_type("text");
        input.set_value(initial_value);
        if let Some(p) = placeholder {
            input.set_placeholder(p);
        }
        // Wire native `input` event to the Rust callback. We use
        // `input` rather than `change` so every keystroke fires —
        // matching the controlled-component "single source of truth"
        // expectation.
        let input_clone = input.clone();
        let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            on_change(input_clone.value());
        });
        let _ = input.add_event_listener_with_callback(
            "input",
            closure.as_ref().unchecked_ref(),
        );
        // Stash closure under a fresh node id so it lives as long as
        // the node does. Reuse `state_listeners` map since it's the
        // existing per-node closure holder.
        let id = self.node_id(&input.clone().unchecked_into::<Node>());
        self.state_listeners
            .entry(id)
            .or_default()
            .push(closure);
        input.unchecked_into::<Node>()
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        if let Ok(input) = node.clone().dyn_into::<web_sys::HtmlInputElement>() {
            // Only write if different — avoids cursor-jump artifacts
            // when our own on_change wrote back to the signal.
            if input.value() != value {
                input.set_value(value);
            }
        }
    }

    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
    ) -> Self::Node {
        let input: web_sys::HtmlInputElement = self
            .doc
            .create_element("input")
            .expect("create_element input failed")
            .unchecked_into();
        input.set_type("checkbox");
        // role="switch" gives screen readers a switch UX even
        // though the underlying widget is a checkbox.
        let _ = input.set_attribute("role", "switch");
        input.set_checked(initial_value);
        let input_clone = input.clone();
        let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            on_change(input_clone.checked());
        });
        let _ = input.add_event_listener_with_callback(
            "change",
            closure.as_ref().unchecked_ref(),
        );
        let id = self.node_id(&input.clone().unchecked_into::<Node>());
        self.state_listeners
            .entry(id)
            .or_default()
            .push(closure);
        input.unchecked_into::<Node>()
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        if let Ok(input) = node.clone().dyn_into::<web_sys::HtmlInputElement>() {
            if input.checked() != value {
                input.set_checked(value);
            }
        }
    }

    fn create_scroll_view(&mut self, horizontal: bool) -> Self::Node {
        let div = self
            .doc
            .create_element("div")
            .expect("create_element div failed");
        self.apply_default_class(&div);
        // Apply the overflow style inline (not via the framework's
        // style system) so it's always present regardless of
        // user-supplied styling. The inline rules win over class
        // rules for the overflow properties; the class still
        // governs flex direction etc.
        let overflow = if horizontal { "overflow-x: auto; overflow-y: hidden" } else { "overflow-y: auto; overflow-x: hidden" };
        let _ = div.set_attribute("style", overflow);
        div.unchecked_into::<Node>()
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    ) -> Self::Node {
        let input: web_sys::HtmlInputElement = self
            .doc
            .create_element("input")
            .expect("create_element input failed")
            .unchecked_into();
        input.set_type("range");
        let _ = input.set_attribute("min", &min.to_string());
        let _ = input.set_attribute("max", &max.to_string());
        if let Some(s) = step {
            let _ = input.set_attribute("step", &s.to_string());
        } else {
            // "any" enables continuous values in the browser.
            let _ = input.set_attribute("step", "any");
        }
        input.set_value(&initial_value.to_string());

        // Fire on every `input` event (continuous drag).
        let input_clone = input.clone();
        let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            // Parse the string value back to f32; bail on parse error
            // (shouldn't happen with a range input).
            if let Ok(v) = input_clone.value().parse::<f32>() {
                on_change(v);
            }
        });
        let _ = input.add_event_listener_with_callback("input", closure.as_ref().unchecked_ref());
        let id = self.node_id(&input.clone().unchecked_into::<Node>());
        self.state_listeners.entry(id).or_default().push(closure);
        input.unchecked_into::<Node>()
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        if let Ok(input) = node.clone().dyn_into::<web_sys::HtmlInputElement>() {
            let s = value.to_string();
            if input.value() != s {
                input.set_value(&s);
            }
        }
    }

    fn create_web_view(&mut self, url: &str) -> Self::Node {
        let iframe = self
            .doc
            .create_element("iframe")
            .expect("create_element iframe failed");
        let _ = iframe.set_attribute("src", url);
        // Minimal default styling: take a sensible size; authors can
        // override via .with_style(...).
        let _ = iframe.set_attribute("style", "width: 100%; height: 400px; border: 0");
        iframe.unchecked_into::<Node>()
    }

    fn update_web_view_url(&mut self, node: &Self::Node, url: &str) {
        if let Ok(el) = node.clone().dyn_into::<web_sys::Element>() {
            let _ = el.set_attribute("src", url);
        }
    }

    fn create_video(
        &mut self,
        src: &str,
        autoplay: bool,
        controls: bool,
        loop_playback: bool,
    ) -> Self::Node {
        let video = self
            .doc
            .create_element("video")
            .expect("create_element video failed");
        let _ = video.set_attribute("src", src);
        if autoplay {
            let _ = video.set_attribute("autoplay", "");
            // Most browsers require `muted` for autoplay to work
            // without user gesture; matches RN's autoplay-friendly
            // default.
            let _ = video.set_attribute("muted", "");
        }
        if controls {
            let _ = video.set_attribute("controls", "");
        }
        if loop_playback {
            let _ = video.set_attribute("loop", "");
        }
        video.unchecked_into::<Node>()
    }

    fn update_video_src(&mut self, node: &Self::Node, src: &str) {
        if let Ok(el) = node.clone().dyn_into::<web_sys::Element>() {
            let _ = el.set_attribute("src", src);
        }
    }

    fn create_activity_indicator(
        &mut self,
        size: framework_core::primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&framework_core::Color>,
    ) -> Self::Node {
        // Inject the keyframes rule once. Subsequent creations reuse
        // the same rule by checking a static flag in the style sheet.
        self.ensure_spinner_keyframes();

        let span = self
            .doc
            .create_element("span")
            .expect("create_element span failed");
        let diameter = match size {
            framework_core::primitives::activity_indicator::ActivityIndicatorSize::Small => 16,
            framework_core::primitives::activity_indicator::ActivityIndicatorSize::Large => 36,
        };
        let accent = color
            .map(|c| c.0.as_str())
            .unwrap_or("currentColor");
        // Inline style: thin ring, accent on top, animated rotation.
        // Authors can override via .with_style(...) — these are just
        // defaults so the spinner renders meaningfully without one.
        let style = format!(
            "display: inline-block; width: {d}px; height: {d}px; \
             border: 2px solid transparent; border-top-color: {c}; \
             border-radius: 50%; animation: ui-spin 0.8s linear infinite",
            d = diameter,
            c = accent
        );
        let _ = span.set_attribute("style", &style);
        span.unchecked_into::<Node>()
    }

    fn create_virtualizer(
        &mut self,
        callbacks: framework_core::VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        horizontal: bool,
    ) -> Self::Node {
        // 1) Make sure the JS-side recycler shim is in the page.
        self.ensure_virtualizer_shim();

        // 2) Create the outer scrolling container.
        let container = self
            .doc
            .create_element("div")
            .expect("create_element div failed");
        let container_node: Node = container.clone().unchecked_into();
        let id = self.node_id(&container_node);

        // 3) Build the JS callbacks object. Each Rust callback is
        //    wrapped in a Closure so JS can invoke it; we keep the
        //    closures alive in `virtualizer_closures[id]`.
        //
        //    NOTE: js-sys-typed Closures are FnMut even when the
        //    underlying Rust closure is Fn — that's fine, we just
        //    invoke through the immutable signature.

        // Closures are kept alive by attaching them as JS-side
        // properties on the Virtualizer instance below; the
        // instance owns them for its lifetime. Rust-side we just
        // construct, `.forget()`, and let JS hold the references.

        let item_count_cb = {
            let f = callbacks.item_count.clone();
            Closure::<dyn FnMut() -> JsValue>::new(move || {
                JsValue::from_f64(f() as f64)
            })
        };
        let item_count_js = item_count_cb.as_ref().clone();

        let item_key_cb = {
            let f = callbacks.item_key.clone();
            Closure::<dyn FnMut(JsValue) -> JsValue>::new(move |idx: JsValue| {
                let i = idx.as_f64().unwrap_or(0.0) as usize;
                // Item key is a u64; JS numbers handle up to 2^53.
                JsValue::from_f64(f(i) as f64)
            })
        };
        let item_key_js = item_key_cb.as_ref().clone();

        let item_size_cb = {
            let f = callbacks.item_size.clone();
            Closure::<dyn FnMut(JsValue) -> JsValue>::new(move |idx: JsValue| {
                let i = idx.as_f64().unwrap_or(0.0) as usize;
                JsValue::from_f64(f(i) as f64)
            })
        };
        let item_size_js = item_size_cb.as_ref().clone();

        let mount_item_cb = {
            let f = callbacks.mount_item.clone();
            Closure::<dyn FnMut(JsValue) -> JsValue>::new(move |idx: JsValue| {
                let i = idx.as_f64().unwrap_or(0.0) as usize;
                let (node, scope_id) = f(i);
                // Return a 2-element array: [node, scopeId].
                let arr = js_sys::Array::new_with_length(2);
                arr.set(0, node.into());
                arr.set(1, JsValue::from_f64(scope_id as f64));
                arr.into()
            })
        };
        let mount_item_js = mount_item_cb.as_ref().clone();

        let release_item_cb = {
            let f = callbacks.release_item.clone();
            Closure::<dyn FnMut(JsValue)>::new(move |scope_id: JsValue| {
                let id = scope_id.as_f64().unwrap_or(0.0) as u64;
                f(id);
            })
        };
        let release_item_js = release_item_cb.as_ref().clone();

        let set_measured_size_cb = {
            let f = callbacks.set_measured_size.clone();
            Closure::<dyn FnMut(JsValue, JsValue)>::new(
                move |scope_id: JsValue, size: JsValue| {
                    let id = scope_id.as_f64().unwrap_or(0.0) as u64;
                    let sz = size.as_f64().unwrap_or(0.0) as f32;
                    f(id, sz);
                },
            )
        };
        let set_measured_size_js = set_measured_size_cb.as_ref().clone();

        // Build the callbacks object.
        let cb_obj = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&cb_obj, &JsValue::from_str("itemCount"), &item_count_js);
        let _ = js_sys::Reflect::set(&cb_obj, &JsValue::from_str("itemKey"), &item_key_js);
        let _ = js_sys::Reflect::set(&cb_obj, &JsValue::from_str("itemSize"), &item_size_js);
        let _ = js_sys::Reflect::set(&cb_obj, &JsValue::from_str("mountItem"), &mount_item_js);
        let _ = js_sys::Reflect::set(&cb_obj, &JsValue::from_str("releaseItem"), &release_item_js);
        let _ = js_sys::Reflect::set(
            &cb_obj,
            &JsValue::from_str("setMeasuredSize"),
            &set_measured_size_js,
        );
        let _ = js_sys::Reflect::set(
            &cb_obj,
            &JsValue::from_str("measureSizes"),
            &JsValue::from_bool(callbacks.measure_sizes),
        );
        let _ = js_sys::Reflect::set(
            &cb_obj,
            &JsValue::from_str("overscan"),
            &JsValue::from_f64(overscan as f64),
        );
        let _ = js_sys::Reflect::set(
            &cb_obj,
            &JsValue::from_str("horizontal"),
            &JsValue::from_bool(horizontal),
        );

        // 4) Construct the Virtualizer JS class.
        let window = web_sys::window().expect("no window");
        let ctor_raw = match js_sys::Reflect::get(&window, &JsValue::from_str("__idealystVirtualizer")) {
            Ok(v) => v,
            Err(e) => {
                web_sys::console::error_2(
                    &JsValue::from_str("[virtualizer] Reflect::get(window, __idealystVirtualizer) failed:"),
                    &e,
                );
                panic!("Reflect::get failed");
            }
        };
        if ctor_raw.is_undefined() || ctor_raw.is_null() {
            web_sys::console::error_1(&JsValue::from_str(
                "[virtualizer] window.__idealystVirtualizer is undefined/null — shim never installed",
            ));
            panic!("shim missing");
        }
        if !ctor_raw.is_function() {
            web_sys::console::error_2(
                &JsValue::from_str("[virtualizer] window.__idealystVirtualizer is not a function. Value:"),
                &ctor_raw,
            );
            panic!("shim not a function");
        }
        let ctor: js_sys::Function = ctor_raw.unchecked_into();
        let args = js_sys::Array::new_with_length(2);
        args.set(0, container.clone().into());
        args.set(1, cb_obj.into());
        let instance = match js_sys::Reflect::construct(&ctor, &args) {
            Ok(v) => v,
            Err(e) => {
                web_sys::console::error_2(
                    &JsValue::from_str("[virtualizer] Reflect::construct(Virtualizer) failed:"),
                    &e,
                );
                panic!("construct failed");
            }
        };

        // 5) Keep the closures alive — leak via a side store. The
        //    closures get dropped (and thus invalidate the JS-side
        //    function references) when the virtualizer's
        //    `on_node_unstyled` cleans up. The closures hold the Rcs
        //    so the underlying framework callbacks survive too.
        //
        //    We type-erase by `forget`ing each into a Vec<()> sentinel
        //    that we identify by node id. The real fix is per-closure
        //    storage; this Vec is heterogeneous and we hold them via
        //    forget. Simpler: hold them in the JS instance object
        //    itself by setting properties — JS keeps them alive.
        let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_item_count"), item_count_cb.as_ref());
        let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_item_key"), item_key_cb.as_ref());
        let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_item_size"), item_size_cb.as_ref());
        let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_mount"), mount_item_cb.as_ref());
        let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_release"), release_item_cb.as_ref());
        let _ = js_sys::Reflect::set(&instance, &JsValue::from_str("_rust_cb_set_size"), set_measured_size_cb.as_ref());
        // Then forget the Rust-side Closure wrappers — JS now holds
        // them via the instance properties, so they'll live as long
        // as the JS instance.
        item_count_cb.forget();
        item_key_cb.forget();
        item_size_cb.forget();
        mount_item_cb.forget();
        release_item_cb.forget();
        set_measured_size_cb.forget();

        // Store the JS instance so virtualizer_data_changed can find it.
        self.virtualizer_instances.insert(id, instance);

        container_node
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        let p: *const web_sys::Node = node;
        let Some(&id) = self.node_ids.get(&p) else { return };
        let Some(instance) = self.virtualizer_instances.get(&id) else { return };
        let _ = js_sys::Reflect::get(instance, &JsValue::from_str("dataChanged"))
            .ok()
            .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
            .map(|f| f.call0(instance));
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

    /// Web handles interaction states via CSS pseudo-classes
    /// (`:hover`, `:active`, `:focus`, `:disabled`) — the browser
    /// tracks transitions natively and no Rust-side state signal is
    /// needed. The framework calls `apply_styled_states` instead of
    /// `apply_style` when this returns true.
    fn handles_states_natively(&self) -> bool {
        true
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(framework_core::StateBits, Rc<StyleRules>)],
    ) {
        let Ok(element) = node.clone().dyn_into::<web_sys::Element>() else {
            return;
        };
        let id = self.node_id(node);

        // Mint a dedicated dynamic class for the base rules, with
        // pseudo-class overlay rules attached for each declared state.
        // Even when the base content key matches a pre-generated class,
        // we still need separate state overlay rules — the pre-gen
        // path only mints base classes, so we always take the dynamic
        // path when state overlays exist.
        //
        // Key: include the overlay states in the cache key so distinct
        // (base, overlays) combinations get distinct class names. We
        // do this by concatenating each overlay's content_key with a
        // pseudo-class tag.
        let mut key = base.content_key();
        for (bit, ov) in overlays {
            key.push(';');
            key.push_str(state_bit_tag(*bit));
            key.push(':');
            key.push_str(&ov.content_key());
        }

        let class_name = hash_class_name(&key);
        // Insert the base rule.
        let base_body = rules_to_css(base);
        let base_idx = self.insert_rule(&class_name, &base_body);

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
            let body = rules_to_css(overlay);
            let idx = self.insert_rule(&selector, &body);
            state_indices.push(idx);
        }

        let _ = element.set_attribute("class", &class_name);

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
            self.delete_rule(old.rule_index);
            for idx in old.state_rule_indices {
                self.delete_rule(idx);
            }
        }
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
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

    fn make_text_input_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::text_input::TextInputHandle {
        let input: web_sys::HtmlInputElement = node
            .clone()
            .dyn_into()
            .expect("text_input node is not an HtmlInputElement");
        framework_core::primitives::text_input::TextInputHandle::new(
            Rc::new(input),
            &WebTextInputOps,
        )
    }

    fn make_scroll_view_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::scroll_view::ScrollViewHandle {
        let el: web_sys::HtmlElement = node
            .clone()
            .dyn_into()
            .expect("scroll_view node is not an HtmlElement");
        framework_core::primitives::scroll_view::ScrollViewHandle::new(
            Rc::new(el),
            &WebScrollViewOps,
        )
    }

    fn make_video_handle(
        &self,
        node: &Self::Node,
    ) -> framework_core::primitives::video::VideoHandle {
        // `HtmlMediaElement` exposes play/pause/currentTime, so we
        // downcast to that. Both `<video>` and `<audio>` are
        // HtmlMediaElement subclasses.
        let el: web_sys::HtmlMediaElement = node
            .clone()
            .dyn_into()
            .expect("video node is not an HtmlMediaElement");
        framework_core::primitives::video::VideoHandle::new(
            Rc::new(el),
            &WebVideoOps,
        )
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

struct WebButtonOps;
impl ButtonOps for WebButtonOps {
    fn click(&self, node: &dyn Any) {
        if let Some(html) = node.downcast_ref::<web_sys::HtmlElement>() {
            html.click();
        }
    }
}

struct WebTextInputOps;
impl framework_core::primitives::text_input::TextInputOps for WebTextInputOps {
    fn focus(&self, node: &dyn Any) {
        if let Some(input) = node.downcast_ref::<web_sys::HtmlInputElement>() {
            let _ = input.focus();
        }
    }
    fn blur(&self, node: &dyn Any) {
        if let Some(input) = node.downcast_ref::<web_sys::HtmlInputElement>() {
            let _ = input.blur();
        }
    }
    fn select_all(&self, node: &dyn Any) {
        if let Some(input) = node.downcast_ref::<web_sys::HtmlInputElement>() {
            input.select();
        }
    }
}

struct WebScrollViewOps;
impl framework_core::primitives::scroll_view::ScrollViewOps for WebScrollViewOps {
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
        if let Some(html) = node.downcast_ref::<web_sys::HtmlElement>() {
            html.set_scroll_left(x as i32);
            html.set_scroll_top(y as i32);
        }
    }
}

struct WebVideoOps;
impl framework_core::primitives::video::VideoOps for WebVideoOps {
    fn play(&self, node: &dyn Any) {
        if let Some(v) = node.downcast_ref::<web_sys::HtmlMediaElement>() {
            // play() returns a Promise; we ignore it. Browsers may
            // reject if autoplay rules block playback — caller can
            // catch via JS if they care, not worth surfacing here.
            let _ = v.play();
        }
    }
    fn pause(&self, node: &dyn Any) {
        if let Some(v) = node.downcast_ref::<web_sys::HtmlMediaElement>() {
            let _ = v.pause();
        }
    }
    fn seek(&self, node: &dyn Any, seconds: f32) {
        if let Some(v) = node.downcast_ref::<web_sys::HtmlMediaElement>() {
            v.set_current_time(seconds as f64);
        }
    }
}

impl WebBackend {
    /// Inject `@keyframes ui-spin` into the stylesheet on first use.
    /// Subsequent ActivityIndicator constructions reuse the same
    /// keyframes — the rule is identity-stable, no need to re-create.
    /// Inject the `.ui-default` rule the first time a framework
    /// element is created. The rule encodes the framework's
    /// mobile-first defaults so every node gets `display: flex;
    /// flex-direction: column` even before any user style is
    /// applied. User-minted classes override on overlap because
    /// they're inserted later (later wins at equal specificity).
    fn ensure_defaults_class(&mut self) {
        if self.defaults_class_injected {
            return;
        }
        // We use `insert_rule` directly rather than the framework's
        // own class minting because this rule isn't owned by any
        // particular stylesheet — it's a global baseline. Bump
        // recorded indices on the existing per-rule caches since
        // this insertion shifts every existing rule up by 1.
        let rule = ".ui-default { display: flex; flex-direction: column; }";
        let _ = self.sheet().insert_rule(rule);
        for e in self.pregen.values_mut() {
            e.rule_index += 1;
        }
        for s in self.dynamic.values_mut() {
            s.rule_index += 1;
            for sidx in s.state_rule_indices.iter_mut() {
                *sidx += 1;
            }
        }
        self.defaults_class_injected = true;
    }

    /// Attach the framework's default class to a freshly created
    /// element. `apply_style` later concatenates the user-minted
    /// class alongside this one — see the className-merge logic
    /// inside `apply_style`.
    fn apply_default_class(&mut self, element: &web_sys::Element) {
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
    fn ensure_virtualizer_shim(&mut self) {
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

    fn ensure_spinner_keyframes(&mut self) {
        if self.spinner_keyframes_injected {
            return;
        }
        let rule = "@keyframes ui-spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }";
        let _ = self.sheet().insert_rule(rule);
        // The rule's insertion shifts every recorded index up by 1.
        // The existing per-rule cache machinery does this for normal
        // class rules; @keyframes is a different rule type and
        // doesn't currently bump indices, so we manually do it here.
        for e in self.pregen.values_mut() {
            e.rule_index += 1;
        }
        for s in self.dynamic.values_mut() {
            s.rule_index += 1;
            for sidx in s.state_rule_indices.iter_mut() {
                *sidx += 1;
            }
        }
        self.spinner_keyframes_injected = true;
    }

    /// Removes a node's dynamic slot, if any, and deletes its CSS rules
    /// (base + any per-state pseudo-class overlays).
    fn drop_dynamic_slot(&mut self, id: u32) {
        if let Some(slot) = self.dynamic.remove(&id) {
            self.delete_rule(slot.rule_index);
            for idx in slot.state_rule_indices {
                self.delete_rule(idx);
            }
        }
    }
}
