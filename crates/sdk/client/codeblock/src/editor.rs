//! `code_editor` — an editable code panel with author-supplied
//! decorations.
//!
//! The editable sibling of [`code_block`](crate::code_block). Same
//! contract with the outside world — the caller says which byte ranges
//! look how, the primitive never parses anything — but the text is a
//! live, focusable, IME-capable editor rather than a static panel.
//!
//! ## Shape: an in-flow decorated layer with the real editor on top
//!
//! ```text
//! outer  view          ← the author's box styling (`.with_style`)
//!   stack view         ← position: relative (the absolute layer's containing block)
//!     pre  ────────────┐
//!       styled_text    │ IN FLOW. One attributed node carrying the
//!                      │ decorations. Its measured size IS the
//!                      │ editor's size.
//!     text_area  ──────┘ position: absolute, inset 0. Transparent
//!                        glyphs, visible caret. Stretches to whatever
//!                        the decorated layer measured.
//! ```
//!
//! Every backend runs this same handler — there is no per-platform
//! editor implementation, and that is deliberate. The hard part of
//! per-range styling (an attributed run list realized as ONE native
//! node, wrapping through the platform's own text engine) is already
//! solved by `styled_text`: `NSAttributedString` on Apple,
//! `SpannableString` on Android, nested `<span>`s on web/SSR,
//! cosmic-text rich spans on the GPU backend. Reimplementing an
//! editable `NSTextView`/`UITextView`/`EditText` per backend inside
//! this SDK would duplicate the controlled-value binding, key handling,
//! focus and IME plumbing that `text_area` already owns on all of them.
//! The scene `Registry` seam is still right here: a backend that later
//! grows a genuinely single-widget decorated editor can register a
//! concrete handler for this payload the way `code_block` does for
//! macOS/iOS/Android, without any author-visible change.
//!
//! ## Why the decorated layer is the one in flow
//!
//! It removes the two failure modes the hand-rolled version of this
//! (`examples/fiddle`, before it moved onto this primitive) had:
//!
//! - **No scroll desync.** Neither layer scrolls internally: the
//!   decorated layer measures to the full text, and the editor is
//!   stretched to that same box, so the content always fits. Scrolling
//!   happens on an ancestor and moves both layers as one. The
//!   alternative — a fixed-height box with an internally scrolling
//!   textarea — needs the highlight layer's scroll offset kept in sync
//!   with the editor's on every wheel tick, keystroke and caret move,
//!   and drifts the moment one of those paths is missed.
//! - **No metric drift.** Font family, size, line height and padding
//!   come from [`EditorMetrics`] and are written to BOTH layers by the
//!   handler. They are not author style, so there is no way to style
//!   one layer and forget the other — the failure that makes glyphs
//!   walk away from the caret one row at a time.
//!
//! ## No soft wrap
//!
//! The editor is always code-mode (`white-space: pre` on web, the same
//! no-wrap shape on native). Soft wrap would require the two layers to
//! choose *identical* break points, and the framework's style substrate
//! has no `white-space` property to put both of them in `pre-wrap` — the
//! decorated layer relies on the `<pre>` element for its whitespace
//! semantics on web. Long lines overflow horizontally; put the editor in
//! a scrolling ancestor and both layers scroll together, because the
//! decorated layer's own measured width drives the box.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{item, Element, MountCx};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::{
    Color, FontFamily, Length, Position, RunUnderline, StyleRules, TextRun, TextRunStyle, Tokenized,
};
use runtime_vocabulary::builders;
use runtime_vocabulary::caps::{DocumentOps, TextOps};
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{attach_style, IntoStyleProp, StyleProp, StyleServices};
use runtime_world::{effect, ReadSignal, Signal};

use crate::decoration::{flatten, Decoration, DecorationStyle};

