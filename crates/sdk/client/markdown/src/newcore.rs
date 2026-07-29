//! New-core surface — the scene-registry leg (idea-lite migration).
//!
//! The old-core leg is an `Element::External` payload dispatched through
//! the per-backend `ExternalRegistry`; that registry does not exist on
//! the new core. The scene [`Registry`] is the new core's unified
//! primitive==external contract, so this leg registers a payload handler
//! there instead.
//!
//! The old WEB handler (`src/web.rs::build`) was already generic over
//! the old `Backend` trait — semantic DOM built through trait calls, no
//! `web_sys` — so its new-core port is a caps-GENERIC scene handler
//! (the codeblock class, not svg's `WebBackend`-concrete split): ONE
//! [`register`] for every caps-complete host (`StyleServices` +
//! [`TextOps`]), translating the old handler method-for-method
//! (`Backend::create_element` → `DocumentOps::create_element`,
//! `Backend::create_text` → `TextOps::create_text`, literal-rules
//! `apply_style`, `insert`). Web AND SSR mount the identical semantic
//! DOM. Note this is an UPGRADE for SSR: on the old core the web
//! handler module was wasm32-gated, so the host-side `register`
//! fallback never installed it and old-core SSR rendered the
//! External-placeholder `<div>`; the new-core output is kept consistent
//! with the old WEB handler DOM (the byte-parity fence in
//! tests/newcore.rs), which is what SSR would have produced had the
//! generic handler been registered there.
//!
//! The old iOS/Android single-node native handlers (one `UILabel` /
//! `TextView` + attribute spans) stay old-core-only until the new-core
//! native boots grow an external story — the codeblock precedent; their
//! modules are `not(feature = "new-core")`-gated in lib.rs.
//!
//! The old-core wire machinery (`ensure_wire_serde` +
//! `register_external_serde`) belongs to the old recorder's
//! `CreateExternal` path and has no role here: the new-core dev-wire
//! path serializes through the recorder's caps (out of scope for this
//! SDK), so [`register`] installs no serde.
//!
//! Same authored call shape as the old-core leg: `ui! { Markdown(source
//! = …, theme = …) }` (struct-literal `BuildElement` dispatch through
//! the `pub type Markdown = MarkdownProps` alias) and the low-level
//! `markdown(src, theme).with_style(…).bind(r)` builder — a consuming
//! app compiles the same source against either core. App bootstrap
//! passes [`register`] as the boot registration seam (there is no
//! inventory self-registration on the new core; the registry is built
//! explicitly at boot).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{
    Color, FlexDirection, FontFamily, FontStyle, FontWeight, Length, Ref, StyleRules, Tokenized,
};
use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::TextOps;
use runtime_vocabulary::glue::{switch, BuildElement, IntoElement, Reactive};
use runtime_vocabulary::style_attach::{attach_style, IntoStyleProp, StyleProp, StyleServices};

use crate::ir::{MarkdownDoc, MdBlock, MdRun, MdTheme};

// ============================================================================
// Public API surface — same shapes as the old core.
// ============================================================================

/// Typed handle to a mounted markdown node — same name as the old-core
/// alias (`ExternalHandle<MarkdownDoc>`) so author `Ref<MarkdownHandle>`
/// declarations compile unchanged. Minimal on this leg: a repo-wide
/// sweep found NO consumer of the old handle beyond the alias itself,
/// so the handle carries the type-erased native node (keeping it
/// reachable for future imperative ops) and nothing else.
#[derive(Clone)]
pub struct MarkdownHandle {
    node: Rc<dyn Any>,
}

impl MarkdownHandle {
    /// The type-erased native node the handler mounted (the outer
    /// semantic-DOM `<div>` on caps-generic hosts). Downcast to the
    /// host's concrete node type for bespoke plumbing.
    pub fn node(&self) -> Rc<dyn Any> {
        self.node.clone()
    }
}

/// Props for the [`Markdown`] component.
///
/// Both props are [`Reactive`], so passing a live `Signal`/`rx!` (for
/// either the source text or the theme) makes the rendered document
/// update — the node is rebuilt on change.
pub struct MarkdownProps {
    /// The CommonMark/GFM source to render. Static or reactive.
    pub source: Reactive<String>,
    /// Per-element-type resolved styling. Default is [`MdTheme::light`];
    /// pass a reactive theme to follow an app theme toggle.
    pub theme: Reactive<MdTheme>,
}

impl Default for MarkdownProps {
    fn default() -> Self {
        Self {
            source: Reactive::Static(String::new()),
            theme: Reactive::Static(MdTheme::light()),
        }
    }
}

