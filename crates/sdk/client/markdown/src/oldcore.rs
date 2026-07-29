//! Old-core surface — `Element::External` payload + the per-backend
//! `ExternalRegistry`. Byte-moved from the crate root when the
//! `new-core` leg landed (see lib.rs); the default build re-exports
//! everything here unchanged.

use std::any::Any;
use std::rc::Rc;

use runtime_core::{component, Bound, Element, ExternalHandle, IdealystSchema, Reactive};

use crate::ir::{MarkdownDoc, MdTheme};
use crate::parse;

/// Convenience handle alias — the typed `Ref` target for a markdown
/// external node. Saves callers writing `ExternalHandle<MarkdownDoc>`.
pub type MarkdownHandle = ExternalHandle<MarkdownDoc>;

/// Props for the [`Markdown`] component.
///
/// Both props are [`Reactive`], so passing a live `Signal`/`rx!` (for
/// either the source text or the theme) makes the rendered document
/// update — the single native node is rebuilt on change.
#[derive(IdealystSchema)]
pub struct MarkdownProps {
    /// The CommonMark/GFM source to render. Static or reactive.
    pub source: Reactive<String>,
    /// Per-element-type resolved styling. Default is [`MdTheme::light`];
    /// pass a reactive theme (e.g. `rx!(if dark.get() { MdTheme::dark() }
    /// else { MdTheme::light() })`) to follow an app theme toggle.
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

/// Render a markdown document.
///
/// Parses `source` and paints it with `theme`, rebuilding the single
/// native node whenever either prop changes. On a backend without a
/// registered handler the framework shows its external placeholder.
#[component]
pub fn Markdown(props: &MarkdownProps) -> Element {
    let source = props.source.clone();
    let theme = props.theme.clone();
    // Reactive region: rebuild the one external node whenever the source
    // text or the resolved theme changes. `ui!` has no ergonomic form
    // for a reactive region keyed on a `(String, MdTheme)` tuple, so we
    // call `switch` directly — the documented direct-call form (see
    // `runtime_core::switch`). `switch` re-runs the branch only when the
    // tuple's `PartialEq` value actually differs, so static props build
    // exactly once.
    runtime_core::switch(
        move || (source.get(), theme.get()),
        move |key| {
            let (src, th) = key;
            markdown(src.clone(), th.clone()).into()
        },
    )
}

/// Low-level builder: construct a markdown `Element::External` from a
/// source string + resolved theme. Mirrors `codeblock::code_block` —
/// returns a `Bound<MarkdownHandle>` so `.with_style(...)` lands on the
/// outer native node (the container `<div>` / `UILabel` / `TextView`).
///
/// Prefer the [`Markdown`] component for reactive source/theme; this is
/// the escape hatch for one-shot rendering or custom plumbing.
pub fn markdown(source: impl Into<String>, theme: MdTheme) -> Bound<MarkdownHandle> {
    // Register the wire serde here too: `markdown` runs while the app
    // builds its tree, including on the runtime-server RECORDER (headless
    // app code). So the serializer is in place before the recorder's
    // `create_external` emits the wire command — no app-level recorder
    // wiring needed (codeblock pattern).
    ensure_wire_serde();
    let doc = parse::parse(&source.into(), theme);
    runtime_core::external::<MarkdownDoc>(doc)
}

/// Register the wire (serialize, deserialize) pair for [`MarkdownDoc`] so
/// a `markdown(...)` external renders over the runtime-server wire: the
/// recorder serializes the resolved doc into `CreateExternal`, the device
/// deserializes it and dispatches to its real per-backend handler.
///
/// Idempotent + cheap (thread-local guard). Called from [`markdown`]
/// (recorder side) and every [`register`] (device side).
fn ensure_wire_serde() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    runtime_core::register_external_serde(
        std::any::type_name::<MarkdownDoc>(),
        |any| {
            let doc = any.downcast_ref::<MarkdownDoc>()?;
            serde_json::to_vec(doc).ok()
        },
        |bytes| {
            let doc: MarkdownDoc = serde_json::from_slice(bytes).ok()?;
            Some(Rc::new(doc) as Rc<dyn Any>)
        },
    );
}

// =============================================================================
// Per-target `register` — the compiler picks the variant by target triple,
// so app bootstrap writes `markdown::register(&mut backend)` once.
// =============================================================================

/// Web (+ SSR) — registers the semantic-DOM handler.
#[cfg(target_arch = "wasm32")]
pub fn register<B: runtime_core::RegisterExternal>(backend: &mut B) {
    ensure_wire_serde();
    backend.register_external::<MarkdownDoc, _>(crate::web::build::<B>);
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
#[cfg(target_arch = "wasm32")]
inventory::submit! {
    backend_web::WebExternalRegistrar(register::<backend_web::WebBackend>)
}

/// Android — registers the `android` handler (one `TextView` +
/// `SpannableStringBuilder`).
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register(backend: &mut backend_android::AndroidBackend) {
    ensure_wire_serde();
    backend.register_external::<MarkdownDoc, _>(crate::android::build);
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
inventory::submit! {
    backend_android::AndroidExternalRegistrar(register)
}

/// iOS — registers the `ios` handler (one `UILabel` +
/// `NSAttributedString`).
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register(backend: &mut backend_ios::IosBackend) {
    ensure_wire_serde();
    backend.register_external::<MarkdownDoc, _>(crate::ios::build);
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
inventory::submit! {
    backend_ios::IosExternalRegistrar(register)
}

/// Fallback for other targets (macOS / terminal / gpu). No native
/// handler yet — still registers the wire serde so the recorder (which
/// compiles into this generic variant) serializes the payload.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "android"),
    not(target_os = "ios"),
))]
pub fn register<B: runtime_core::Backend>(_backend: &mut B) {
    ensure_wire_serde();
}
