//! New-core (idea-lite migration) support for the website — only
//! compiled under the `new-core` feature.
//!
//! Three pieces:
//!
//! 1. **Web boot entry** — `wasm-pack build` of this crate under
//!    `--no-default-features --features new-core` produces a module
//!    whose start fn mounts `app()` into `#app` via
//!    `backend_web::newcore::start_in` (the conformance /
//!    idea-ui-docs newcore.html pattern). The CLI's `idealyst
//!    dev/build` flows keep booting the old core; `newcore.html` is
//!    the new-core demo vehicle.
//!
//! 2. **Codeblock scene handler** — the `codeblock` SDK is an
//!    old-core External (`Element::External` payload + per-backend
//!    registry) and cannot join a new-core graph. The scene
//!    `Registry` is the new core's unified primitive==external
//!    contract, so the website registers its own payload handler
//!    here, reproducing `codeblock::build_code_block`'s DOM
//!    byte-for-byte through the SAME capability calls the old
//!    handler made through the Backend trait: one
//!    `create_element("pre")` + per-run `create_text` +
//!    literal-color `apply_style` + `insert`, then the author style
//!    attached to the outer node. Generic over the caps traits so
//!    the SSR backend can reuse it for the static-render path.
//!
//! 3. **`styled_text` shim** — glue has no `styled_text(runs)` mirror
//!    yet; the vocabulary's `text()` builder carries the same
//!    `TextSourceProp::Runs` channel (`create_styled_text` at mount),
//!    so the shim is a thin wrapper giving `Prose`/`primitives.rs`
//!    the old `.with_style(…)` + `IntoElement` call shape.
//!
//! The old-core `TextRun`/`Color`/`StyleRules` types are the
//! vocabulary's own transitional prop types, reached through the
//! `old_runtime_core` alias in lib.rs (the facade shadows the
//! extern-prelude `runtime_core` name).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{Color, Element, StyleRules, Tokenized};
use runtime_scene::{item, MountCx, Registry};
use runtime_vocabulary::caps::TextOps;
use runtime_vocabulary::style_attach::{attach_style, IntoStyleProp, StyleProp, StyleServices};

// Re-exports so prose.rs / primitives.rs cfg-swap ONE import line:
// the same-named items resolve to the identical old-core types the
// vocabulary builders consume.
pub use old_runtime_core::styled_text::plain_text_of;
pub use old_runtime_core::{TextRun, TextRunStyle};

// =============================================================================
// styled_text — new-core shim over the vocabulary text builder.
// =============================================================================

/// Builder-shaped wrapper mirroring the old `styled_text(runs)` return
/// (`Bound<TextHandle>`): `.with_style(…)` then `IntoElement`.
pub struct StyledText(runtime_vocabulary::builders::TextBuilder);

/// One text node with inline-styled ranges — the new-core lowering is
/// the vocabulary `text()` builder's `runs` channel (mounted via
/// `create_styled_text`, exactly the old primitive's backend call).
pub fn styled_text(runs: Vec<TextRun>) -> StyledText {
    StyledText(runtime_vocabulary::builders::text().runs(runs))
}

impl StyledText {
    pub fn with_style(self, style: impl IntoStyleProp) -> Self {
        StyledText(self.0.style(style))
    }
}

impl runtime_core::IntoElement for StyledText {
    fn into_element(self) -> Element {
        self.0.build()
    }
}

// =============================================================================
// Codeblock — website-local scene-registry primitive.
// =============================================================================

/// Payload for the website's new-core codeblock. Single-take style slot
/// (the vocabulary `PrimCell` discipline, inlined): the scene hands the
/// handler a shared `&Rc<Self>`, but `StyleProp` must move at mount.
pub struct CodeBlockPrim {
    spans: Vec<(String, Color)>,
    style: RefCell<Option<StyleProp>>,
}

/// Author-side constructor — the new-core stand-in for
/// `codeblock::code_block(spans)`, same call shape at the one call
/// site (`pages/common.rs::CodeBlock`).
pub struct CodeBlockBuilder {
    spans: Vec<(String, Color)>,
    style: Option<StyleProp>,
}

pub fn code_block(spans: Vec<(String, Color)>) -> CodeBlockBuilder {
    CodeBlockBuilder { spans, style: None }
}

impl CodeBlockBuilder {
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

impl runtime_core::IntoElement for CodeBlockBuilder {
    fn into_element(self) -> Element {
        item(
            CodeBlockPrim {
                spans: self.spans,
                style: RefCell::new(self.style),
            },
            Vec::new(),
        )
    }
}

/// Mount handler — `codeblock::build_code_block` (web/SSR handler)
/// call-for-call: `<pre>` outer node, one text span per run with its
/// color applied as a literal rule, author style attached to the
/// `<pre>` through the standard style channel (`attach_style` is the
/// new-core home of the old walker's external style attach).
fn mount_code_block<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<CodeBlockPrim>,
    _children: Vec<Element>,
) -> H::Node
where
    H: StyleServices + TextOps,
{
    let backend = cx.backend().clone();
    let a11y = AccessibilityProps::default();
    let mut pre = backend.borrow_mut().create_element("pre");
    for (text, color) in &prim.spans {
        let span = backend.borrow_mut().create_text(text, &a11y);
        let mut rules = StyleRules::default();
        rules.color = Some(Tokenized::Literal(color.clone()));
        backend.borrow_mut().apply_style(&span, &Rc::new(rules));
        backend.borrow_mut().insert(&mut pre, span);
    }
    if let Some(style) = prim.style.borrow_mut().take() {
        attach_style(&backend, &pre, style);
    }
    pre
}

/// Register the website's payload handlers on a scene registry —
/// passed as the `register` seam of `backend_web::newcore::start_in`
/// (and reusable against any caps-complete host, e.g. SSR).
pub fn register_handlers<H>(registry: &mut Registry<H>)
where
    H: StyleServices + TextOps + 'static,
{
    registry.register::<CodeBlockPrim, _>(mount_code_block::<H>);
}

// =============================================================================
// Web boot entry.
// =============================================================================

#[cfg(target_arch = "wasm32")]
mod web_entry {
    use wasm_bindgen::prelude::*;

    /// New-core web boot: mounts into `#app` (newcore.html). The
    /// wasm-pack module's start fn — the CLI wrapper (old core) is not
    /// involved in this build.
    #[wasm_bindgen(start)]
    pub fn boot() {
        // Console logger so framework warnings reach devtools (the CLI
        // wrapper normally installs this).
        backend_web::install_logger();
        backend_web::newcore::start_in("#app", super::register_handlers, || crate::app());
    }
}
