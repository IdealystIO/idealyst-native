//! FULL-OP golden harness (P2b exit gate): a recorder that logs the
//! complete backend-call stream — `create_*` / `update_*` / `apply_style`
//! / handler installs / `release_*` — not just the 7 structural ops, so
//! the vocabulary's generic handlers can be proven to emit the SAME
//! sequences as the old walker.
//!
//! # The recorded alphabet
//!
//! [`FullRecorder`] overrides exactly the Backend methods both mount
//! paths model in P2/P3c; everything else stays at the trait default
//! (unrecorded). The P3c style-engine gate added `install_tokens` /
//! `update_tokens` (theme delivery + cohort-swap ordering) and
//! `attach_html_class` (preminted class stamping) — none fire in the
//! pre-P3c scenarios, so their goldens are unchanged. Deliberately
//! OUTSIDE the alphabet, with rationale:
//!
//! - `register_stylesheet`/`unregister_stylesheet` — the fixtures mint
//!   a fresh `Rc<StyleSheet>` per closure fire (the common inline
//!   shape), so this stream pins Rc-lifetime churn (dead-Weak sweep
//!   timing) rather than style semantics; and the two sides register
//!   through different engines (runtime-core's thread-local table vs
//!   the vocabulary's per-world table) whose sweep POINTS legitimately
//!   differ. Registration behavior on the new path is pinned by
//!   `runtime-vocabulary/tests/vocab.rs`'s sheet-path suite instead.
//! - `apply_default_text_font` — document-level premint plumbing with
//!   no per-node observable; pinned by the vocabulary suite.
//! - `WireBindingOps` notes (`note_text_binding`, …) — declarative
//!   wire/generator backends only; scenarios use opaque closures, which
//!   skip them on the old side too.
//! - `finish`/`run_layout`/`set_page_metadata`/robot/introspection —
//!   host-lifecycle plumbing outside the mount contract (P3 drivers).
//!
//! Args render compactly and deterministically: closures as `<fn>`,
//! `StyleRules` as a one-line non-`None`-fields digest
//! ([`rules_digest`]), `AccessibilityProps` omitted when default.
//!
//! Both sides share this ONE recorder type: the old walker drives it as
//! a `runtime_core::Backend`; the new core drives it through
//! `LegacyBridge<FullRecorder>` (whose `Host` + caps impls delegate to
//! these same Backend methods), so a matching run produces byte-equal
//! step bodies.

use std::path::PathBuf;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives;
use runtime_core::primitives::icon::IconData;
use runtime_core::{Backend, Color, Easing, StateBits, StyleRules};

use crate::{Mode, PNode, Recorder};

// ===========================================================================
// Arg rendering
// ===========================================================================

/// One-line digest of resolved `StyleRules`: the derived Debug output
/// with every `field: None` entry dropped (depth-aware split, so nested
/// `Some(..)` payloads render fully). Deterministic: derived Debug
/// follows declaration order, and both sides digest equal structs.
pub fn rules_digest(rules: &StyleRules) -> String {
    compact_struct_debug(&format!("{rules:?}"))
}

fn compact_struct_debug(dbg: &str) -> String {
    let Some(open) = dbg.find('{') else {
        return dbg.to_string();
    };
    let close = dbg.rfind('}').unwrap_or(dbg.len());
    let inner = &dbg[open + 1..close];
    let mut segs: Vec<&str> = Vec::new();
    let mut level = 0i32;
    let mut in_str = false;
    let mut prev = ' ';
    let mut seg_start = 0usize;
    for (i, ch) in inner.char_indices() {
        match ch {
            '"' if prev != '\\' => in_str = !in_str,
            '{' | '(' | '[' if !in_str => level += 1,
            '}' | ')' | ']' if !in_str => level -= 1,
            ',' if !in_str && level == 0 => {
                segs.push(inner[seg_start..i].trim());
                seg_start = i + 1;
            }
            _ => {}
        }
        prev = ch;
    }
    let tail = inner[seg_start..].trim();
    if !tail.is_empty() {
        segs.push(tail);
    }
    let kept: Vec<&str> = segs.into_iter().filter(|s| !s.ends_with(": None")).collect();
    format!("{{{}}}", kept.join(", "))
}

