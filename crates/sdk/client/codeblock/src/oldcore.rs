//! Old-core surface — `Element::External` payload + the per-backend
//! `ExternalRegistry`. Byte-moved from the crate root when the
//! `new-core` leg landed (see lib.rs); the default build re-exports
//! everything here unchanged.

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{Backend, Bound, Color, ExternalHandle, IdealystSchema, RegisterExternal,
    StyleRules, Tokenized};

use std::rc::Rc;

/// Type-erased payload for the CodeBlock external primitive. Lives
/// here because the framework dispatches handlers by [`TypeId`], and
/// the type needs to be the SAME type across the call site and the
/// backend leaf — so it lives in the umbrella crate that both depend
/// on.
///
/// [`TypeId`]: std::any::TypeId
#[derive(Clone, IdealystSchema)]
pub struct CodeBlockProps {
    /// Color-run sequence. Each tuple is one run of same-colored
    /// text. Consecutive same-color runs are NOT auto-coalesced —
    /// the tokenizer caller decides whether to coalesce. The per-
    /// backend handlers don't pay for the difference because every
    /// run lowers to a single `ForegroundColorSpan` /
    /// `NSForegroundColorAttributeName` range / `<span>` inside ONE
    /// outer native node.
    pub spans: Vec<(String, Color)>,
}

/// Convenience handle alias — saves callers writing
/// `Ref<ExternalHandle<CodeBlockProps>>` everywhere.
pub type CodeBlockHandle = ExternalHandle<CodeBlockProps>;

/// Construct a `CodeBlock` from a flat span list.
///
/// Always returns an `Element::External` keyed by [`CodeBlockProps`];
/// the per-backend handler installed via [`register`] decides how to
/// render it. Returns a `Bound<CodeBlockHandle>` so `.with_style(...)`
/// works the same way it would for any other primitive — the style
/// lands on the outer native node (the `<pre>` / `HorizontalScrollView`
/// / `UIScrollView`).
///
/// ```ignore
/// code_block(vec![
///     ("fn ".into(),    Color("#888".into())),
///     ("hello".into(),  Color("#0a0".into())),
///     ("() { … }".into(), Color("#444".into())),
/// ])
/// ```
pub fn code_block(spans: Vec<(String, Color)>) -> Bound<CodeBlockHandle> {
    // Register the wire serde here too: `code_block` runs while the app
    // builds its tree, including on the runtime-server RECORDER (which
    // runs app code headless). So the serializer is in place before the
    // recorder's `create_external` emits the wire command — no app-level
    // recorder registration needed.
    ensure_wire_serde();
    runtime_core::external::<CodeBlockProps>(CodeBlockProps { spans })
}

