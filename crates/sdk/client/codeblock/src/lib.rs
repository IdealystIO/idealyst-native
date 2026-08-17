//! `codeblock` — read-only colored-text panel primitive.
//!
//! A flat sequence of `(text, color)` runs rendered as a **single
//! native node** on every platform that has a real handler. Built for
//! syntax-highlighted source display — the docs site renders ~140 line
//! tokenized snippets and ships dozens of them per page.
//!
//! ## Why this is a third-party primitive, not a framework one
//!
//! It used to be `Element::CodeBlock` in the framework core. A
//! measurement showed the perf justification was real: the equivalent
//! composition (`view` + per-token styled `text`) generates 100–300×
//! more backend ops per re-render even with batched fast paths. The
//! structural gap (composition rebuilds every span on each render; the
//! single-node primitive replaces one node) can't be closed by
//! framework optimization alone.
//!
//! But the primitive doesn't fit the core's intent — it isn't a
//! platform-native widget and is expressible from existing primitives
//! if perf weren't a concern. CLAUDE.md rule 3 says exactly this case
//! belongs in a third-party extension. So we kept the fast single-node
//! renderer but moved the type out of core.
//!
//! ## Per-platform rendering
//!
//! [`register`] installs the payload handler on a scene
//! [`Registry`](runtime_scene::Registry). It type-dispatches ONCE at
//! registration (the toolbar SDK's pattern) so the concrete native
//! backends get their single-node handler and every other
//! caps-complete host gets the portable `<pre>`/span one:
//!
//! - **macOS** — an `NSScrollView` (horizontal) wrapping an
//!   `NSTextField` label with per-run `NSColor` ranges. One label per
//!   block.
//! - **iOS** — a `UIScrollView` (horizontal) containing an
//!   inset-honoring label whose `attributedText` is an
//!   `NSAttributedString` with per-run
//!   `NSForegroundColorAttributeName` ranges. One label per block.
//! - **Android** — a `RustCodeBlock` (HorizontalScrollView + TextView)
//!   that sets a `SpannableString` with one `ForegroundColorSpan` per
//!   run. One TextView per code block, regardless of token count.
//! - **Linux (GTK4)** — one `gtk::Label` carrying the concatenated runs
//!   with a `pango::AttrList` holding one foreground-colour range each.
//!   Deliberately NOT selectable: `gtk_label_set_selectable()` also makes
//!   the widget focusable, and GTK focuses the first focusable widget in a
//!   freshly mapped subtree, so a selectable code panel captured the caret
//!   on every navigation (see `linux.rs`).
//! - **Windows** — one painted colored-runs leaf via the Windows backend's
//!   `create_colored_code_leaf`, wrapped in a `create_view` so author box
//!   styling lands like everywhere else.
//! - **Every other host (web, SSR, terminal, gpu, host-mock)** — one
//!   `create_element("pre")` outer node with one `create_text` child
//!   per run, each carrying its literal color. Backends with no tag
//!   concept get a plain view (the `create_element` cap default), so
//!   the panel still renders; only the one-node-per-block optimization
//!   is absent.
//!
//! ## Padding is author-driven
//!
//! Handlers apply NO padding of their own. The author's `padding_*` (via
//! `.with_style` on the `code_block`) lands on the outer node; each
//! backend realizes it INSIDE the scroll region (CSS padding on the web
//! `<pre>`, label `textInsets` on iOS, documentView offset on macOS,
//! `setPadding` + `clipToPadding=false` on Android), so the padding
//! scrolls WITH the content — text sits inset at rest and reaches the
//! block's own edge mid-scroll, the `<pre> { padding }` observable model.
//! Handlers previously hard-coded a 20pt/dp inset; combined with author
//! padding on a wrapping panel that doubled the visible padding and
//! clipped scrolled content before the block's edge (and made overlay
//! use-cases like the fiddle editor impossible to align).
//!
//! ## Usage
//!
//! ```ignore
//! use codeblock::code_block;
//!
//! // At app bootstrap: `register` IS the boot registration seam.
//! backend_web::newcore::start_in("#app", codeblock::register, app);
//!
//! // Inside an effect / arm body:
//! let spans: Vec<(String, Color)> = tokenize(source);
//! code_block(spans).with_style(my_codeblock_style())
//! ```
//!
//! An UNREGISTERED payload panics at realize (the scene contract), so a
//! missed `register` fails loud rather than rendering a silent
//! placeholder.
#![deny(missing_docs)]