/// ` a11y={..}` suffix, or empty for the default props (the common
/// case, keeping golden lines short).
fn a11y_suffix(a11y: &AccessibilityProps) -> String {
    let dbg = format!("{a11y:?}");
    if dbg == format!("{:?}", AccessibilityProps::default()) {
        String::new()
    } else {
        format!(" a11y={}", compact_struct_debug(&dbg))
    }
}

fn icon_digest(data: &IconData) -> String {
    format!(
        "icon(vb={:?} paths={} filled={})",
        data.view_box,
        data.paths.len(),
        data.filled
    )
}

fn opt_str(v: Option<&str>) -> String {
    match v {
        Some(s) => format!("{s:?}"),
        None => "none".to_string(),
    }
}

// ===========================================================================
// FullRecorder
// ===========================================================================

/// The full-op recording backend. See the module docs for the alphabet
/// contract.
pub struct FullRecorder {
    pub(crate) rec: Recorder,
    pub(crate) splice: bool,
    /// Captured button press callbacks, in creation order — lets an
    /// integration test fire an authored `on_click` exactly as a real
    /// backend would (proving the macro → builders → handlers → backend
    /// wiring end to end). See [`FullRecorder::button_action`].
    actions: Vec<(String, Rc<dyn Fn()>)>,
    /// Captured `attach_states` setters, in attach order — lets a style
    /// scenario flip interaction bits exactly as a native event source
    /// would (the P3c state-overlay gate). See
    /// [`FullRecorder::state_setter`].
    state_setters: Vec<Rc<dyn Fn(StateBits, bool)>>,
}

impl FullRecorder {
    pub fn new(rec: Recorder, mode: Mode) -> Self {
        FullRecorder {
            rec,
            splice: matches!(mode, Mode::Spliced),
            actions: Vec::new(),
            state_setters: Vec::new(),
        }
    }

    /// The `nth` captured interaction-state setter (attach order).
    /// Panics loudly when out of range.
    pub fn state_setter(&self, nth: usize) -> Rc<dyn Fn(StateBits, bool)> {
        self.state_setters
            .get(nth)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "no state setter #{nth}; {} were captured",
                    self.state_setters.len()
                )
            })
    }

    /// The press callback of the most recently created button whose
    /// label matched at creation time. Panics (with the known labels)
    /// when nothing matched — tests want that loud.
    pub fn button_action(&self, label: &str) -> Rc<dyn Fn()> {
        self.actions
            .iter()
            .rev()
            .find(|(l, _)| l == label)
            .map(|(_, f)| f.clone())
            .unwrap_or_else(|| {
                panic!(
                    "no recorded button labeled {label:?}; known: {:?}",
                    self.actions.iter().map(|(l, _)| l).collect::<Vec<_>>()
                )
            })
    }
}

impl Backend for FullRecorder {
    type Node = PNode;

    // --- structural (same line format as ParityBackend/SceneHost) ---

    fn insert(&mut self, parent: &mut PNode, child: PNode) {
        self.rec.push(format!("insert {parent} <- {child}"));
    }

    fn insert_many(&mut self, parent: &mut PNode, children: Vec<PNode>) {
        let list = children
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        self.rec.push(format!("insert_many {parent} <- [{list}]"));
    }

    fn insert_at(&mut self, parent: &mut PNode, child: PNode, index: usize) {
        self.rec
            .push(format!("insert_at {parent} <- {child} @ {index}"));
    }

    fn remove_child(&mut self, parent: &PNode, child: &PNode) {
        self.rec.push(format!("remove_child {parent} -x {child}"));
    }