/// Register the wire (serialize, deserialize) pair for `CodeBlockProps`
/// so a `code_block(...)` `Element::External` renders over the
/// runtime-server wire: the recorder serializes the spans into
/// `CreateExternal`, and the device deserializes them back and dispatches
/// to its real per-backend handler. Without this, External payloads can't
/// cross the wire and the device shows the not-available placeholder.
///
/// Idempotent + cheap (guarded by a thread-local flag). Called from
/// [`code_block`] (covers the recorder side) and from every [`register`]
/// (covers the device client side).
fn ensure_wire_serde() {
    thread_local! {
        static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    runtime_core::register_external_serde(
        std::any::type_name::<CodeBlockProps>(),
        |any| {
            let props = any.downcast_ref::<CodeBlockProps>()?;
            // `Color` is a `Color(String)` newtype; ship the runs as
            // (text, color-string) pairs.
            let plain: Vec<(&str, &str)> = props
                .spans
                .iter()
                .map(|(t, c)| (t.as_str(), c.0.as_str()))
                .collect();
            serde_json::to_vec(&plain).ok()
        },
        |bytes| {
            let plain: Vec<(String, String)> = serde_json::from_slice(bytes).ok()?;
            let spans = plain.into_iter().map(|(t, c)| (t, Color(c))).collect();
            Some(std::rc::Rc::new(CodeBlockProps { spans }) as std::rc::Rc<dyn std::any::Any>)
        },
    );
}

// =============================================================================
// Web / SSR — backend-neutral handler that uses the Backend trait so
// SSR + hydration share the same DOM shape. Compiled on every target
// (it only speaks the `Backend` trait): wasm32 wires it through
// `register`, host SSR through `register_generic`.
// =============================================================================

fn build_code_block<B: Backend>(props: &Rc<CodeBlockProps>, backend: &mut B) -> B::Node {
    let a11y = AccessibilityProps::default();
    let mut pre = backend.create_element("pre");
    for (text, color) in &props.spans {
        let span = backend.create_text(text, &a11y);
        let mut rules = StyleRules::default();
        rules.color = Some(Tokenized::Literal(color.clone()));
        backend.apply_style(&span, &Rc::new(rules));
        backend.insert(&mut pre, span);
    }
    pre
}

/// Register the backend-neutral `<pre>`/span handler on ANY external
/// registry — the host-side SSR/SSG seam (the wasm32 `register` wires
/// the same handler on web, so server render + hydration share one DOM
/// shape; the new-core scene handler in `newcore.rs` reproduces it
/// byte-for-byte). App SSR bootstrap calls this from
/// `register_ssr_extensions` (mirroring `swap_navigator::register_generic`);
/// without it, host SSR renders the External-not-registered placeholder
/// and code panels are missing from the static HTML.
pub fn register_generic<B: Backend + RegisterExternal>(backend: &mut B) {
    ensure_wire_serde();
    backend.register_external::<CodeBlockProps, _>(build_code_block::<B>);
}

// =============================================================================
// Per-target `register` — one per backend type. The variant of `register`
// that the compiler picks is determined by the target triple, so app
// bootstrap can write `codeblock::register(&mut backend)` once and
// not care which target it's compiling for.
// =============================================================================

/// Web (+ SSR via the same wasm32-target shell) — registers the
/// generic `build_code_block` handler against the backend's external
/// registry.
#[cfg(target_arch = "wasm32")]
pub fn register<B: RegisterExternal>(backend: &mut B) {
    ensure_wire_serde();
    backend.register_external::<CodeBlockProps, _>(build_code_block::<B>);
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
#[cfg(target_arch = "wasm32")]
inventory::submit! {
    backend_web::WebExternalRegistrar(register::<backend_web::WebBackend>)
}

/// Android — registers the [`crate::android::build`] handler. Produces a
/// single `RustCodeBlock` (HorizontalScrollView + TextView with
/// SpannableString). See `android.rs` for the JNI plumbing.
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register(backend: &mut backend_android::AndroidBackend) {
    ensure_wire_serde();
    backend.register_external::<CodeBlockProps, _>(crate::android::build);
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
inventory::submit! {
    backend_android::AndroidExternalRegistrar(register)
}

/// iOS — registers the [`crate::ios::build`] handler. Produces a single
/// UIScrollView (horizontal) wrapping a UILabel with
/// NSAttributedString. See `ios.rs` for the obj-c plumbing.
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register(backend: &mut backend_ios::IosBackend) {
    ensure_wire_serde();
    backend.register_external::<CodeBlockProps, _>(crate::ios::build);
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
inventory::submit! {
    backend_ios::IosExternalRegistrar(register)
}

/// macOS — registers the [`crate::macos::build`] handler. Produces a single
/// `NSScrollView` (horizontal) wrapping an `NSTextField` label with an
/// `NSAttributedString`. See `macos.rs` for the obj-c plumbing.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub fn register(backend: &mut backend_macos::MacosBackend) {
    ensure_wire_serde();
    backend.register_external::<CodeBlockProps, _>(crate::macos::build);
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
inventory::submit! {
    backend_macos::MacosExternalRegistrar(register)
}

/// Fallback for other targets (terminal / gpu). No-op generic
/// over any `Backend`. Authors get the framework's standard
/// external-not-registered placeholder until a per-backend handler
/// lands.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "android"),
    not(target_os = "ios"),
    not(target_os = "macos"),
))]
pub fn register<B: runtime_core::Backend>(_backend: &mut B) {
    // No per-backend native handler here, but still register the wire
    // serde so the recorder (which falls into this generic variant)
    // serializes the payload.
    ensure_wire_serde();
}