/// Default monospace stack. The same CSS-ish stack the rest of the
/// framework uses for code, resolved per platform by each backend's
/// font-stack classifier (SF Mono on Apple, the platform monospace
/// alias on Android, the browser's `ui-monospace` on web).
pub const DEFAULT_FONT_FAMILY: &str = "ui-monospace, SFMono-Regular, Menlo, monospace";
/// Default glyph size, in px. 13 is the conventional editor body size
/// and matches what the framework's code panels already render at.
pub const DEFAULT_FONT_SIZE: f32 = 13.0;
/// Default line box, in px — ~1.54× the default font size, the ratio
/// code reads comfortably at. Emitted as a pixel value because the web
/// style layer lowers `line_height` to `px`, and because BOTH layers
/// must land on the same integral row height: a fractional line box
/// accumulates rounding drift between the two text engines row by row.
pub const DEFAULT_LINE_HEIGHT: f32 = 20.0;
/// Default inset, in px, applied identically to both layers.
pub const DEFAULT_PADDING: f32 = 12.0;
/// Default glyph color for undecorated text. Near-black rather than
/// pure black: pure black on a light panel is the one value that reads
/// as harsh at code density. Authors override via
/// [`CodeEditorBuilder::text_color`].
pub const DEFAULT_TEXT_COLOR: &str = "#24292f";
/// Tab width, in characters, for BOTH layers. Mirrors the `tab-size: 4`
/// the web backend bakes into every editable text node
/// (`backend-web/src/primitives/text_area.rs`) — the decorated layer has
/// to be told, because `<pre>` otherwise keeps the UA's 8 and a tabbed
/// file's highlight slides right of its glyphs.
const TAB_SIZE: &str = "4";

/// The metrics both layers share. Owned by the primitive, never by
/// author style, because the decorated layer and the editor layer have
/// to agree on all of them exactly — see the module docs.
#[derive(Clone, Debug)]
pub struct EditorMetrics {
    /// Font stack for both layers.
    pub font_family: String,
    /// Glyph size in px.
    pub font_size: f32,
    /// Line box in px.
    pub line_height: f32,
    /// Inset in px, applied to both layers.
    pub padding: f32,
    /// Glyph color for text no decoration covers.
    pub text_color: Color,
    /// Caret color. `None` follows [`Self::text_color`] — the editor's
    /// own glyphs are transparent, so a caret that inherited from them
    /// would be invisible.
    pub caret_color: Option<Color>,
}

impl Default for EditorMetrics {
    fn default() -> Self {
        Self {
            font_family: DEFAULT_FONT_FAMILY.to_string(),
            font_size: DEFAULT_FONT_SIZE,
            line_height: DEFAULT_LINE_HEIGHT,
            padding: DEFAULT_PADDING,
            text_color: Color(DEFAULT_TEXT_COLOR.to_string()),
            caret_color: None,
        }
    }
}

/// Where decorations come from. Both shapes exist because they fail
/// differently, not because one is a convenience wrapper for the other.
pub(crate) enum DecorationSource {
    /// No decorations — a plain monospace editor.
    None,
    /// Pull: the primitive calls this with the text it is about to
    /// display, so the ranges can never describe a different buffer
    /// than the one on screen. The right shape for a synchronous
    /// tokenizer.
    Pull(Rc<dyn Fn(&str) -> Vec<Decoration>>),
    /// Push: decorations arrive out of band (an async diagnostics pass,
    /// a worker-side parser) and may describe an older buffer. Ranges
    /// are clamped against the current text rather than trusted — see
    /// [`crate::decoration::flatten`].
    Push(ReadSignal<Vec<Decoration>>),
}

impl DecorationSource {
    fn compute(&self, text: &str) -> Vec<Decoration> {
        match self {
            DecorationSource::None => Vec::new(),
            DecorationSource::Pull(f) => f(text),
            DecorationSource::Push(s) => s.get(),
        }
    }
}

/// Payload for a code editor. `style` and `editor` are single-take
/// slots (the `PrimCell` discipline): the scene hands the handler a
/// shared `&Rc<Self>`, but a `StyleProp` and an `Element` must MOVE at
/// mount.
pub(crate) struct CodeEditorPrim {
    pub(crate) value: Signal<String>,
    pub(crate) decorations: DecorationSource,
    pub(crate) metrics: EditorMetrics,
    pub(crate) style: RefCell<Option<StyleProp>>,
    pub(crate) editor: RefCell<Option<Element>>,
}