use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{item, Element, Host, MountCx, Registry};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::{Color, StyleRules, Tokenized};
use runtime_vocabulary::caps::TextOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{attach_style, IntoStyleProp, StyleProp, StyleServices};

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod macos;
#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
mod linux;

/// Payload for a code block. Single-take style slot (the vocabulary
/// `PrimCell` discipline, inlined): the scene hands the handler a
/// shared `&Rc<Self>`, but `StyleProp` must move at mount.
struct CodeBlockPrim {
    spans: Vec<(String, Color)>,
    style: RefCell<Option<StyleProp>>,
}

/// Author-side builder returned by [`code_block`] — `.with_style(…)`
/// then element coercion.
pub struct CodeBlockBuilder {
    spans: Vec<(String, Color)>,
    style: Option<StyleProp>,
}

/// Construct a `CodeBlock` from a flat span list.
///
/// ```ignore
/// code_block(vec![
///     ("fn ".into(),      Color("#888".into())),
///     ("hello".into(),    Color("#0a0".into())),
///     ("() { … }".into(), Color("#444".into())),
/// ])
/// ```
pub fn code_block(spans: Vec<(String, Color)>) -> CodeBlockBuilder {
    CodeBlockBuilder { spans, style: None }
}

impl CodeBlockBuilder {
    /// Attach the author style — lands on the outer node (the `<pre>` /
    /// scroller).
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

/// Shared mount tail: the author style on the outer node, through the
/// standard style channel.
fn attach_author_style<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &CodeBlockPrim)
where
    H: StyleServices,
{
    if let Some(style) = prim.style.borrow_mut().take() {
        attach_style(backend, node, style);
    }
}

/// Portable mount handler: `<pre>` outer node, one text span per run
/// with its color applied as a literal rule, author style attached to
/// the `<pre>`. Generic over the caps traits so every caps-complete
/// host shares it — the web backend (`backend_web::newcore::start_in`'s
/// register seam) and the SSR backend
/// (`backend_ssr::newcore::render_path_with`) mount the identical
/// `<pre>`/span shape, which is what keeps the website's SSG parity
/// dumps aligned.
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
    attach_author_style(&backend, &pre, prim);
    pre
}

/// macOS single-node handler — `Registry<MacosBackend>`-concrete (the
/// AppKit widget graph has no caps-trait expression).
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
fn mount_code_block_macos(
    cx: &mut MountCx<'_, backend_macos::MacosBackend>,
    prim: &Rc<CodeBlockPrim>,
    _children: Vec<Element>,
) -> backend_macos::MacosNode {
    let backend = cx.backend().clone();
    let node = macos::build(&prim.spans, &mut backend.borrow_mut());
    attach_author_style(&backend, &node, prim);
    node
}

/// Linux (GTK4) single-node handler — `Registry<LinuxBackend>`-concrete.
/// One `gtk::Label` carrying the concatenated spans plus a per-run colour
/// `pango::AttrList` — the GTK analogue of the macOS NSAttributedString leaf.
#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
fn mount_code_block_linux(
    cx: &mut MountCx<'_, backend_linux::LinuxBackend>,
    prim: &Rc<CodeBlockPrim>,
    _children: Vec<Element>,
) -> backend_linux::LinuxNode {
    let backend = cx.backend().clone();
    let node = linux::build(&prim.spans, &mut backend.borrow_mut());
    attach_author_style(&backend, &node, prim);
    node
}

