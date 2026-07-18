//! Reactive text bindings page — built via the `docs!` macro.
//!
//! Covers f-string text (`text { "count: {count}" }`) and the
//! `TextSource::JsBinding` structure it produces — the web backend's
//! fast path for hierarchy-scale reactive text. Most app authors only
//! need the first section; the rest is for people who measured a real
//! fan-out cost and want to understand the machinery.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "reactive-text-bindings",
    title = "Reactive text bindings",
    category = Advanced,
    description = "How f-string text slots become live bindings, and the web backend's fast path that moves per-fire fan-out off the wasm boundary at hierarchy scale.",
    related = ["reactivity", "primitives", "backends"],
    concepts = [
        JsBinding, FStringText, TextSlotDispatch, RegisterSignalForJs,
        Signal, Effect,
    ],

    section(heading = "The authoring surface") {
        p("A string literal in text position interpolates ",
          code("{name}"),
          " placeholders the way Rust's own ", code("format!"),
          " treats inline named arguments — and each slot is live or \
           static by the interpolated value's TYPE:"),
        code(rust, r##"
            let count = signal(0);
            let doubled = memo(move || count.get() * 2);
            let unit: &str = "items";   // plain Display value

            ui! {
                // `count` and `doubled` are signals → LIVE slots.
                // `unit` is a plain value → baked in at build time.
                text { "count: {count} {unit}   doubled: {doubled:.1}" }
            }
        "##),
        p("Signals and memo outputs (", code("ReadSignal"),
          ") subscribe automatically — no closure, no ", code(".get()"),
          ". Format specs pass through (width, precision, fill); ",
          code("{{"), " / ", code("}}"),
          " escape literal braces. Positional ", code("{}"),
          " and Debug ", code("{x:?}"),
          " are not supported in text f-strings — use ",
          code("text { move || format!(…) }"),
          " for those. A literal with no valid ", code("{ident}"),
          " placeholder never changes meaning: brace-containing prose \
           renders verbatim."),
        p("That's the whole authoring story. Everything below is what \
           the lowering produces and why it's fast."),
    },

    section(heading = "When the fast path matters") {
        p("If you're rendering a few reactive labels — counters, \
           status text, a clock — any reactive text form is fast \
           enough. The default Effect path is fine for normal UIs."),
        p("If you're rendering thousands of reactive text nodes that \
           all subscribe to the same signal — long simulation grids, \
           dense data tables, leaderboards updating per tick — the \
           Effect path's cost is dominated by per-leaf framework \
           bookkeeping + wasm→JS marshalling. F-string signal slots \
           produce a structured binding that moves that fan-out \
           entirely to JS, closing most of the gap to compiled \
           frameworks like Svelte for that specific pattern — \
           automatically, with no opt-in syntax."),
    },

    section(heading = "What the lowering produces") {
        p("A text literal whose slots are all plain values bakes to ",
          code("TextSource::Static"),
          " — zero reactive machinery. Signal slots produce a ",
          code("TextSource::JsBinding(JsBindingSpec)"),
          " carrying a structured payload:"),
        list(
            [code("signal_ids"),
             ": ", code("Vec<u64>"),
             ", the arena ids of each signal slot, in template order"],
            [code("template_parts"),
             ": ", code("Vec<String>"),
             ", N+1 static text parts surrounding the N signal slots \
              (baked static slots are pre-formatted into these)"],
            [code("initial_values"),
             ": ", code("Vec<String>"),
             ", the starting value of each signal as a string — used \
              to seed the JS-side cache and compute the binding's \
              initial ", code("nodeValue"), " synchronously at mount"],
            [code("stringifiers"),
             ": ", code("Vec<Rc<dyn Fn() -> String>>"),
             ", one per signal slot — the web backend installs these \
              as per-signal JS notifiers at bind time, so a ",
             code("signal.set"),
             " ships its new value across the wasm→JS boundary in one \
              FFI hop"],
            [code("compute_fallback"),
             ": ", code("Rc<dyn Fn() -> String>"),
             ", a closure that re-evaluates the full template — used \
              by the walker when the active backend does not opt into \
              JS bindings (everything except web today)."],
        ),
        p("A slot that is reactive but has no signal id — a ",
          code("Reactive<T>"),
          " prop whose value is a derived closure — can't ride the \
           template binding; it routes the whole text through the \
           regular ", code("Bound"),
          " Effect path instead. Correct everywhere, just without the \
           fan-out speedup."),
        p("You can construct the spec by hand if you're generating \
           bindings programmatically:"),
        code(rust, r##"
            use runtime_core::{text, JsBindingSpec, TextSource};
            use std::rc::Rc;

            text(TextSource::JsBinding(JsBindingSpec {
                signal_ids: vec![global.id()],
                template_parts: vec!["leaf 42: g=".into(), "".into()],
                initial_values: vec!["0".into()],
                compute_fallback: Rc::new(move || {
                    format!("leaf 42: g={}", global.get())
                }),
                stringifiers: vec![Rc::new(move || {
                    runtime_core::untrack(|| global.get()).to_string()
                })],
            }))
        "##),
        p("The f-string lowering exists so authors don't keep the \
           template + signal_ids + initials + stringifiers + \
           compute_fallback in sync by hand — change the literal and \
           every derived field updates together."),
    },

    section(heading = "Per-signal notifiers") {
        p("Before a binding can fire for a signal, the web backend \
           needs to know how to stringify that signal's value when it \
           changes. The walker auto-installs the spec's ",
          code("stringifiers"),
          " as per-signal JS notifiers at bind time (skipping any \
           signal that already has one, so a custom notifier installed \
           first — e.g. by a class binding — is never clobbered)."),
        p("From that point, every ", code("global.set(...)"),
          " runs the Rust subscriber fan-out AS BEFORE, then ALSO \
           ships ", code("(sid, stringified_value)"),
          " across the wasm→JS boundary in one FFI hop. The JS-side \
           reactive layer holds the binding registry and does the \
           per-binding fan-out internally — no per-leaf wasm crossing."),
        p("The stringifiers read ", code("untrack"),
          "ed: they run from inside ", code("Signal::set"),
          ", and a tracked read there would subscribe whatever outer \
           Effect happened to be running to the signal being updated."),
    },

    section(heading = "What the fast path saves per fire") {
        p("On a 20 k-leaf fan-out (one signal change, all leaves \
           subscribe), the difference vs the Effect path:"),
        list(
            ["No per-leaf Rust ", code("Effect"),
             " runs. ", code("run_effect"),
             "'s bookkeeping (RefCell borrows, scope stack push/\
              pop, subscriber HashSet maintenance) at 20 k× was \
              ~50 ms of the old apply window."],
            ["No per-leaf ", code("format!()"),
             " allocations. The JS-side composer reads cached signal \
              values and concatenates against the prebaked parts; \
              the per-fire ", code("String"),
             " is built and consumed inside V8's hot loop."],
            ["No per-leaf wasm→JS ", code("update_text"),
             " FFI call. The signal change ships ONCE across the \
              boundary; the JS shim walks 20 k bindings inside that \
              single hop."],
            ["The Rust subscriber list for these signals is empty \
              (no Effects subscribe to them), so the ",
             code("collect_subscribers"),
             " on ", code("Signal::set"), " is a no-op."],
        ),
        p("Result on the hierarchy bench: 283 ms → 20 ms at \
           20 k leaves, below Svelte's ~32 ms for the same shape. \
           Practical end-app speedup will depend on how much of \
           your apply time was actually in this pattern."),
    },

    section(heading = "Cross-backend behaviour") {
        p("Signal-slot f-strings produce ", code("TextSource::JsBinding"),
          " unconditionally. The walker, when building it, checks \
           the active backend's ", code("supports_js_text_bindings()"),
          ":"),
        list(
            [code("true"), " (web backend with ",
             code("install_text_batcher(&backend)"),
             " called): the walker calls ",
             code("backend.register_reactive_text_binding(...)"),
             " and does NOT install a Rust Effect. The \
              JS-side binding registry handles updates."],
            [code("false"),
             " (every native backend today, and web without the \
              batcher installed): the walker lowers to the legacy ",
             code("Bound"), " Effect path using ", code("compute_fallback"),
             ". Output is identical; you just don't get the fan-out \
              speedup. No regression vs a closure-form ",
             code("text"), " node."],
        ),
        p("This is why ", code("compute_fallback"),
          " is mandatory in the spec — it's the universal path that \
           makes the same source correct on web (fast), iOS / Android \
           / wgpu (correct, regular Effect), and the wire/runtime-\
           server generator backends."),
    },

    section(heading = "Lifecycle and cleanup") {
        p("Bindings are registered with the JS-side layer at mount \
           time. The framework wires a ", code("crate::on_cleanup"),
          " callback on the surrounding reactive scope so that on \
           scope drop (switch-arm flip, component unmount, owner \
           drop) the walker calls ",
          code("release_reactive_text_binding(text_id)"),
          " + ", code("release_text_id(text_id)"),
          " on the backend, which removes the binding from the JS \
           registry. There's no leak across mount/unmount cycles."),
        p("Per-signal notifier closures are dropped automatically \
           when their signal's arena slot is freed (",
          code("take_signals_batched"),
          " removes them from ", code("signal_js_notifiers"),
          " when the slot is recycled). Manual ",
          code("unregister_signal_js_notifier"),
          " exists if you want to detach a still-live signal from \
           JS subscription (rare)."),
    },

    section(heading = "When to reach for the closure form instead") {
        list(
            ["Positional or Debug formatting, or interpolating an \
              EXPRESSION rather than a name in scope — ",
             code("text { move || format!(…) }"),
             " is the general form and stays fully supported."],
            ["Heavy formatting per fire. The JS-side composer is a \
              simple concatenation; locale-aware number formatting, \
              date rendering, etc. won't fit the template model — \
              keep those in the closure form (Effect path)."],
        ),
        p("Derived values need no special handling: a ",
          code("memo"), " output is a ", code("ReadSignal"),
          " and interpolates as a live slot like any signal."),
    },

    section(heading = "The smoke test") {
        p("If something looks wrong — bindings not updating, stale \
           values — there's a built-in smoke test you can run from \
           a devtools console with the web variant loaded:"),
        code(javascript, r##"
            __idealystBindingsSmokeTest()
            // logs PASS / FAIL plus details
        "##),
        p("It creates two text nodes, registers them as bindings \
           sharing one synthetic signal id, fires a value change, \
           and asserts both nodes' ", code("nodeValue"),
          " updated. Useful for confirming the JS shim is loaded \
           and the registry-update path is alive."),
        p("Diagnostic counters are exposed at ",
          code("window.__idealystBindingStats"),
          " — registrations, releases, signal notifications, total \
           bindings updated. A non-zero ", code("signalNotifications"),
          " with zero ", code("bindingsUpdated"),
          " means signals are firing but no bindings are subscribed \
           — almost always a notifier that never got installed."),
    },

    section(heading = "Mental model") {
        p("The Rust signal is still the source of truth. There is \
           no second copy of state somewhere — JS holds a cache of \
           the LAST value each subscribed signal sent. Set the \
           signal, the cache updates, the dependent bindings \
           rewrite their nodes."),
        p("Rust Effects and JS bindings can both observe the same \
           signal. They don't compete — Rust Effects fire on the \
           Rust subscriber list, JS bindings update on the JS-side \
           fan-out triggered by the notifier. The framework simply \
           does both. For signal-slot f-string leaves, there is no \
           Rust Effect by design — the JS binding is the sole DOM \
           writer for those nodes — so there's no \"two sources of \
           truth\" problem in practice."),
        p("The whole feature is a backend optimization, not a \
           programming-model shift. Rewrite every f-string as ",
          code("text { move || format!(…) }"),
          " and the app behaves the same — slower at hierarchy \
           scale, same correctness."),
    },
}