/// Author-side builder returned by [`code_editor`].
pub struct CodeEditorBuilder {
    value: Signal<String>,
    on_change: Rc<dyn Fn(String)>,
    on_key_down: Option<runtime_shared::primitives::key::KeyDownHandler>,
    decorations: DecorationSource,
    metrics: EditorMetrics,
    style: Option<StyleProp>,
    placeholder: Option<String>,
    test_id: Option<&'static str>,
    ref_fill: Option<Box<dyn FnOnce(runtime_shared::primitives::text_area::TextAreaHandle)>>,
}

/// Construct a code editor over a `Signal<String>`.
///
/// `value` is the source of truth (the controlled pattern every editable
/// primitive in the framework uses) and `on_change` reports each edit
/// back — the caller decides whether to accept it, exactly as with
/// `text_area`. The signal is read by BOTH layers, which is why it is a
/// `Signal` rather than an `impl IntoValue<String>`: the decorated layer
/// has to re-read the same text the editor is holding.
///
/// ```ignore
/// use codeblock::{code_editor, Decoration};
///
/// // At app bootstrap — `register` is the boot registration seam.
/// backend_web::newcore::start_in("#app", codeblock::register, app);
///
/// let src = signal(String::from("fn main() {}"));
/// code_editor(src, move |next| src.set(next))
///     .decorate(|text| my_tokenizer(text))   // -> Vec<Decoration>
///     .with_style(editor_panel_style())
/// ```
pub fn code_editor(value: Signal<String>, on_change: impl Fn(String) + 'static) -> CodeEditorBuilder {
    CodeEditorBuilder {
        value,
        on_change: Rc::new(on_change),
        on_key_down: None,
        decorations: DecorationSource::None,
        metrics: EditorMetrics::default(),
        style: None,
        placeholder: None,
        test_id: None,
        ref_fill: None,
    }
}

impl CodeEditorBuilder {
    /// Decorate by function: called with the text the editor is about
    /// to display, and re-called on every change. Ranges are therefore
    /// always describing the buffer on screen.
    ///
    /// This is the shape a synchronous tokenizer wants. The function
    /// runs on every keystroke, so it should be cheap — tokenizing tens
    /// of kilobytes is sub-millisecond, but a full parse is not, and
    /// belongs behind [`decorations`](Self::decorations) instead.
    pub fn decorate(mut self, f: impl Fn(&str) -> Vec<Decoration> + 'static) -> Self {
        self.decorations = DecorationSource::Pull(Rc::new(f));
        self
    }

    /// Decorate by signal: for producers that can't answer
    /// synchronously (a language server, a worker-side parser, a
    /// compile that just finished). The primitive re-reads the signal
    /// whenever it changes, and clamps stale ranges against the current
    /// text rather than panicking on them.
    ///
    /// Composes with [`decorate`](Self::decorate) only in the sense that
    /// the later call wins — to layer syntax colors under async
    /// diagnostics, concatenate both lists in one source (decorations
    /// layer in list order; see [`crate::decoration`]).
    pub fn decorations(mut self, source: impl Into<ReadSignal<Vec<Decoration>>>) -> Self {
        self.decorations = DecorationSource::Push(source.into());
        self
    }

    /// Replace the whole metric set at once.
    pub fn metrics(mut self, metrics: EditorMetrics) -> Self {
        self.metrics = metrics;
        self
    }

    /// Font stack and size for both layers.
    pub fn font(mut self, family: impl Into<String>, size_px: f32) -> Self {
        self.metrics.font_family = family.into();
        self.metrics.font_size = size_px;
        self
    }

    /// Line box in px. Keep it integral — see [`DEFAULT_LINE_HEIGHT`].
    pub fn line_height(mut self, px: f32) -> Self {
        self.metrics.line_height = px;
        self
    }