/// The `ui!` tag for the markdown component — struct-literal dispatch
/// (`Markdown(source = …)`) resolves through this alias to
/// [`MarkdownProps`]'s [`BuildElement`] impl, the same contract
/// `#[component]` generates on the old core.
pub type Markdown = MarkdownProps;

impl BuildElement for MarkdownProps {
    /// Reproduces the old-core `#[component] fn Markdown` body: a
    /// reactive region keyed on the `(source, theme)` tuple. `switch`
    /// re-runs the branch only when the tuple's `PartialEq` value
    /// actually differs (dedupe-on-equal-scrutinee), so static props
    /// build exactly once.
    fn build(self) -> Element {
        let source = self.source;
        let theme = self.theme;
        switch(
            move || (source.get(), theme.get()),
            move |key| {
                let (src, th) = key;
                markdown(src.clone(), th.clone()).into_element()
            },
        )
    }
}

// ============================================================================
// Payload + builder — the old `Bound<MarkdownHandle>` call shape
// (`.with_style(…)` / `.bind(…)` then element coercion).
// ============================================================================

/// Scene payload for the markdown item. Single-take slots (the
/// vocabulary `PrimCell` discipline, inlined): the scene hands the
/// handler a shared `&Rc<Self>`, but the style/ref-fill must move at
/// mount.
struct MarkdownPrim {
    doc: MarkdownDoc,
    style: RefCell<Option<StyleProp>>,
    ref_fill: RefCell<Option<Box<dyn FnOnce(Rc<dyn Any>)>>>,
}

/// Author-side builder returned by [`markdown`] — mirrors the old-core
/// `Bound<MarkdownHandle>` call shape.
pub struct MarkdownBound {
    doc: MarkdownDoc,
    style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(Rc<dyn Any>)>>,
}

/// Low-level builder: construct a markdown scene item from a source
/// string + resolved theme. Mirrors `codeblock::code_block` —
/// `.with_style(...)` lands on the outer node (the semantic-DOM
/// container `<div>`).
///
/// Prefer the [`Markdown`] component for reactive source/theme; this is
/// the escape hatch for one-shot rendering or custom plumbing. (No wire
/// serde registration on this leg — see the module docs.)
pub fn markdown(source: impl Into<String>, theme: MdTheme) -> MarkdownBound {
    let doc = crate::parse::parse(&source.into(), theme);
    MarkdownBound {
        doc,
        style: None,
        ref_fill: None,
    }
}

impl MarkdownBound {
    /// Attach the author style — lands on the outer node, exactly like
    /// `.with_style` on the old-core `Bound`.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }

    /// Bind a `Ref<MarkdownHandle>` for imperative access. At mount
    /// time the handler wraps the native node in a [`MarkdownHandle`]
    /// and fills the ref (old-core `RefFill::External` timing).
    pub fn bind(mut self, r: Ref<MarkdownHandle>) -> Self {
        self.ref_fill = Some(Box::new(move |node_any| {
            r.fill(MarkdownHandle { node: node_any });
        }));
        self
    }
}

impl IntoElement for MarkdownBound {
    fn into_element(self) -> Element {
        item(
            MarkdownPrim {
                doc: self.doc,
                style: RefCell::new(self.style),
                ref_fill: RefCell::new(self.ref_fill),
            },
            Vec::new(),
        )
    }
}

/// Mirror of the old core's `From<Bound<H>> for Element`.
impl From<MarkdownBound> for Element {
    fn from(b: MarkdownBound) -> Element {
        b.into_element()
    }
}

// ============================================================================
// Handler + registration seam — the old web handler, call-for-call,
// over the caps traits.
// ============================================================================

/// Vertical gap between blocks (px).
const BLOCK_GAP: f32 = 12.0;
/// Indent per list nesting level (px).
const LIST_INDENT: f32 = 20.0;

/// Register the markdown payload handler on a scene registry. Pass this
/// as the boot registration seam (the `register` argument of
/// `backend_web::newcore::start_in` / `backend_ssr::newcore::
/// render_path_with`) — ONE generic registration for every
/// caps-complete host, because the ported handler needs nothing beyond
/// the caps traits (semantic DOM, no `web_sys`).
pub fn register<H>(registry: &mut Registry<H>)
where
    H: StyleServices + TextOps + 'static,
{
    registry.register::<MarkdownPrim, _>(mount_markdown::<H>);
}