    fn clear_children(&mut self, node: &PNode) {
        self.rec.push(format!("clear_children {node}"));
    }

    fn create_reactive_anchor(&mut self) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!("create {n} anchor"));
        n
    }

    fn supports_child_splice(&self) -> bool {
        self.splice
    }

    // --- view / containers ---

    fn create_view(&mut self, a11y: &AccessibilityProps) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!("create {n} view{}", a11y_suffix(a11y)));
        n
    }

    fn mark_container(&mut self, node: &PNode) {
        self.rec.push(format!("mark_container {node}"));
    }

    fn create_pressable(
        &mut self,
        _on_click: Rc<dyn Fn()>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec
            .push(format!("create {n} pressable on_click=<fn>{}", a11y_suffix(a11y)));
        n
    }

    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} scroll_view horizontal={horizontal} on_scroll={}{}",
            if on_scroll.is_some() { "<fn>" } else { "none" },
            a11y_suffix(a11y)
        ));
        n
    }

    // --- input channels ---

    fn install_touch_handler(&mut self, node: &PNode, _handler: runtime_core::TouchHandler) {
        self.rec.push(format!("install_touch_handler {node}"));
    }

    fn install_wheel_handler(&mut self, node: &PNode, _handler: runtime_core::WheelHandler) {
        self.rec.push(format!("install_wheel_handler {node}"));
    }

    fn install_hover_handler(&mut self, node: &PNode, _handler: runtime_core::HoverHandler) {
        self.rec.push(format!("install_hover_handler {node}"));
    }

    fn install_file_drop_handler(&mut self, node: &PNode, _handler: runtime_core::FileDropHandler) {
        self.rec.push(format!("install_file_drop_handler {node}"));
    }

    fn mark_preserves_focus(&mut self, node: &PNode) {
        self.rec.push(format!("mark_preserves_focus {node}"));
    }

    // --- text ---

    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> PNode {
        let n = self.rec.mint();
        self.rec
            .push(format!("create {n} text {content:?}{}", a11y_suffix(a11y)));
        n
    }

    fn create_styled_text(&mut self, runs: &[runtime_core::TextRun], a11y: &AccessibilityProps) -> PNode {
        let n = self.rec.mint();
        let plain = runtime_core::styled_text::plain_text_of(runs);
        self.rec.push(format!(
            "create {n} styled_text {plain:?} runs={}{}",
            runs.len(),
            a11y_suffix(a11y)
        ));
        n
    }

    /// Opted-in: bound texts take the batched-id fast path so the
    /// goldens pin `update_text_by_id`/`release_text_id`. The id is the
    /// node's creation index.
    fn create_text_with_id(
        &mut self,
        content: &str,
        a11y: &AccessibilityProps,
    ) -> Option<(PNode, u32)> {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} text+id{} {content:?}{}",
            n.0,
            a11y_suffix(a11y)
        ));
        Some((n, n.0))
    }

    fn update_text(&mut self, node: &PNode, content: &str) {
        self.rec.push(format!("update_text {node} {content:?}"));
    }

    fn update_text_by_id(&mut self, id: u32, content: String) {
        self.rec.push(format!("update_text_by_id id{id} {content:?}"));
    }

    fn release_text_id(&mut self, id: u32) {
        self.rec.push(format!("release_text_id id{id}"));
    }

    // --- button ---

    fn create_button(
        &mut self,
        label: &str,
        on_click: &runtime_core::Action,
        leading_icon: Option<&IconData>,
        trailing_icon: Option<&IconData>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.actions.push((label.to_string(), on_click.fire.clone()));
        let n = self.rec.mint();
        let mut line = format!("create {n} button {label:?} on_click=<fn>");
        if let Some(icon) = leading_icon {
            line.push_str(&format!(" leading={}", icon_digest(icon)));
        }
        if let Some(icon) = trailing_icon {
            line.push_str(&format!(" trailing={}", icon_digest(icon)));
        }
        line.push_str(&a11y_suffix(a11y));
        self.rec.push(line);
        n
    }

    fn update_button_label(&mut self, node: &PNode, label: &str) {
        self.rec.push(format!("update_button_label {node} {label:?}"));
    }

    // --- image ---

    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} image src={src:?} alt={}{}",
            opt_str(alt),
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_image_src(&mut self, node: &PNode, src: &str) {
        self.rec.push(format!("update_image_src {node} {src:?}"));
    }

    fn update_image_alt(&mut self, node: &PNode, alt: Option<&str>) {
        self.rec
            .push(format!("update_image_alt {node} {}", opt_str(alt)));
    }

    fn install_image_load_handler(&mut self, node: &PNode, _handler: runtime_core::ImageLoadHandler) {
        self.rec.push(format!("install_image_load_handler {node}"));
    }

    fn install_image_error_handler(
        &mut self,
        node: &PNode,
        _handler: runtime_core::ImageErrorHandler,
    ) {
        self.rec.push(format!("install_image_error_handler {node}"));
    }

    fn register_asset(
        &mut self,
        id: runtime_core::assets::AssetId,
        kind: runtime_core::assets::AssetTag,
        _source: &runtime_core::assets::AssetSource,
    ) {
        self.rec
            .push(format!("register_asset id={} kind={kind:?}", id.0));
    }

    // --- icon ---

    fn create_icon(
        &mut self,
        data: &IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} {} color={}{}",
            icon_digest(data),
            color.map(|c| c.0.clone()).unwrap_or_else(|| "none".into()),
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_icon_color(&mut self, node: &PNode, color: &Color) {
        self.rec.push(format!("update_icon_color {node} {}", color.0));
    }

    fn update_icon_data(&mut self, node: &PNode, data: &IconData) {
        self.rec
            .push(format!("update_icon_data {node} {}", icon_digest(data)));
    }

    fn update_icon_stroke(&mut self, node: &PNode, progress: f32) {
        self.rec
            .push(format!("update_icon_stroke {node} {progress}"));
    }

    fn animate_icon_stroke(
        &mut self,
        node: &PNode,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        self.rec.push(format!(
            "animate_icon_stroke {node} {from}->{to} {duration_ms}ms {easing:?} infinite={infinite} autoreverses={autoreverses}"
        ));
    }

    // --- toggle / slider / activity ---

    fn create_toggle(
        &mut self,
        initial_value: bool,
        _on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} toggle {initial_value} on_change=<fn>{}",
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_toggle_value(&mut self, node: &PNode, value: bool) {
        self.rec.push(format!("update_toggle_value {node} {value}"));
    }

    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        _on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} slider {initial_value} [{min},{max}] step={} on_change=<fn>{}",
            step.map(|s| s.to_string()).unwrap_or_else(|| "none".into()),
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_slider_value(&mut self, node: &PNode, value: f32) {
        self.rec.push(format!("update_slider_value {node} {value}"));
    }

    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} activity_indicator {size:?} color={}{}",
            color.map(|c| c.0.clone()).unwrap_or_else(|| "none".into()),
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &PNode,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        self.rec
            .push(format!("update_activity_indicator_size {node} {size:?}"));
    }

    // --- link ---

    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} link route={:?} url={:?} external={} on_activate=<fn>{}",
            config.route,
            config.url,
            config.external,
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_link_url(&mut self, node: &PNode, url: &str) {
        self.rec.push(format!("update_link_url {node} {url:?}"));
    }

    // --- text input / area ---

    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        secure: bool,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} text_input {initial_value:?} placeholder={} secure={secure} on_change=<fn> on_key_down={} on_blur={}{}",
            opt_str(placeholder),
            if on_key_down.is_some() { "<fn>" } else { "none" },
            if on_blur.is_some() { "<fn>" } else { "none" },
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_text_input_value(&mut self, node: &PNode, value: &str) {
        self.rec
            .push(format!("update_text_input_value {node} {value:?}"));
    }

    fn update_text_input_secure(&mut self, node: &PNode, secure: bool) {
        self.rec
            .push(format!("update_text_input_secure {node} {secure}"));
    }

    fn update_text_input_placeholder(&mut self, node: &PNode, placeholder: Option<&str>) {
        self.rec.push(format!(
            "update_text_input_placeholder {node} {}",
            opt_str(placeholder)
        ));
    }

    fn set_text_input_focus_handler(&mut self, node: &PNode, _handler: Rc<dyn Fn(bool)>) {
        self.rec
            .push(format!("set_text_input_focus_handler {node}"));
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        _on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} text_area {initial_value:?} placeholder={} wrap={wrap} rows=[{:?},{:?}] on_change=<fn> on_key_down={}{}",
            opt_str(placeholder),
            min_rows,
            max_rows,
            if on_key_down.is_some() { "<fn>" } else { "none" },
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_text_area_value(&mut self, node: &PNode, value: &str) {
        self.rec
            .push(format!("update_text_area_value {node} {value:?}"));
    }

    // --- style ---

    fn apply_style(&mut self, node: &PNode, style: &Rc<StyleRules>) {
        self.rec
            .push(format!("apply_style {node} {}", rules_digest(style)));
    }

    fn on_node_unstyled(&mut self, node: &PNode) {
        self.rec.push(format!("on_node_unstyled {node}"));
    }

    fn attach_states(&mut self, node: &PNode, setter: Rc<dyn Fn(StateBits, bool)>) {
        self.state_setters.push(setter);
        self.rec.push(format!("attach_states {node}"));
    }

    // --- P3c style-engine alphabet: theme-token delivery + preminted
    //     class stamping. None of these fire in the pre-P3c scenarios
    //     (no tokens installed, no Preminted styles), so adding them
    //     left the existing goldens untouched. ---

    fn install_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        let names: Vec<&str> = tokens.iter().map(|t| t.name).collect();
        self.rec.push(format!("install_tokens {names:?}"));
    }

    fn update_tokens(&mut self, tokens: &[runtime_core::TokenEntry]) {
        let names: Vec<&str> = tokens.iter().map(|t| t.name).collect();
        self.rec.push(format!("update_tokens {names:?}"));
    }

    fn supports_preminted_styles(&self) -> bool {
        true
    }

    fn attach_html_class(&self, node: &PNode, class: &str) {
        self.rec.push(format!("attach_html_class {node} {class}"));
    }

    fn set_disabled(&mut self, node: &PNode, disabled: bool) {
        self.rec.push(format!("set_disabled {node} {disabled}"));
    }

    fn apply_safe_area_padding(&mut self, node: &PNode, sides: runtime_core::SafeAreaSides) {
        self.rec
            .push(format!("apply_safe_area_padding {node} {sides:?}"));
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &PNode, sides: runtime_core::SafeAreaSides) {
        self.rec
            .push(format!("apply_scroll_view_safe_area_inset {node} {sides:?}"));
    }

    // --- lifecycle: required but out of alphabet ---

    fn finish(&mut self, _root: PNode) {}
}