/// iOS single-node handler — `Registry<IosBackend>`-concrete.
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
fn mount_code_block_ios(
    cx: &mut MountCx<'_, backend_ios::IosBackend>,
    prim: &Rc<CodeBlockPrim>,
    _children: Vec<Element>,
) -> backend_ios::IosNode {
    let backend = cx.backend().clone();
    let node = ios::build(&prim.spans, &mut backend.borrow_mut());
    attach_author_style(&backend, &node, prim);
    node
}

/// Android single-node handler — `Registry<AndroidBackend>`-concrete.
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
fn mount_code_block_android(
    cx: &mut MountCx<'_, backend_android::AndroidBackend>,
    prim: &Rc<CodeBlockPrim>,
    _children: Vec<Element>,
) -> jni::objects::GlobalRef {
    let backend = cx.backend().clone();
    let node = android::build(&prim.spans, &mut backend.borrow_mut());
    attach_author_style(&backend, &node, prim);
    node
}

/// Register the codeblock payload handler on a scene registry. Pass
/// this as the boot registration seam — the `register` argument of
/// `backend_web::newcore::start_in` /
/// `backend_ssr::newcore::render_path_with` / a native host's
/// `run_with` — against any caps-complete host.
///
/// The platform dispatch happens ONCE here, by registry type: on macOS
/// / iOS / Android the concrete native registry gets the single-node
/// handler, and anything else (including SSR + SSG builds that run on
/// those same host OSes) gets the portable `<pre>`/span handler. A cfg
/// split alone could not express that — `target_os = "macos"` is true
/// for a macOS SSG build too.
pub fn register<H>(registry: &mut Registry<H>)
where
    H: StyleServices + TextOps + 'static,
{
    #[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn std::any::Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_macos::MacosBackend>>() {
            reg.register::<CodeBlockPrim, _>(mount_code_block_macos);
            return;
        }
    }
    #[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn std::any::Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_linux::LinuxBackend>>() {
            reg.register::<CodeBlockPrim, _>(mount_code_block_linux);
            return;
        }
    }

    #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn std::any::Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_ios::IosBackend>>() {
            reg.register::<CodeBlockPrim, _>(mount_code_block_ios);
            return;
        }
    }
    #[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn std::any::Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_android::AndroidBackend>>() {
            reg.register::<CodeBlockPrim, _>(mount_code_block_android);
            return;
        }
    }
    registry.register::<CodeBlockPrim, _>(mount_code_block::<H>);
}

/// Declare this SDK's payload kind **late-bound** instead of installing
/// its handler — the boot half of lazy registration. Pair with
/// [`register_from_chunk`] from inside a `#[component(lazy)]` body.
///
/// Only web code-splits, so on every other target this installs the
/// handler eagerly exactly as [`register`] does. That is deliberate:
/// deferring a kind nothing later registers leaves the payload parked
/// behind a placeholder forever, with no panic and no log, and native
/// has no chunk to arrive. Calling `defer` is therefore always safe —
/// it splits where splitting exists and is a plain `register` elsewhere.
pub fn defer<H>(registry: &mut Registry<H>)
where
    H: Host + StyleServices + TextOps + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        registry.defer::<CodeBlockPrim>();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        register(registry);
    }
}

/// Install this SDK's payload handler from inside a lazy chunk — the
/// chunk half of lazy registration. Requires [`defer`] at boot.
///
/// Generic over the host because this crate takes no backend dependency;
/// the caller pins `H` to its concrete backend, e.g.
/// `register_from_chunk::<backend_web::WebBackend>()`. A no-op on
/// non-web targets, where [`defer`] already registered eagerly.
pub fn register_from_chunk<H>()
where
    H: Host + StyleServices + TextOps + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        runtime_scene::defer_registration::<H, _>(|registry| {
            registry.register_deferred::<CodeBlockPrim, _>(mount_code_block::<H>);
        });
    }
}