/// Mount handler — the old-core web handler (`web.rs::build`)
/// call-for-call: outer `<div>` column with the theme's base
/// color/size, one semantic element per block, per-run styled text
/// children, then the author style attached to the outer node (the old
/// walker order: style AFTER the DOM builds) and the ref filled. No
/// `release_external` teardown — the root comes from `create_element`,
/// not `create_external`, so teardown is the ordinary node removal
/// (codeblock posture for the caps-generic class).
fn mount_markdown<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<MarkdownPrim>,
    _children: Vec<Element>,
) -> H::Node
where
    H: StyleServices + TextOps,
{
    let backend = cx.backend().clone();
    let a11y = AccessibilityProps::default();
    let doc = &prim.doc;
    let theme = &doc.theme;

    let root = backend.borrow_mut().create_element("div");
    let mut rs = StyleRules::default();
    rs.flex_direction = Some(FlexDirection::Column);
    rs.gap = Some(Tokenized::Literal(Length::Px(BLOCK_GAP)));
    rs.color = Some(Tokenized::Literal(Color(theme.text.clone())));
    rs.font_size = Some(Tokenized::Literal(Length::Px(theme.base_size)));
    // NB: `line_height` here is absolute px in this framework, not a
    // unitless multiplier — leave it unset so each element gets the
    // browser's proportional `normal` line height (a fixed px value
    // would collapse multi-size text: too tight for headings).
    backend.borrow_mut().apply_style(&root, &Rc::new(rs));

    let mut root = root;
    for block in &doc.blocks {
        let node = build_block(&backend, block, theme, &a11y);
        backend.borrow_mut().insert(&mut root, node);
    }

    if let Some(style) = prim.style.borrow_mut().take() {
        attach_style(&backend, &root, style);
    }
    if let Some(fill) = prim.ref_fill.borrow_mut().take() {
        let any_node: Rc<dyn Any> = Rc::new(root.clone());
        fill(any_node);
    }
    root
}

fn build_block<H>(
    backend: &Rc<RefCell<H>>,
    block: &MdBlock,
    theme: &MdTheme,
    a11y: &AccessibilityProps,
) -> H::Node
where
    H: StyleServices + TextOps,
{
    match block {
        MdBlock::Heading { level, runs } => {
            let tag = format!("h{}", level.clamp(&1, &6));
            let node = backend.borrow_mut().create_element(&tag);
            let mut rs = StyleRules::default();
            rs.color = Some(Tokenized::Literal(Color(theme.heading.clone())));
            rs.font_size = Some(Tokenized::Literal(Length::Px(theme.heading_size(*level))));
            rs.font_weight = Some(FontWeight::Bold);
            // Zero the UA heading margins; the column `gap` owns spacing.
            zero_margins(&mut rs);
            backend.borrow_mut().apply_style(&node, &Rc::new(rs));
            append_runs(backend, &node, runs, theme, &theme.heading, a11y);
            node
        }
        MdBlock::Paragraph { runs } => {
            let node = backend.borrow_mut().create_element("p");
            let mut rs = StyleRules::default();
            zero_margins(&mut rs);
            backend.borrow_mut().apply_style(&node, &Rc::new(rs));
            append_runs(backend, &node, runs, theme, &theme.text, a11y);
            node
        }
        MdBlock::CodeBlock { text } => {
            let node = backend.borrow_mut().create_element("pre");
            let mut rs = StyleRules::default();
            rs.background = Some(Tokenized::Literal(Color(theme.code_bg.clone())));
            rs.color = Some(Tokenized::Literal(Color(theme.code_fg.clone())));
            rs.font_family = Some(mono_family(theme));
            rs.font_size = Some(Tokenized::Literal(Length::Px(theme.base_size * 0.9)));
            pad_all(&mut rs, 12.0);
            rs.border_top_left_radius = Some(Tokenized::Literal(Length::Px(6.0)));
            rs.border_top_right_radius = Some(Tokenized::Literal(Length::Px(6.0)));
            rs.border_bottom_left_radius = Some(Tokenized::Literal(Length::Px(6.0)));
            rs.border_bottom_right_radius = Some(Tokenized::Literal(Length::Px(6.0)));
            zero_margins(&mut rs);
            backend.borrow_mut().apply_style(&node, &Rc::new(rs));
            let mut node = node;
            let txt = backend.borrow_mut().create_text(text, a11y);
            backend.borrow_mut().insert(&mut node, txt);
            node
        }
        MdBlock::Quote { runs } => {
            let node = backend.borrow_mut().create_element("blockquote");
            let mut rs = StyleRules::default();
            rs.color = Some(Tokenized::Literal(Color(theme.quote_fg.clone())));
            rs.font_style = Some(FontStyle::Italic);
            rs.border_left_width = Some(Tokenized::Literal(3.0));
            rs.border_left_color = Some(Tokenized::Literal(Color(theme.muted.clone())));
            rs.padding_left = Some(Tokenized::Literal(Length::Px(12.0)));
            zero_margins(&mut rs);
            backend.borrow_mut().apply_style(&node, &Rc::new(rs));
            append_runs(backend, &node, runs, theme, &theme.quote_fg, a11y);
            node
        }
        MdBlock::List { ordered: _, items } => {
            let root = backend.borrow_mut().create_element("div");
            let mut rs = StyleRules::default();
            rs.flex_direction = Some(FlexDirection::Column);
            rs.gap = Some(Tokenized::Literal(Length::Px(2.0)));
            backend.borrow_mut().apply_style(&root, &Rc::new(rs));
            let mut root = root;
            for item in items {
                let row = backend.borrow_mut().create_element("div");
                let mut row_rs = StyleRules::default();
                row_rs.flex_direction = Some(FlexDirection::Row);
                row_rs.margin_left =
                    Some(Tokenized::Literal(Length::Px(item.depth as f32 * LIST_INDENT)));
                backend.borrow_mut().apply_style(&row, &Rc::new(row_rs));
                let mut row = row;

                let marker = backend
                    .borrow_mut()
                    .create_text(&format!("{}  ", item.marker), a11y);
                let mut m_rs = StyleRules::default();
                m_rs.color = Some(Tokenized::Literal(Color(theme.muted.clone())));
                backend.borrow_mut().apply_style(&marker, &Rc::new(m_rs));
                backend.borrow_mut().insert(&mut row, marker);

                let content = backend.borrow_mut().create_element("div");
                append_runs(backend, &content, &item.runs, theme, &theme.text, a11y);
                backend.borrow_mut().insert(&mut row, content);

                backend.borrow_mut().insert(&mut root, row);
            }
            root
        }
        MdBlock::Rule => {
            let node = backend.borrow_mut().create_element("hr");
            let mut rs = StyleRules::default();
            rs.border_top_width = Some(Tokenized::Literal(1.0));
            rs.border_top_color = Some(Tokenized::Literal(Color(theme.muted.clone())));
            zero_margins(&mut rs);
            backend.borrow_mut().apply_style(&node, &Rc::new(rs));
            node
        }
    }
}