// ===========================================================================
// Shared fixtures — used by BOTH scenario files so the two sides build
// identical prop values (digest equality depends on it).
// ===========================================================================

/// Test rules: `width: <w>px` + a literal background color. The old
/// side wraps these in a static `StyleSheet`; resolution returns the
/// same struct (literals resolve to themselves), so both sides digest
/// identically.
pub fn test_rules(width: f32, background: &str) -> StyleRules {
    StyleRules {
        width: Some(runtime_core::Tokenized::Literal(runtime_core::Length::Px(width))),
        background: Some(runtime_core::Tokenized::Literal(Color(background.to_string()))),
        ..Default::default()
    }
}

/// A fixed icon for the scenarios.
pub const TEST_ICON: IconData = IconData {
    view_box: (24, 24),
    paths: &["M0 0h24v24H0z"],
    fill_rule: primitives::icon::FillRule::NonZero,
    filled: false,
};

// --- P3c style-engine fixtures (shared: both sides must resolve to
//     byte-equal digests; the SHEET types are runtime-core's on both
//     cores, so the constructors are literally shared) ---

use runtime_core::{StyleApplication, StyleSheet, TokenEntry, TokenValue, Tokenized};

/// The theme token both style scenarios swap.
pub fn surface_token(value: &str) -> TokenEntry {
    TokenEntry {
        name: "color-surface",
        value: TokenValue::Color(Color(value.to_string())),
    }
}

