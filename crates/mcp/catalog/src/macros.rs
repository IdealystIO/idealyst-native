//! Hand-curated registration table for [`MacroEntry`].
//!
//! Same lock pattern as `primitives.rs` / `utilities.rs`: `MacroEntry`
//! carries a private `_seal: ()` so only this crate can construct one.
//! Every entry here documents a macro that actually exists — the
//! `macro_rules!` set lives in `runtime_core` (`crates/runtime/core/src/lib.rs`)
//! and the proc-macros in `runtime_macros` (`crates/runtime/macros/src/lib.rs`).
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
// NOTE: signal creation is NOT here — the `signal!` macro was removed
// (it expanded verbatim to `Signal::new`, no macro-only capability) and
// replaced by the plain `signal(value)` function, documented as a
// `UtilityEntry` in `utilities.rs` under `UtilityCategory::Reactive`.
// ---------------------------------------------------------------------

inventory::submit! {
    MacroEntry {
        name: "effect",
        invocation: "effect!({ … })",
        kind: MacroKind::Reactive,
        module_path: "runtime_core",
        docs: "Write a reactive side effect **inside a component**: runs the body once, re-running whenever any signal it reads changes — dependencies are tracked automatically, there is no deps array. The macro inserts the `move ||` and there is no handle to manage; the surrounding component scope owns the effect and frees it on teardown (it debug-asserts a scope is active). To react to a signal from *outside* the component tree — app init, an async callback, a platform/service install — use the `watch(…)` function and store the returned `Subscription` (`.leak()` for a process-lifetime pin). Pair with `on_cleanup(...)` for teardown — the callback fires before the next re-run and on disposal. See [[reactivity]].",
        expansion: "Effect::scoped(move || { … });",
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
        docs: "The primary DSL for composing an element tree. Primitives are lowercase (`view`, `text`, `button`, …); components are PascalCase and dispatch through `BuildElement` (`Card(...)`, `Field(...)`). Supports `if` / `if let` / `match` branches and `for item in items { … }` iteration inline, plus bare-identifier child splats — write children where they render, not in an out-of-macro `Vec::push` loop. The canonical component-body form; see [[component-hygiene]] and [[components]].",
        expansion: "",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "jsx",
        invocation: "jsx! { <Foo prop=\"x\" expr={e}>…</Foo> }",
        kind: MacroKind::Markup,
        module_path: "runtime_macros",
        docs: "Angle-bracket peer of `ui!` — same dispatch, same `BuildElement` semantics, JSX-familiar syntax. Pick `ui!` or `jsx!` per file and stay in it; don't mix the two (or hand-built `Element`) in one component without a reason. See [[component-hygiene]].",
        expansion: "",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "lazy",
        invocation: "lazy!(path::to::Component)",
        kind: MacroKind::Markup,
        module_path: "runtime_macros",
        docs: "Marks a code-splitting boundary: the wrapped subtree builds behind a lazily-loaded chunk on backends that support splitting (web), and inline elsewhere. See the `lazy` module in runtime-macros for constraints and naming.",
        expansion: "",
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
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "children",
        invocation: "children![a, opt, vec]",
        kind: MacroKind::Markup,
        module_path: "runtime_core",
        docs: "Build a `Vec<Element>` from a mixed-shape list, flattening `Option<Element>` (skips `None`) and `Vec<Element>` (extends inline) so call sites write conditionals naturally. For *flattening received children* in a container component — not for authoring new children in a push loop (write those inside `ui!`). See [[component-hygiene]].",
        expansion: "a Vec<Element> built via ChildList::append_to",
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
        docs: "Construct an `AnimatedValue<T>` — the per-frame motion handle passed to `.animate(...)` and bound to a prop via `.bind(node_ref, AnimProp::…)`. `T` is inferred from the initial value (`f32` for scalar motion, a 4-tuple for color).",
        expansion: "AnimatedValue::new(initial)",
        _seal: (),
    }
}

inventory::submit! {
    MacroEntry {
        name: "animate_at",
        invocation: "animate_at!(at_ms, av, animator)",
        kind: MacroKind::Animation,
        module_path: "runtime_core",
        docs: "Schedule one `av.animate(animator)` call `at_ms` from now. Clones the `AnimatedValue` handle into the closure so the original binding stays usable for further calls. Returns a `ScheduledTask` that cancels the pending dispatch on drop — hold it (e.g. via `on_cleanup`) to keep the timer alive.",
        expansion: "after_ms(at_ms, move || av.animate(animator)) -> ScheduledTask",
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
        docs: "Declare a typed stylesheet with variants and per-state overrides. `state foo(theme) { … }` arms accept exactly the four framework interaction states (`hovered`, `pressed`, `focused`, `disabled`). See [[styling]] for the grammar.",
        expansion: "",
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
        _seal: (),
    }
}