    /// Inset in px, applied to both layers.
    pub fn padding(mut self, px: f32) -> Self {
        self.metrics.padding = px;
        self
    }

    /// Glyph color for text no decoration covers.
    pub fn text_color(mut self, color: impl Into<String>) -> Self {
        self.metrics.text_color = Color(color.into());
        self
    }

    /// Caret color; defaults to the text color.
    pub fn caret_color(mut self, color: impl Into<String>) -> Self {
        self.metrics.caret_color = Some(Color(color.into()));
        self
    }

    /// Author box styling — lands on the OUTER node only. Fonts and
    /// padding are deliberately not part of this; they are metrics.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }

    /// Key handler on the editing layer. The canonical use is Tab:
    /// return `KeyOutcome::PreventDefault` and call `insert_text("    ")`
    /// on the bound handle.
    pub fn on_key_down(
        mut self,
        f: impl Fn(&runtime_shared::primitives::key::KeyEvent)
                -> runtime_shared::primitives::key::KeyOutcome
            + 'static,
    ) -> Self {
        self.on_key_down = Some(Rc::new(f));
        self
    }

    /// Placeholder shown while the buffer is empty.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    /// Robot/automation anchor, forwarded to the editing layer (the
    /// node a driver types into).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.test_id = Some(id);
        self
    }

    /// Bind the editing layer's imperative handle into a `Ref` —
    /// `focus()`, `select_all()`, `insert_text()`. The mirror of
    /// `text_area(..).bind(..)`, and the way a Tab handler reaches the
    /// editor to substitute spaces for the suppressed default.
    pub fn bind(
        self,
        r: runtime_shared::reactive::Ref<runtime_shared::primitives::text_area::TextAreaHandle>,
    ) -> Self {
        self.on_handle(move |h| r.fill(h))
    }

    /// Bind the editing layer's imperative handle — `focus()`,
    /// `select_all()`, `insert_text()`.
    pub fn on_handle(
        mut self,
        fill: impl FnOnce(runtime_shared::primitives::text_area::TextAreaHandle) + 'static,
    ) -> Self {
        self.ref_fill = Some(Box::new(fill));
        self
    }
}

/// Mirrors `CodeBlockBuilder`: `.into()` reaches `Element` directly, so
/// the builder drops into a `ui!` child slot without the call site
/// importing `IntoElement`.
impl From<CodeEditorBuilder> for Element {
    fn from(b: CodeEditorBuilder) -> Element {
        b.into_element()
    }
}

impl IntoElement for CodeEditorBuilder {
    fn into_element(self) -> Element {
        // The editing layer is an ordinary `text_area` element: it keeps
        // the controlled-value binding, key handling, focus/blur, IME and
        // robot registration that the vocabulary already implements on
        // every backend. The handler only positions it and locks its
        // metrics.
        let mut editor = builders::text_area()
            .value(self.value)
            .on_change({
                let f = self.on_change.clone();
                move |v| f(v)
            })
            // No soft wrap: the two layers must break lines identically,
            // and only `pre` guarantees that (module docs).
            .wrap(false)
            .style(StyleProp::Static(Rc::new(editor_layer_style(&self.metrics))));
        if let Some(k) = self.on_key_down {
            editor = editor.on_key_down(move |ev| k(ev));
        }
        if let Some(p) = self.placeholder {
            editor = editor.placeholder(p);
        }
        if let Some(id) = self.test_id {
            editor = editor.test_id(id);
        }
        if let Some(fill) = self.ref_fill {
            editor = editor.on_handle(fill);
        }

        item(
            CodeEditorPrim {
                value: self.value,
                decorations: self.decorations,
                metrics: self.metrics,
                style: RefCell::new(self.style),
                editor: RefCell::new(Some(editor.build())),
            },
            Vec::new(),
        )
    }
}

