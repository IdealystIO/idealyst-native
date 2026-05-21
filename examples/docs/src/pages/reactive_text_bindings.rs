//! Reactive text bindings page — built via the `docs!` macro.
//!
//! Covers `text_fmt!` / `bind!` / `TextSource::JsBinding` — the web
//! backend's fast path for hierarchy-scale reactive text. Most app
//! authors don't need to know any of this; the page is for people
//! who measured a real fan-out cost and want to opt into the path.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{codeblock, pageheader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{body, card, heading, stack};

docs! {
    slug = "reactive-text-bindings",
    title = "Reactive text bindings",
    category = Advanced,
    description = "Opt-in fast path for reactive text on the web backend. Moves per-fire fan-out off the wasm boundary at hierarchy scale.",
    related = ["reactivity", "primitives", "backends"],
    concepts = [
        JsBinding, TextFmtMacro, BindSentinel, RegisterSignalForJs,
        Signal, Effect,
    ],

    section(heading = "When you need this") {
        p("If you're rendering a few reactive labels — counters, \
           status text, a clock — ", code("text(closure)"),
          " is the right tool. The default path is fast for normal \
           UIs."),
        p("If you're rendering thousands of reactive text nodes that all subscribe to the same \
           signal — long simulation grids, dense data tables, \
           leaderboards updating per tick — the default path's \
           cost is dominated by per-leaf framework bookkeeping + \
           wasm→JS marshalling. ", code("text_fmt!"),
          " is the opt-in that moves that fan-out entirely to JS, \
           closing most of the gap to compiled frameworks like \
           Svelte for that specific pattern."),
        p("Measure first. The cost only shows up at the ~thousand-\
           leaf scale; below that, the optimization buys you \
           ergonomics that aren't there (it's not less code) and \
           a couple new concepts to keep in your head."),
    },

    section(heading = "The shape") {
        p("Two flavours of arg in the macro, and the macro produces a \
           value — not a primitive — so it drops into any component \
           slot that accepts a text source:"),
        code(rust, r##"
            use framework_core::{bind, text_fmt, signal, Signal};

            let id: u32 = 42;
            let global: Signal<u32> = signal!(0);

            Text { text_fmt!("leaf {}: g={}", id, bind!(global)) }
        "##),
        p("Args wrapped in ", code("bind!(...)"),
          " are signals — the framework subscribes to them via ",
          code(".id()"), " and the active backend's binding layer \
           recomputes the node's text when they fire. Bare exprs \
           are captured by value at construction time and ",
          code("Display"),
          "-formatted into the template's static parts; they \
           never re-evaluate."),
        p("Only plain ", code("{}"),
          " placeholders are supported today — no ", code("{:?}"),
          ", no positional ", code("{0}"), ", no named ",
          code("{name}"), " (yet). Escape literal braces with ",
          code("{{"), " / ", code("}}"), ", same as ",
          code("format!"), "."),
    },

    section(heading = "What it produces") {
        p(code("text_fmt!"), " expands to a ", code("TextSource::JsBinding(...)"),
          " value — drop it into ", code("Text { ... }"),
          " (or any component arg that takes ", code("IntoTextSource"),
          " — e.g. ", code("Button(label = text_fmt!(...))"),
          "). The macro carries a structured payload:"),
        list(
            [code("signal_ids"),
             ": ", code("Vec<u64>"),
             ", the arena ids of each signal slot, in template order"],
            [code("template_parts"),
             ": ", code("Vec<String>"),
             ", N+1 static text parts surrounding the N signal slots \
              (captured exprs are pre-formatted into these)"],
            [code("initial_values"),
             ": ", code("Vec<String>"),
             ", the starting value of each signal as a string — used \
              to seed the JS-side cache and compute the binding's \
              initial ", code("nodeValue"), " synchronously at mount"],
            [code("compute_fallback"),
             ": ", code("Rc<dyn Fn() -> String>"),
             ", a closure that re-evaluates the full template — \
              used by the walker when the active backend does not \
              opt into JS bindings (everything except web today). \
              Same code shape a legacy ", code("text(closure)"),
             " would have carried."],
        ),
        p("You can construct the spec by hand if you have a reason \
           to skip the macro:"),
        code(rust, r##"
            use framework_core::{text, JsBindingSpec, TextSource};
            use std::rc::Rc;

            text(TextSource::JsBinding(JsBindingSpec {
                signal_ids: vec![global.id()],
                template_parts: vec!["leaf 42: g=".into(), "".into()],
                initial_values: vec!["0".into()],
                compute_fallback: Rc::new(move || {
                    format!("leaf 42: g={}", global.get())
                }),
            }))
        "##),
        p("The macro exists so authors don't have to keep the \
           template + signal_ids + initials + compute_fallback in \
           sync by hand — change the macro's format string and \
           every derived field updates together."),
    },

    section(heading = "The per-signal one-time setup") {
        p("Before any binding fires for a signal, the web backend \
           needs to know how to stringify the signal's value when \
           it changes. Register each signal once at app startup:"),
        code(rust, r##"
            // Inside your `app()` function or wherever signals are
            // created, after the backend is mounted but before the
            // first text_fmt! binding is built.
            BACKEND.with(|s| {
                if let Some(backend_rc) = s.borrow().as_ref() {
                    let mut b = backend_rc.borrow_mut();
                    b.register_signal_for_js(global.id(), move || {
                        framework_core::untrack(|| global.get()).to_string()
                    });
                }
            });
        "##),
        p(code("register_signal_for_js"),
          " installs a closure in the framework's per-signal \
           notifier slot. From that point, every ", code("global.set(...)"),
          " runs the Rust subscriber fan-out AS BEFORE, then ALSO \
           ships ", code("(sid, stringified_value)"),
          " across the wasm→JS boundary in one FFI hop. The JS-side \
           reactive layer holds the binding registry and does the \
           per-binding fan-out internally — no per-leaf wasm crossing."),
        p("The ", code("untrack"),
          " wrapping is defensive: the stringifier closure runs \
           from inside ", code("Signal::set"),
          ", and if that ", code("set"),
          " was itself called inside an outer Effect, an un-",
          code("untrack"),
          "-ed read would subscribe the outer Effect to the signal \
           we're already updating. Always wrap."),
    },

    section(heading = "What the macro saves you per fire") {
        p("On a 20 k-leaf fan-out (one signal change, all leaves \
           subscribe), the difference vs ", code("text(closure)"),
          ":"),
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
        p("A ", code("text_fmt!"),
          " call produces ", code("TextSource::JsBinding"),
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
              speedup. No regression vs the old ",
             code("text(closure)"), " shape."],
        ),
        p("This is why ", code("compute_fallback"),
          " is mandatory in the spec — it's the universal path that \
           lets ", code("text_fmt!"),
          " be source-portable. A binding written once with ",
          code("text_fmt!"),
          " behaves correctly on web (fast), iOS / Android / wgpu \
           (correct, regular Effect), and the wire/AAS generator \
           backends (still emits the same primitive metadata)."),
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
        p("Per-signal notifier closures installed by ",
          code("register_signal_for_js"),
          " are dropped automatically when their signal's arena \
           slot is freed (",
          code("take_signals_batched"),
          " removes them from ", code("signal_js_notifiers"),
          " when the slot is recycled). Manual ",
          code("unregister_signal_js_notifier"),
          " exists if you want to detach a still-live signal from \
           JS subscription (rare)."),
    },

    section(heading = "When the macro is the wrong choice") {
        list(
            ["Reading non-signal state inside the format string. \
              ", code("text_fmt!"),
             " distinguishes signals (via ", code("bind!"),
             ") from captured values. ANY reactive read that isn't \
              a plain ", code("Signal<T>::get()"),
             " — derived values, memoized computations, signal-of-\
              signal — needs to go through the ", code("compute_fallback"),
             " path. Just use ", code("text(closure)"),
             " for those — the macro's win evaporates if the \
              expression isn't a clean template + signals."],
            ["Heavy formatting per fire. The JS-side composer is a \
              simple concatenation; templates with complex format \
              specs (locale-aware number formatting, date rendering, \
              etc.) won't fit and won't help — keep those in ",
             code("text(closure)"), "."],
            ["Backends that don't support JS bindings. Falls back \
              correctly to the legacy path, so this isn't ",
             code("wrong"), " — but you're paying the cost of \
              constructing the spec and ", code("compute_fallback"),
             " for no benefit. If a screen is iOS-only, just write \
              ", code("text(closure)"), " and move on."],
        ),
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
           — almost always a missing ",
          code("register_signal_for_js"),
          " call."),
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
           does both. For ", code("text_fmt!"),
          " leaves, there is no Rust Effect by design — the JS \
           binding is the sole DOM writer for those nodes — so \
           there's no \"two sources of truth\" problem in practice."),
        p("The whole feature exists as a backend optimization, not \
           a programming-model shift. If you delete every ",
          code("text_fmt!"), " call and replace with ",
          code("text(closure)"),
          ", the app behaves the same — slower at hierarchy scale, \
           same correctness."),
    },
}
