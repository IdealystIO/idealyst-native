//! New-core surface — the scene-registry leg (idea-lite migration).
//!
//! The old-core leg is an `Element::External` payload dispatched through
//! the per-backend `ExternalRegistry`; that registry does not exist on
//! the new core. The scene [`Registry`] is the new core's unified
//! primitive==external contract, so this leg registers a payload handler
//! there instead, reproducing the old web/SSR handler's DOM
//! **byte-for-byte** through the SAME capability calls the old handler
//! made through the `Backend` trait: one `create_element("pre")` +
//! per-run `create_text` + literal-color `apply_style` + `insert`, then
//! the author style attached to the outer node.
//!
//! The handler is generic over the caps traits
//! ([`StyleServices`] + [`TextOps`]) so every caps-complete host shares
//! it — the web backend (`backend_web::newcore::start_in`'s register
//! seam) and the SSR backend (`backend_ssr::newcore::render_path_with`)
//! both mount the identical `<pre>`/span shape, which is what keeps the
//! website's old/new SSG parity dumps aligned (this handler grew out of
//! the website-local shim that was verified pixel-identical against the
//! old core; see websites/website/tests/ssg_parity.rs).
//!
//! Same authored call shape as the old-core leg:
//! `code_block(spans).with_style(…)` then `IntoElement` — a consuming
//! app compiles the same source against either core. App bootstrap
//! passes [`register`] as the boot registration seam (there is no
//! inventory self-registration on the new core; the registry is built
//! explicitly at boot).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{Color, StyleRules, Tokenized};
use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::TextOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{attach_style, IntoStyleProp, StyleProp, StyleServices};

/// Payload for the new-core codeblock. Single-take style slot (the
/// vocabulary `PrimCell` discipline, inlined): the scene hands the
/// handler a shared `&Rc<Self>`, but `StyleProp` must move at mount.
struct CodeBlockPrim {
    spans: Vec<(String, Color)>,
    style: RefCell<Option<StyleProp>>,
}

/// Author-side builder returned by [`code_block`] — mirrors the
/// old-core `Bound<CodeBlockHandle>` call shape (`.with_style(…)` then
/// `IntoElement`).
pub struct CodeBlockBuilder {
    spans: Vec<(String, Color)>,
    style: Option<StyleProp>,
}

/// Construct a `CodeBlock` from a flat span list — the new-core
/// stand-in for the old-core `code_block(spans)`, same call shape at
/// every author site.
pub fn code_block(spans: Vec<(String, Color)>) -> CodeBlockBuilder {
    CodeBlockBuilder { spans, style: None }
}

impl CodeBlockBuilder {
    /// Attach the author style — lands on the outer node (the `<pre>`),
    /// exactly like `.with_style` on the old-core `Bound`.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

impl IntoElement for CodeBlockBuilder {
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

/// Mount handler — the old-core `build_code_block` (web/SSR handler)
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

/// Register the codeblock payload handler on a scene registry. Pass
/// this as the boot registration seam — the `register` argument of
/// `backend_web::newcore::start_in` / `backend_ssr::newcore::
/// render_path_with` — against any caps-complete host.
pub fn register<H>(registry: &mut Registry<H>)
where
    H: StyleServices + TextOps + 'static,
{
    registry.register::<CodeBlockPrim, _>(mount_code_block::<H>);
}