/// Style for the stack: the containing block the editing layer's
/// `position: absolute` resolves against, stretched across the author's
/// box.
fn stack_style() -> StyleRules {
    StyleRules {
        position: Some(Position::Relative),
        align_self: Some(runtime_shared::AlignSelf::Stretch),
        ..Default::default()
    }
}

/// Style for the decorated layer's `<pre>` wrapper. This node is IN
/// FLOW — its measured size is the editor's size.
fn decorated_layer_style(m: &EditorMetrics) -> StyleRules {
    let pad = Some(Tokenized::Literal(Length::Px(m.padding)));
    let zero = Some(Tokenized::Literal(Length::Px(0.0)));
    StyleRules {
        // `<pre>` carries a UA margin on web; zero it so the two layers
        // start at the same origin.
        margin_top: zero.clone(),
        margin_right: zero.clone(),
        margin_bottom: zero.clone(),
        margin_left: zero,
        padding_top: pad.clone(),
        padding_right: pad.clone(),
        padding_bottom: pad.clone(),
        padding_left: pad,
        font_family: Some(FontFamily::System(m.font_family.clone())),
        font_size: Some(Tokenized::Literal(Length::Px(m.font_size))),
        line_height: Some(Tokenized::Literal(m.line_height)),
        color: Some(Tokenized::Literal(m.text_color.clone())),
        ..Default::default()
    }
}

/// Style for the editing layer: same metrics, stretched over the
/// decorated layer, with invisible glyphs and a visible caret.
fn editor_layer_style(m: &EditorMetrics) -> StyleRules {
    let pad = Some(Tokenized::Literal(Length::Px(m.padding)));
    let zero = Some(Tokenized::Literal(Length::Px(0.0)));
    StyleRules {
        position: Some(Position::Absolute),
        top: zero.clone(),
        right: zero.clone(),
        bottom: zero.clone(),
        left: zero,
        padding_top: pad.clone(),
        padding_right: pad.clone(),
        padding_bottom: pad.clone(),
        padding_left: pad,
        font_family: Some(FontFamily::System(m.font_family.clone())),
        font_size: Some(Tokenized::Literal(Length::Px(m.font_size))),
        line_height: Some(Tokenized::Literal(m.line_height)),
        // The editor's own glyphs are invisible — the decorated layer
        // carries every visible pixel of text. The caret would vanish
        // with them (it follows `color` by default), so it is pinned
        // explicitly.
        color: Some(Tokenized::Literal(Color("transparent".into()))),
        caret_color: Some(Tokenized::Literal(
            m.caret_color.clone().unwrap_or_else(|| m.text_color.clone()),
        )),
        background: Some(Tokenized::Literal(Color("transparent".into()))),
        ..Default::default()
    }
}

/// Build the attributed run list for `text` under `decorations`.
///
/// Undecorated stretches become plain runs (they inherit the decorated
/// layer's own paragraph style), so the common case of a mostly-plain
/// buffer costs one run, not one run per character.
pub(crate) fn runs_for(text: &str, decorations: &[Decoration]) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = flatten(text, decorations)
        .into_iter()
        .map(|run| {
            let slice = run.slice(text).to_string();
            if run.style.is_empty() {
                TextRun::plain(slice)
            } else {
                TextRun::styled(slice, run_style(&run.style))
            }
        })
        .collect();
    // A buffer ending in a newline has a real, empty last row that the
    // caret can sit on. The editing layer renders it (a `<textarea>`'s
    // value is not HTML); a text node does not — web collapses a
    // trailing newline, and the platform labels have nothing to lay out
    // on that row either. Without this the decorated layer measures one
    // row short and the editor's last line is clipped.
    if text.is_empty() || text.ends_with('\n') {
        runs.push(TextRun::plain(" "));
    }
    runs
}

