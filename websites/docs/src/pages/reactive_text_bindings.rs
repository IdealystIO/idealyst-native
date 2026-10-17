//! Reactive text bindings page — built via the `docs!` macro.
//!
//! Covers f-string text (`text { "count: {count}" }`) and the
//! pre-decomposed JS binding it lowers to — the web backend's
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
        p("Signals, read halves (", code("ReadSignal"), "), and memo outputs (",
          code("Memo"), ") subscribe automatically — no closure, no ", code(".get()"),
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
        p("A text literal whose slots are all plain values bakes to a \
           constant string — zero reactive machinery. A literal with signal \
           slots lowers to a pre-decomposed binding that the ", code("text"),
          " mount handler receives. It carries, per text node:"),
        list(
            [code("signal_ids"), ": ", code("Vec<u64>"),
             ", each signal slot's ", code("raw_id()"), ", in template order"],
            [code("template_parts"), ": ", code("Vec<String>"),
             ", N+1 static text parts surrounding the N signal slots \
              (baked static slots are pre-formatted into these)"],
            [code("initial_values"), ": ", code("Vec<String>"),
             ", each signal's starting value as a string — used to compute \
              the node's first ", code("nodeValue"), " at mount"],
            [code("stringifiers"),
             ": one untracked read-and-format closure per signal slot, handed \
              to the backend at registration"],
            [code("tracked_reads"),
             ": one tracked read-and-format closure per signal slot — the body \
              of that signal's notifier effect (next section)"],
            [code("compute_fallback"),
             ": a tracked closure that re-evaluates the whole template — the \
              effect body on backends without JS bindings (everything except \
              web today)."],
        ),
        p("A slot that is reactive but has no signal id — a ",
          code("Reactive<T>"),
          " prop whose value is a derived closure — can't ride the \
           template binding; it routes the whole text through a regular \
           effect instead. Correct everywhere, just without the fan-out \
           speedup."),
        p("The binding type is internal to the framework: there is no public \
           constructor for apps. The f-string lowering is the way to produce \
           one, so the template, ids, initial values, and readers can never \
           drift out of sync — change the literal and every field updates \
           together."),
    },

    section(heading = "Per-signal notifiers") {
        p("World signals have no write hook, so nothing happens inside ",
          code("Signal::set"),
          ". Instead, the text handler leases ONE notifier effect per signal \
           in the world. The effect reads the signal (tracked) and calls the \
           backend's ", code("notify_signal_text_js(signal_id, value)"),
          ", which ships ", code("(sid, formatted value)"),
          " across the wasm→JS boundary in one FFI hop. The JS-side layer holds \
           the binding registry and does the per-binding fan-out internally — \
           no per-leaf wasm crossing."),
        p("Every binding to that signal shares the same notifier, whether the \
           binding is a text node or a class binding — the first one installs \
           it, later ones take a lease. The notifier lives while any lease is \
           held and is freed with the last one. Its first run happens when it \
           is created, before the binding registers, so the JS value cache is \
           seeded for the first paint."),
        p("Because the notifier is an ordinary effect, delivery follows the \
           normal flush: writes stage, the flush commits them, and the \
           notifier runs once per flush for its signal — then JS updates every \
           bound node."),
    },

    section(heading = "What the fast path saves per fire") {
        p("On a 20 k-leaf fan-out (one signal change, all leaves \
           subscribe), the difference vs one effect per leaf:"),
        list(
            ["One Rust effect run per changed signal instead of one per leaf."],
            ["No per-leaf ", code("format!()"),
             " allocations. The JS-side composer reads cached signal \
              values and concatenates against the prebaked parts; \
              the per-fire ", code("String"),
             " is built and consumed inside the JS engine's hot loop."],
            ["No per-leaf wasm→JS ", code("update_text"),
             " FFI call. The signal change ships ONCE across the \
              boundary; the JS shim walks the bindings inside that \
              single hop."],
        ),
        p("Result on the hierarchy bench: 283 ms → 20 ms at \
           20 k leaves, below Svelte's ~32 ms for the same shape. \
           Practical end-app speedup will depend on how much of \
           your apply time was actually in this pattern."),
    },

    section(heading = "Cross-backend behaviour") {
        p("Signal-slot f-strings always produce the binding. The text handler \
           checks the active backend's ", code("supports_js_text_bindings()"),
          " when it mounts the node:"),
        list(
            [code("true"), " (the web backend, once ",
             code("install_global_self(&backend)"),
             " has run — the web boot entries call it for you): the handler \
              calls ", code("register_reactive_text_binding(...)"),
             ", leases the per-signal notifiers, and installs NO per-leaf \
              effect. The JS-side binding registry handles updates."],
            [code("false"),
             " (every native backend today): the handler installs one effect \
              per node that runs ", code("compute_fallback"),
             ". Output is identical; you just don't get the fan-out \
              speedup. No regression vs a closure-form ",
             code("text"), " node."],
        ),
        p("This is why ", code("compute_fallback"),
          " is part of every binding — it's the universal path that \
           makes the same source correct on web (fast), iOS / Android \
           / wgpu (correct, regular effect), and the wire/runtime-\
           server generator backends."),
    },

    section(heading = "Lifecycle and cleanup") {
        p("Bindings are registered with the JS-side layer at mount. The \
           handler registers a teardown on the node, so when the node is torn \
           down (an ", code("if"), " branch flips, a component unmounts) it \
           calls ", code("release_reactive_text_binding(text_id)"),
          " and ", code("release_text_id(text_id)"),
          " on the backend, removing the binding from the JS registry, and \
           then drops the binding's notifier leases. There's no leak across \
           mount/unmount cycles."),
        p("A notifier is freed when the last binding that leased it goes \
           away, so it never outlives the bindings — or the signal — it serves."),
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
          code("memo"), " output is a ", code("Memo<T>"),
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
           per-leaf Rust effect by design — the JS binding is the sole DOM \
           writer for those nodes — so there's no \"two sources of \
           truth\" problem in practice."),
        p("The whole feature is a backend optimization, not a \
           programming-model shift. Rewrite every f-string as ",
          code("text { move || format!(…) }"),
          " and the app behaves the same — slower at hierarchy \
           scale, same correctness."),
    },
}