/// Append the inline runs of a block as styled text children.
fn append_runs<H>(
    backend: &Rc<RefCell<H>>,
    parent: &H::Node,
    runs: &[MdRun],
    theme: &MdTheme,
    base_color: &str,
    a11y: &AccessibilityProps,
) where
    H: StyleServices + TextOps,
{
    let mut parent = parent.clone();
    for run in runs {
        let node = backend.borrow_mut().create_text(&run.text, a11y);
        let mut rs = StyleRules::default();
        let color = if run.link.is_some() {
            &theme.link
        } else if run.code {
            &theme.code_fg
        } else {
            base_color
        };
        rs.color = Some(Tokenized::Literal(Color(color.to_string())));
        if run.bold {
            rs.font_weight = Some(FontWeight::Bold);
        }
        if run.italic {
            rs.font_style = Some(FontStyle::Italic);
        }
        if run.strike {
            rs.strikethrough = Some(true);
        }
        if run.link.is_some() {
            rs.underline = Some(true);
        }
        if run.code {
            rs.font_family = Some(mono_family(theme));
            rs.background = Some(Tokenized::Literal(Color(theme.code_bg.clone())));
            rs.padding_left = Some(Tokenized::Literal(Length::Px(4.0)));
            rs.padding_right = Some(Tokenized::Literal(Length::Px(4.0)));
        }
        backend.borrow_mut().apply_style(&node, &Rc::new(rs));
        backend.borrow_mut().insert(&mut parent, node);
    }
}

fn mono_family(theme: &MdTheme) -> FontFamily {
    FontFamily::System(
        theme
            .mono_family
            .clone()
            .unwrap_or_else(|| "ui-monospace, SFMono-Regular, Menlo, monospace".to_string()),
    )
}

fn zero_margins(rs: &mut StyleRules) {
    rs.margin_top = Some(Tokenized::Literal(Length::Px(0.0)));
    rs.margin_bottom = Some(Tokenized::Literal(Length::Px(0.0)));
}

fn pad_all(rs: &mut StyleRules, px: f32) {
    rs.padding_top = Some(Tokenized::Literal(Length::Px(px)));
    rs.padding_right = Some(Tokenized::Literal(Length::Px(px)));
    rs.padding_bottom = Some(Tokenized::Literal(Length::Px(px)));
    rs.padding_left = Some(Tokenized::Literal(Length::Px(px)));
}
