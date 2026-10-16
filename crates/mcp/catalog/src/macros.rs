//! Hand-curated registration table for [`MacroEntry`].
//!
//! Same lock pattern as `primitives.rs` / `utilities.rs`: `MacroEntry`
//! carries a private `_seal: ()` so only this crate can construct one.
//! Every entry here documents a macro that actually exists — the
//! `macro_rules!` set lives in `runtime_vocabulary` (re-exported by
//! `runtime_core`) and the proc-macros in `runtime_macros` (`crates/runtime/macros/src/lib.rs`).
//! The drift audit (`.claude/audits/mcp-catalog-drift.md`) checks this
//! table against those definitions, so adding/removing a macro means
//! updating this file in the same change.
//!
//! `expansion` shows the primitive underneath so a reader never has to
//! guess what a macro lowers to — the gap that had authors reaching for
//! `Effect::new` instead of `effect!`. Left empty for proc-macros whose
//! codegen is too large to usefully summarize in one line.

use crate::{MacroEntry, MacroKind};

// ---------------------------------------------------------------------
// Reactive — state + reactivity (runtime_core macro_rules!)
//
// NOTE: signal creation is NOT here — there is no `signal!` macro;
// `signal(value)` is a plain function, documented as a `UtilityEntry` in
// `utilities.rs` under `UtilityCategory::Reactive`.
// ---------------------------------------------------------------------

inventory::submit! {
    MacroEntry {
        name: "effect",
        invocation: "effect!({ … })",
        kind: MacroKind::Reactive,
        module_path: "runtime_core",
        docs: "Write a reactive side effect **inside a component**: runs the body once, re-running whenever any signal it reads changes — dependencies are tracked automatically, there is no deps array. The macro inserts the `move ||` and there is no handle to manage; the surrounding component scope owns the effect and frees it on teardown (it debug-asserts a scope is active). To react to a signal from *outside* the component tree — app init, an async callback, a platform/service install — use the `watch(…)` function and store the returned `Subscription` (`.leak()` for a process-lifetime pin). Pair with `on_cleanup(...)` for teardown — the callback fires before the next re-run and on disposal. See [[reactivity]].",
        expansion: "Effect::scoped(move || { … });",
        snippet: "effect!({\n\t$0\n});",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "rx",
        invocation: "rx!(expr)",
        kind: MacroKind::Reactive,
        module_path: "runtime_core",
        docs: "Wrap an expression in a `Reactive` derived value — recomputes when the signals it reads change. Used to pass computed, auto-updating values into props (`content = rx!(format!(\"clicked {}×\", count.get()))`). For text specifically, an f-string literal (`text { \"count: {count}\" }`) is usually terser. See [[reactivity]].",
        expansion: "Reactive::derive(move || expr)",
        snippet: "rx!(${1:expr})",
        _seal: (),
    }
}

// `bind!` and `text_fmt!` are NOT here — both were removed in 0.3.
// Reactive text interpolation is now a TYPE-driven f-string on the
// text literal itself (`text { "count: {count}" }` — signal slots
// live, `Display` values baked), so neither the sentinel nor the
// template macro earns its tokens. See the `text` primitive docs and
// [[reactivity]].

// `memo` is NOT here either — like `signal!`, the `memo!` macro was
// removed (it only inserted `move ||` around a closure the author writes
// anyway). `memo(move || …)` is documented as a `UtilityEntry` in
// `utilities.rs` under `UtilityCategory::Reactive`.

// ---------------------------------------------------------------------
// Markup — element-tree construction (ui!/jsx!/lazy! are
// runtime_macros proc-macros; node_ref!/children! are runtime_core)
// ---------------------------------------------------------------------