/// Map a decoration delta onto a text-run delta. Colors become
/// `Tokenized::Literal` — a decoration producer emits concrete colors
/// (that is what a tokenizer has), while the theme-aware layers of the
/// framework work in tokens.
fn run_style(style: &DecorationStyle) -> TextRunStyle {
    TextRunStyle {
        font_family: None,
        font_weight: style.font_weight,
        font_style: style.font_style,
        // Never per-run: a code editor's rows must stay on one baseline
        // grid or the caret drifts off the glyphs (see DecorationStyle).
        font_size: None,
        color: style.color.clone().map(Tokenized::Literal),
        background: style.background.clone().map(Tokenized::Literal),
        underline: style.underline.as_ref().map(|u| RunUnderline {
            style: u.style,
            color: u.color.clone().map(Tokenized::Literal),
        }),
    }
}

/// Mount handler — one implementation, every caps-complete host.
pub(crate) fn mount_code_editor<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<CodeEditorPrim>,
    _children: Vec<Element>,
) -> H::Node
where
    // `StyleServices` already implies `DocumentOps` (→ `ViewOps`), which
    // is where `create_element` / `create_view` / `insert` come from.
    H: StyleServices + TextOps,
{
    let backend = cx.backend().clone();
    let a11y = AccessibilityProps::default();

    let mut outer = backend.borrow_mut().create_view(&a11y);
    let mut stack = backend.borrow_mut().create_view(&a11y);
    backend
        .borrow_mut()
        .apply_style(&stack, &Rc::new(stack_style()));

    // `<pre>`, not a plain view: it is what gives the decorated layer
    // `white-space: pre` on web, so runs of spaces and newlines survive
    // instead of collapsing. Hosts with no tag concept fall back to a
    // plain view (the `create_element` cap default) and already
    // preserve whitespace in their text engines.
    let mut pre = backend.borrow_mut().create_element("pre");
    backend
        .borrow_mut()
        .apply_style(&pre, &Rc::new(decorated_layer_style(&prim.metrics)));
    // Tab stops must be the SAME width in both layers or a file with
    // literal tabs shows its highlight sliding right of the glyphs, one
    // stop per tab. On web the two elements disagree by default: the
    // framework's editable text declares `tab-size: 4`, while `<pre>`
    // keeps the UA's 8. This is a no-op on every host without a CSS
    // notion of tab stops (the cap's default), which is correct — there
    // the two layers share the platform's own tab metrics already.
    backend
        .borrow()
        .attach_html_style(&pre, "tab-size", TAB_SIZE);

    let initial_text = prim.value.get();
    let initial_runs = runs_for(&initial_text, &prim.decorations.compute(&initial_text));
    let decorated = backend
        .borrow_mut()
        .create_styled_text(&initial_runs, &a11y);
    backend.borrow_mut().insert(&mut pre, decorated.clone());
    backend.borrow_mut().insert(&mut stack, pre);

    // Re-decorate on every change. Reading `value` inside the effect
    // tracks it; for a `Push` source, `compute` reads that signal too,
    // so late-arriving diagnostics re-fire the same effect. One node
    // update per change — `update_styled_text` replaces the attributed
    // string in place rather than rebuilding a node per token, which is
    // what keeps a per-keystroke re-tokenize affordable.
    {
        let b = backend.clone();
        let node = decorated.clone();
        let prim = prim.clone();
        let _ = effect(move || {
            let text = prim.value.get();
            let runs = runs_for(&text, &prim.decorations.compute(&text));
            b.borrow_mut().update_styled_text(&node, &runs);
        });
    }

    // The editing layer, realized through the normal element path so it
    // keeps every `text_area` behaviour. Inserted AFTER the decorated
    // layer so it sits on top and takes the pointer/keyboard.
    if let Some(editor) = prim.editor.borrow_mut().take() {
        cx.realize_children_into(&mut stack, vec![editor]);
    }

    backend.borrow_mut().insert(&mut outer, stack);
    if let Some(style) = prim.style.borrow_mut().take() {
        attach_style(&backend, &outer, style);
    }
    outer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoration::Underline;
    use runtime_shared::FontWeight;

    #[test]
    fn plain_text_is_a_single_plain_run() {
        let runs = runs_for("fn main() {}", &[]);
        assert_eq!(runs.len(), 1);
        assert!(runs[0].style.is_none());
        assert_eq!(runs[0].text, "fn main() {}");
    }

    #[test]
    fn decorated_ranges_become_styled_runs_with_literal_colors() {
        let runs = runs_for(
            "let x",
            &[Decoration::new(
                0..3,
                DecorationStyle::default().with_color("#f00").with_weight(FontWeight::Bold),
            )],
        );
        assert_eq!(runs.len(), 2);
        let style = runs[0].style.as_ref().expect("decorated run carries a style");
        assert_eq!(style.color, Some(Tokenized::Literal(Color("#f00".into()))));
        assert_eq!(style.font_weight, Some(FontWeight::Bold));
        assert!(runs[1].style.is_none(), "the gap inherits the paragraph style");
    }

    #[test]
    fn underline_decorations_carry_their_own_color_into_the_run() {
        let runs = runs_for(
            "err",
            &[Decoration::underline(0..3, Underline::dotted().colored("#c00"))],
        );
        let u = runs[0]
            .style
            .as_ref()
            .and_then(|s| s.underline.as_ref())
            .expect("underline survives the mapping");
        assert_eq!(u.style, runtime_shared::UnderlineStyle::Dotted);
        assert_eq!(u.color, Some(Tokenized::Literal(Color("#c00".into()))));
    }

    /// A decoration must never set a per-run font size: two rows at
    /// different sizes break the baseline grid the caret is positioned
    /// against, so the caret and the glyphs drift apart down the file.
    #[test]
    fn run_style_never_sets_a_font_size() {
        let style = run_style(&DecorationStyle::default().with_color("#f00"));
        assert!(style.font_size.is_none());
        assert!(style.font_family.is_none());
    }

    /// The buffer's trailing empty row is real — the caret can sit on
    /// it — but a text node does not render a trailing newline, so the
    /// decorated layer would measure one row short and clip the
    /// editor's last line.
    #[test]
    fn regression_trailing_newline_keeps_the_last_row_measurable() {
        let runs = runs_for("a\n", &[]);
        let plain = runtime_shared::styled_text::plain_text_of(&runs);
        assert_eq!(plain, "a\n ", "a trailing row must carry something to lay out");
    }

    #[test]
    fn regression_empty_buffer_still_measures_one_row() {
        let runs = runs_for("", &[]);
        assert_eq!(runtime_shared::styled_text::plain_text_of(&runs), " ");
    }

    /// Both layers must receive the same font, size, line box and
    /// padding — the metric drift that walks glyphs away from the caret
    /// is exactly a mismatch between these two style builders.
    #[test]
    fn both_layers_share_identical_metrics() {
        let m = EditorMetrics::default();
        let decorated = decorated_layer_style(&m);
        let editor = editor_layer_style(&m);
        assert_eq!(decorated.font_family, editor.font_family);
        assert_eq!(decorated.font_size, editor.font_size);
        assert_eq!(decorated.line_height, editor.line_height);
        assert_eq!(decorated.padding_top, editor.padding_top);
        assert_eq!(decorated.padding_left, editor.padding_left);
    }

    /// The editing layer's glyphs are transparent, so its caret cannot
    /// inherit its color — it would be invisible.
    #[test]
    fn editor_layer_hides_glyphs_but_keeps_a_visible_caret() {
        let m = EditorMetrics::default();
        let editor = editor_layer_style(&m);
        assert_eq!(
            editor.color,
            Some(Tokenized::Literal(Color("transparent".into())))
        );
        assert_eq!(
            editor.caret_color,
            Some(Tokenized::Literal(Color(DEFAULT_TEXT_COLOR.into()))),
            "caret defaults to the text color, never to the transparent glyph color"
        );
    }

    #[test]
    fn explicit_caret_color_overrides_the_text_color_default() {
        let m = EditorMetrics { caret_color: Some(Color("#0af".into())), ..Default::default() };
        assert_eq!(
            editor_layer_style(&m).caret_color,
            Some(Tokenized::Literal(Color("#0af".into())))
        );
    }
}