/// A `stylesheet!`-shaped sheet: token-referencing base (background via
/// `color-surface`) + a `size` variant with a `large` arm and a
/// defaulted `medium` arm.
pub fn themed_sheet() -> Rc<StyleSheet> {
    fn base() -> StyleRules {
        StyleRules {
            width: Some(Tokenized::Literal(runtime_core::Length::Px(100.0))),
            background: Some(Tokenized::token("color-surface", Color("#000".into()))),
            ..Default::default()
        }
    }
    Rc::new(
        StyleSheet::new(|_vs| base())
            .variant("size", "large", |_vs| StyleRules {
                width: Some(Tokenized::Literal(runtime_core::Length::Px(400.0))),
                ..Default::default()
            })
            .variant("size", "medium", |_vs| StyleRules::default())
            .variant_default("size", "medium"),
    )
}

/// A sheet with a `state hovered { … }` overlay (the macro's reserved
/// `__state_hovered` axis) — the static-divert + overlay-flip fixture.
pub fn hover_sheet() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_vs| StyleRules {
            width: Some(Tokenized::Literal(runtime_core::Length::Px(100.0))),
            ..Default::default()
        })
        .variant("__state_hovered", "on", |_vs| StyleRules {
            width: Some(Tokenized::Literal(runtime_core::Length::Px(999.0))),
            ..Default::default()
        }),
    )
}