inventory::submit! {
    MacroEntry {
        name: "ui",
        invocation: "ui! { … }",
        kind: MacroKind::Markup,
        module_path: "runtime_macros",
        docs: "The primary DSL for composing an element tree. Primitives are lowercase (`view`, `text`, `button`, …); components are PascalCase and dispatch through `BuildElement` (`Card(...)`, `Field(...)`). Supports `if` / `if let` / `match` branches and reactive keyed iteration — `for item in items, key = item.id { … }` where `items` is the `Signal<Vec<T>>` ITSELF (writing `for item in items.get()` freezes a build-time snapshot that never re-renders; see [[reactivity]] pitfalls and the `keyed_list_add_remove` recipe). Bare-identifier child splats supported — write children where they render, not in an out-of-macro `Vec::push` loop. A prop a primitive does not accept is a COMPILE ERROR spanned on the prop name, listing the valid props (with a did-you-mean) — so is a prop written twice, a conflicting pair (`image(src, asset)`), and a `{ … }` block on a primitive that takes no children. Every builder setter with a value has an inline spelling (`view(on_touch = …, bind = r)`, `image(on_load = …)`, `text_input(on_key_down = …)`); the few without one (`.container()`, `.on_handle(..)`) are named in the error. The whole invocation evaluates to an `Element` (the body is coerced through `IntoElement`), so never append `.into_element()` to it — that is the identity. The canonical component-body form; see [[component-hygiene]] and [[components]].",
        expansion: "IntoElement::into_element(<the tree>) — an `Element`",
        snippet: "ui! {\n\t$0\n}",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "jsx",
        invocation: "jsx! { <Foo prop=\"x\" expr={e}>…</Foo> }",
        kind: MacroKind::Markup,
        module_path: "runtime_macros",
        docs: "Angle-bracket peer of `ui!` — same dispatch, same `BuildElement` semantics, JSX-familiar syntax. Pick `ui!` or `jsx!` per file and stay in it; don't mix the two (or hand-built `Element`) in one component without a reason. Like `ui!`, it evaluates to an `Element` — no `.into_element()` after it. See [[component-hygiene]].",
        expansion: "IntoElement::into_element(<the tree>) — an `Element`",
        snippet: "jsx! {\n\t$0\n}",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "lazy",
        invocation: "lazy! { <ui-block> }",
        kind: MacroKind::Markup,
        module_path: "runtime_macros",
        docs: "**DEPRECATED** — use a lazy component instead: `#[component(lazy)]` / `#[lazy_component]` marks a component fn as the code-splitting boundary, with typed props crossing the split, readable chunk filenames, and the standard `loading`/`error` props (see the `lazy-loading` guide). The block form still works while deprecated: it takes a BRACE BLOCK of `ui!`-style markup — `lazy! { text { \"loaded on demand\" } }` — hoisted into a `#[wasm_split]` async fn; code reachable ONLY from it moves into a chunk loaded on first mount. **No captures** — the wasm-split fn is a plain `fn`, not an `Fn`, which is exactly why the component form (props cross the boundary as args) replaced it. Heavy extension SDK? Its registration is the main-bundle anchor: declare the kind at boot with `registry.defer::<P>()` and install the handler from inside the chunk with `register_deferred` / `runtime_scene::defer_registration` (see the [[sdk-components]] guide). See the [[lazy-loading]] guide.",
        expansion: "",
        snippet: "",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "node_ref",
        invocation: "node_ref!(Handle) / node_ref!()",
        kind: MacroKind::Markup,
        module_path: "runtime_core",
        docs: "Construct a typed `Ref<H>` — the handle a backend fills at mount time, read later via `.with(|h| …)`. Spelled `node_ref!` (not `ref!`) because `ref` is a reserved keyword. Two forms: `node_ref!(ViewHandle)` names the type explicitly, `node_ref!()` infers it from the let-binding type.",
        expansion: "Ref::new() / Ref::<H>::new()",
        snippet: "let ${1:handle} = node_ref!(${2:ViewHandle});",
        _seal: (),
    }
}

// ---------------------------------------------------------------------
// Animation (runtime_core macro_rules!)
// ---------------------------------------------------------------------

inventory::submit! {
    MacroEntry {
        name: "animated",
        invocation: "animated!(initial)",
        kind: MacroKind::Animation,
        module_path: "runtime_core",
        docs: "Construct an `AnimatedValue<T>` — the per-frame motion handle passed to `.animate(...)` and bound to a prop via `.bind(node_ref, AnimProp::…)`. `T` is inferred from the initial value (`f32` for scalar motion, a 4-tuple for color). AnimatedValue is for **continuous motion of a prop on an already-mounted node**. For **appearing/disappearing** (toast, panel, modal, disclosure — anything gated by an open/closed `Signal<bool>`), do NOT drive it imperatively with `animated!`/`.animate()` — that fades the node but never unmounts it. Use the **`presence` primitive** instead: it owns mount/unmount timing and plays declarative `enter`/`exit` animations. See [[reactivity]] § Animations and `describe_recipe(\"animated_toast\")`.",
        expansion: "AnimatedValue::new(initial)",
        snippet: "let ${1:av} = animated!(${2:initial});",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "timeline",
        invocation: "timeline! { at => { av: animator, … }, … }",
        kind: MacroKind::Animation,
        module_path: "runtime_core",
        docs: "Declarative multi-phase animation: each `at => { … }` clause fires one or more `av.animate(...)` calls at that moment; `AnimatedValue` handles are cloned into per-task closures automatically. Scheduled tasks are anchored to the current reactive scope — when the surrounding `effect!` re-runs or the `Owner` drops, every pending dispatch cancels, with no explicit `on_cleanup` boilerplate.",
        expansion: "scope-anchored after_ms(...) tasks, one per clause",
        snippet: "timeline! {\n\t${1:at_ms} => {\n\t\t${2:av}: ${3:animator},\n\t},$0\n}",
        _seal: (),
    }
}

// ---------------------------------------------------------------------
// Styling (runtime_macros proc-macro)
// ---------------------------------------------------------------------

inventory::submit! {
    MacroEntry {
        name: "stylesheet",
        invocation: "stylesheet! { … }",
        kind: MacroKind::Styling,
        module_path: "runtime_macros",
        docs: "Declare a typed stylesheet with variants and per-state overrides. `///` doc comments on the sheet land on the generated builder (struct + entry fn); `#[cfg]` and lint attributes (`allow`/`expect`/`warn`/`deny`/`forbid`) apply to every generated item; the rest of the expansion carries its own docs, so a sheet compiles under `deny(missing_docs)`. The `<…>` slot names the sheet's TOKEN VOCABULARY and each block's binding IS that vocabulary, so a theme token is a checked path, not a string: `pub Card<IdeaThemeRef> { base(t) { padding: t.spacing.md(), background: t.color.surface() } }`. The accessor path is the token name (`t.spacing.md()` → `spacing-md`, `t.intent.primary.solid_bg()` → `intent-primary-solid-bg`); namespaces are `t.color.*`, `t.intent.<intent>.<slot>()`, `t.spacing.*`, `t.radius.*`, `t.typography.*_size()`. The binding carries names only — theme VALUES arrive at resolve time from the token registry, so `theme.colors.primary` does not compile. Declare `<()>` when the sheet has no vocabulary (write its bindings `_t`) and reference any tokens with `Tokenized::token(\"name\", fallback)` — still the right form for app-defined token names. Outside a stylesheet, `idea_ui::tokens()` hands back the same namespace. `state foo(t) { … }` arms accept exactly the four framework interaction states (`hovered`, `pressed`, `focused`, `disabled`). See [[styling]] for the grammar and [[theming]] for the token table.",
        expansion: "builder fn + one enum per variant axis + `<name>_style() -> Rc<StyleSheet>`",
        snippet: "stylesheet! {\n\tpub ${1:Name}<${2:IdeaThemeRef}> {\n\t\tbase(t) {\n\t\t\t$0\n\t\t}\n\t}\n}",
        _seal: (),
    }
}

// ---------------------------------------------------------------------
// Props (runtime_macros attribute macro)
// ---------------------------------------------------------------------

inventory::submit! {
    MacroEntry {
        name: "props",
        invocation: "#[props]",
        kind: MacroKind::Component,
        module_path: "runtime_macros",
        docs: "Declare an explicit props struct for a `#[component] fn Name(props: &NameProps)`. Reactive-by-default: every data field is wrapped `Reactive<T>` so a call site may pass a static value, a `Signal<T>`, or `rx!(…)`; `#[prop(static)]` keeps a field plain, `#[prop(reactive)]` forces the wrap, `#[prop(default = expr)]` sets a non-`Default` starting value. Callbacks (`Rc<dyn Fn…>`), handles (`Ref<H>`), signals and `Vec<Element>` children are never wrapped. Prefer inline props (`#[component] fn Badge(label: String, #[prop(default = 3)] count: i32)`) unless the struct needs extra derives (`IdealystSchema`, doc-controls) or a hand-rolled `Default`. See [[component-hygiene]].",
        expansion: "the struct with data fields rewritten to `Reactive<T>` + a `Default` impl honouring `#[prop(default)]`",
        snippet: "#[props]\n#[derive(IdealystSchema)]\npub struct ${1:Name}Props {\n\t/// ${2:What the prop controls.}\n\tpub ${3:label}: ${4:String},$0\n}",
        _seal: (),
    }
}