/// The signal-class mapping: value 0 → a 60px sheet, value 1 → 200px.
/// A fresh static sheet per call, matching the common inline shape.
pub fn class_app(v: u32) -> StyleApplication {
    let width = if v == 0 { 60.0 } else { 200.0 };
    StyleApplication::new(Rc::new(StyleSheet::r#static(test_rules(width, "#606060"))))
}

// ===========================================================================
// Golden paths
// ===========================================================================

/// Absolute path of a full-op scenario's shared golden (owned by the
/// OLD side, like `goldens/`).
pub fn full_golden_path(name: &str, mode: Mode) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("goldens_full")
        .join(format!("{name}.{}.golden", mode.suffix()))
}

/// Absolute path of a full-op NEW-core override golden (sanctioned
/// divergences only — see `full_new::FULL_NEWCORE_OVERRIDES`).
pub fn full_newcore_golden_path(name: &str, mode: Mode) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("goldens_full_newcore")
        .join(format!("{name}.{}.golden", mode.suffix()))
}

// ===========================================================================
// Old-side scenario driver + golden comparison
// ===========================================================================

use std::cell::RefCell;

use runtime_core::{Element, Owner};

use crate::scenarios_full::full_scenarios;
use crate::{serialize_steps, Step};

/// A full-op parity scenario (same shape as [`crate::Scenario`], typed
/// to the full-op drivers).
pub struct FullScenario {
    pub name: &'static str,
    /// Header lines written into the golden file.
    pub about: &'static [&'static str],
    pub modes: &'static [Mode],
    pub run: fn(&mut FullCx),
}