// ---------------------------------------------------------------------
// Component (runtime_macros attribute macro)
// ---------------------------------------------------------------------

inventory::submit! {
    MacroEntry {
        name: "component",
        invocation: "#[component]",
        kind: MacroKind::Component,
        module_path: "runtime_macros",
        docs: "The canonical way to declare a component. Generates the props struct's `BuildElement` impl, the `pub type Tag = TagProps` alias that makes `Tag(...)` work as a `ui!` call site, and the `Default` glue struct-literal dispatch relies on — so don't hand-roll `BuildElement` impls or builder methods. Accepts `#[component(children)]` for container components and `#[component(default(field = expr))]` for non-Default starting values. Name container fns PascalCase to match the tag. See [[component-hygiene]].",
        expansion: "props struct BuildElement impl + `pub type Tag = TagProps` alias + Default glue",
        snippet: "/// ${1:What it renders.}\n#[component]\nfn ${2:Name}(${3:label: String}) -> Element {\n\tui! {\n\t\t$0\n\t}\n}",
        _seal: (),
    }
}

// ---------------------------------------------------------------------
// Catalog — documentation + introspection tooling (runtime_macros)
// ---------------------------------------------------------------------

inventory::submit! {
    MacroEntry {
        name: "recipe",
        invocation: "recipe!(Target, fn name() -> Element { … })",
        kind: MacroKind::Catalog,
        module_path: "runtime_macros",
        docs: "Register a compile-checked usage example for a documentable entity. The fn is real code built against the target's live API, so it fails to compile if the API drifts — self-verifying docs that also feed the MCP catalog (`list_recipes` / `describe_recipe`). Expands to nothing unless the `catalog` feature is on, so it costs zero in production. Keep the needed `use`s inside the fn body so the example is copy-pasteable.",
        expansion: "a RecipeEntry (catalog feature) / nothing (production)",
        snippet: "",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "doc_scope",
        invocation: "doc_scope!(Marker = \"Title\")",
        kind: MacroKind::Catalog,
        module_path: "runtime_macros",
        docs: "Declare a flat documentation scope (a labelled grouping of catalog entities) surfaced by `list_scopes` / `describe_scope`. Item macro — place at module scope alongside the entities it groups.",
        expansion: "a ScopeEntry",
        snippet: "doc_scope!(${1:Marker} = \"${2:Title}\");",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "IdealystSchema",
        invocation: "#[derive(IdealystSchema)]",
        kind: MacroKind::Catalog,
        module_path: "runtime_macros",
        docs: "Derive that captures a struct's or enum's shape (fields/variants plus their `///` docs) into a `TypeEntry`, so `describe_type` and the prop-field inliner can show it. Add it to props structs and the enums they reference so component docs resolve field and variant documentation.",
        expansion: "a TypeEntry describing the struct/enum",
        snippet: "",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "idealyst_tool",
        invocation: "#[idealyst_tool]",
        kind: MacroKind::Catalog,
        module_path: "runtime_macros",
        docs: "Attribute that registers a free function as an MCP-callable tool (a `ToolEntry`, surfaced by `list_tools` / `describe_tool`). The open extension point for third-party chat-callable helpers — distinct from `utilities`, which are author-time API docs, not chat-callable. Gated by the `catalog` feature.",
        expansion: "a ToolEntry exposing the fn over MCP",
        snippet: "",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "robot_fn",
        invocation: "#[robot_fn] / #[robot_fn(name = \"…\")]",
        kind: MacroKind::Catalog,
        module_path: "runtime_macros",
        docs: "Attribute that makes a free function callable by name over the robot bridge, for E2E tests and agents: seed data, fake a location, read app state an element doesn't show. Keeps the function and adds `<fn_ident>::robot()` (named after the Rust fn, not `name = \"…\"`); register it at boot with `runtime_vocabulary::robot_fn::register(f::robot())` (inert outside robot builds, so no cfg). A test calls it with `app.call_function(name, json!({…}))` / the `call_function` MCP tool — arguments by parameter name (serde), the return value as JSON, a `Result`'s `Err` as the failure; an `async fn` is polled to completion. The name defaults to the fn's; SDKs namespace theirs (`location.set`). Free functions only — no generics, `self` or borrowed parameters. See the [[testing]] guide.",
        expansion: "the fn, plus `mod <fn_ident> { pub fn robot() -> RobotFunction }`",
        snippet: "/// ${1:What it does}.\n#[robot_fn(name = \"${2:app.action}\")]\nfn ${3:action}($4) {\n\t$0\n}",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "robot_only",
        invocation: "robot_only! { … }",
        kind: MacroKind::Catalog,
        module_path: "runtime_vocabulary",
        docs: "Compile items only into robot builds — an SDK's test controls and the fake state behind them, so a release build carries none of it. Expands to its tokens with runtime-vocabulary's `robot` feature, to nothing without. The SDK depends on runtime-vocabulary and declares no cfg of its own (the feature is the vocabulary's, which `idealyst dev` turns on). Registration (`robot_fn::register`) needs no gate. See the [[testing]] guide.",
        expansion: "the tokens, or nothing",
        snippet: "runtime_vocabulary::robot_only! {\n\t$0\n}",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "robot_select",
        invocation: "robot_select! { on: { … } off: { … } }",
        kind: MacroKind::Catalog,
        module_path: "runtime_vocabulary",
        docs: "Pick items by build: the `on` tokens with runtime-vocabulary's `robot` feature, the `off` tokens without — e.g. a fakeable platform seam (`on: { mod controls; use controls::Source; } off: { type Source = Platform; }`). The robot_only! of two branches. See the [[testing]] guide.",
        expansion: "the `on` tokens or the `off` tokens",
        snippet: "runtime_vocabulary::robot_select! {\n\ton: { $1 }\n\toff: { $2 }\n}",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "log_info",
        invocation: "log_debug!(…) / log_info!(…) / log_warn!(…) / log_error!(…)",
        kind: MacroKind::Catalog,
        module_path: "runtime_core",
        docs: "Log at a level with `format!`-style arguments, through the installed `Logger` (the platform console: browser console, os_log, logcat, stderr). Reaches the robot's `get_logs` / the `get_logs` MCP tool, so it is how an app reports to a test or an agent. Use these instead of `println!`, which goes nowhere on web and mobile.",
        expansion: "logging::log(LogLevel::Info, &format!(…))",
        snippet: "log_info!(\"${1:msg}\"$0);",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "lazy_component",
        invocation: "#[lazy_component] / #[lazy_component(retryable)]",
        kind: MacroKind::Component,
        module_path: "runtime_macros",
        docs: "Shorthand for `#[component(lazy)]`: the component's body ships in a separate wasm chunk loaded on first mount, its props crossing the split as arguments, with the standard `loading` / `error` props. Accepts the same arguments as `#[component(...)]`, `lazy` implied. See the [[lazy-loading]] guide.",
        expansion: "#[component(lazy, …)]",
        snippet: "",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "host_fn",
        invocation: "#[host_fn]",
        kind: MacroKind::Component,
        module_path: "runtime_macros",
        docs: "An app function remote code can call: in the app it is the function; in a remote bundle it is a stub asking the app to run it. Arguments and result cross as `RemoteValue`s. May be generic over `Key`, `Opaque` and `Numeric` parameters (`#[derive(Key)]` for a key type), so a bundle can call it with types the app never compiled.",
        expansion: "",
        snippet: "",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "typeface",
        invocation: "typeface! { name, faces: [face!(…), …], fallback }",
        kind: MacroKind::Styling,
        module_path: "runtime_core",
        docs: "Declare a font family as a `static Typeface`: a name, its faces (each a `face!(weight: …, style: …, src: \"fonts/…\")`, path relative to the calling file) and a `SystemFallback`. Whether the bytes are embedded depends on runtime-core's `embed-font-bytes` feature: native backends load bytes, web links the bundled path.",
        expansion: "a `Typeface` with `AssetSource`-backed faces",
        snippet: "static ${1:INTER}: runtime_core::assets::Typeface = typeface! {\n\tname: \"${2:Inter}\",\n\tfaces: [\n\t\tface!(weight: FontWeight::Normal, style: FontStyle::Normal, src: \"${3:fonts/Inter-Regular.ttf}\"),\n\t],\n\tfallback: SystemFallback::SansSerif,\n};",
        _seal: (),
    }
}