/// Old-core full-op scenario driver — [`crate::Cx`] with the
/// [`FullRecorder`] backend. The old core is synchronous, so every op
/// lands inside `step`'s closure.
pub struct FullCx {
    rec: Recorder,
    backend: Rc<RefCell<FullRecorder>>,
    owner: Option<Owner>,
    steps: Vec<Step>,
}

impl FullCx {
    pub fn recorder(&self) -> Recorder {
        self.rec.clone()
    }

    /// The `nth` captured interaction-state setter (P3c overlay-flip
    /// scenarios drive hover exactly as a native event source would).
    pub fn state_setter(&self, nth: usize) -> Rc<dyn Fn(StateBits, bool)> {
        self.backend.borrow().state_setter(nth)
    }

    pub fn mount(&mut self, root: Element) {
        assert!(self.owner.is_none(), "scenario mounted twice");
        let owner = runtime_core::render(self.backend.clone(), root);
        self.owner = Some(owner);
        self.snap("mount");
    }

    pub fn step(&mut self, label: &str, f: impl FnOnce()) {
        f();
        self.snap(label);
    }

    fn snap(&mut self, label: &str) {
        self.steps.push(Step {
            label: label.to_string(),
            ops: self.rec.take_ops(),
        });
    }
}

/// Run `scenario` in `mode` against the OLD core, returning the
/// serialized golden text (with header).
pub fn run_full_scenario(scenario: &FullScenario, mode: Mode) -> String {
    let rec = Recorder::default();
    let backend = Rc::new(RefCell::new(FullRecorder::new(rec.clone(), mode)));
    let mut cx = FullCx {
        rec,
        backend,
        owner: None,
        steps: Vec::new(),
    };
    (scenario.run)(&mut cx);
    let steps = std::mem::take(&mut cx.steps);
    // Owner drop (full unmount) stays outside the pinned sequence —
    // sanctioned divergence #4; scenario `full_release_on_swap` pins
    // release ordering through a BRANCH swap instead, which IS inside a
    // step.
    drop(cx);
    let mut out = String::new();
    out.push_str(&format!(
        "# scene-parity FULL-OP golden — scenario `{}`, mode `{}`\n#\n",
        scenario.name,
        mode.suffix()
    ));
    for line in scenario.about {
        out.push_str(&format!("# {line}\n"));
    }
    out.push_str(
        "#\n\
         # Complete backend-call stream (create_*/update_*/apply_style/release_*\n\
         # + the structural ops). Node names are creation-order (n0, n1, ...).\n\
         # Owned by the OLD core; regenerate after an INTENDED walker change\n\
         # with: UPDATE_GOLDENS=1 cargo test -p scene-parity\n",
    );
    out.push_str(&serialize_steps(&steps));
    out
}

/// Old-side test entry point: byte-for-byte against `goldens_full/`.
/// `UPDATE_GOLDENS=1` (re)writes.
pub fn check_full(name: &str, mode: Mode) {
    let all = full_scenarios();
    let scenario = all
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no full-op scenario named `{name}`"));
    assert!(
        scenario.modes.contains(&mode),
        "full-op scenario `{name}` is not registered for mode {mode:?}",
    );

    let actual = run_full_scenario(scenario, mode);
    let path = full_golden_path(name, mode);

    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("create goldens_full dir");
        std::fs::write(&path, &actual).expect("write golden");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing full-op golden {}\nGenerate it with: UPDATE_GOLDENS=1 cargo test -p scene-parity",
            path.display()
        )
    });

    if expected != actual {
        panic!(
            "FULL-OP golden mismatch for scenario `{name}` (mode {mode:?}).\n\
             If the walker's behavior changed INTENTIONALLY, regenerate with\n\
             UPDATE_GOLDENS=1 cargo test -p scene-parity and review the diff.\n\
             \n--- expected ({path}) ---\n{expected}\n--- actual ---\n{actual}",
            path = path.display(),
        );
    }
}
