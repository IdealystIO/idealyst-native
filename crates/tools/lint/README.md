# `lint` — the idealyst source linter

Flags idiom-drift patterns in idealyst projects, over the project's
**un-expanded** Rust source:

| Rule | Default | Flags | Use instead |
|------|---------|-------|-------------|
| `prefer-signal-fn` | warn | `Signal::new(v)`, removed `signal!(v)` | `signal(v)` |
| `prefer-effect-macro` | warn | `Effect::new(\|\| …)` | `effect! { … }` |
| `prefer-memo-fn` | warn | removed `memo!(…)` | `memo(move \|\| …)` |
| `prefer-text-fstring` | warn | removed `text_fmt!(…)` / `bind!(…)` | `text { "count: {count}" }` |
| `prefer-ui-macro` | warn | a primitive constructor called by hand — `runtime_core::view(…)`, `glue::text(…)`, `builders::view()`, or a bare `view(…)` the file imports from the framework — plus `BuildElement::build(…)` and `Element::View { … }`. A builder-PATTERN constructor (`Pool::builder()`, `reqwest::Client::builder()`) is not the framework's builder layer and is never flagged. Only constructors that HAVE a `ui!` tag are flagged: `pressable`, `text_area` and the builders `swap_navigator` / `stack_navigator` / `navigator_outlet` / `portal` / `virtualizer` / `virtual_grid` have no `ui!` spelling, so calling them is the author surface | `ui! { … }` / `jsx! { … }` |
| `prefer-ui-control-flow` | warn | a hand call to the reactive branch glue — `runtime_core::when(…)` / `switch(…)` (qualified, inside `vec![…]`, or a bare `when(…)` the file imports) | `if cond.get() { … }` / `match key.get() { … }` inside `ui!`. For the two shapes the macro can't express (a static-prop fast path, a keyed rebuild of one shape — see below), suppress with the reason |
| `component-pascal-case` | error | `#[component] fn icon_button` | `#[component] fn IconButton` |
| `prefer-component` | warn | a free fn that composes a tree (`ui!` / `jsx!` or a primitive constructor) and returns `Element` without `#[component]` — any such fn with params (incl. a hand-rolled `fn Card(props: &CardProps)`), or a zero-arg one called from 2+ sites. Exempt: fns used as values (`app` entry, screens, render callbacks), a zero-arg one-off helper (CLAUDE.md §9.5), methods, tests, and a non-idealyst `Element` (`web_sys::Element`) | `#[component] fn UserRow(name: String, active: bool) -> Element`, called as `ui! { UserRow(name = …, active = true) }` |
| `snapshot-condition` | warn | hoisted `let ok = x.get()…;` used as a `ui!` `if` condition or `match` scrutinee — a bare plain-value binding is the one condition shape `ui!` lowers statically | `memo(move \|\| …)`, inline the `.get()`, or `.peek()` if intentional (`.get_untracked()` on a `Reactive<T>` prop) |
| `prefer-keyed-list` | warn | a child list built by hand — `VEC.push(ui! { … })` / `ITER.map(\|x\| ui! { … })` — outside the macro. The map counts only over a visible iterator (`.iter()` / `.into_iter()` / `.values()` / `.enumerate()` / a range / …) or when its result is `.collect()`ed; `Option::map(\|x\| ui! { … })` (an optional child) is never flagged | `ui! { view() { for item in items, key = item.id { … } } }` |
| `snapshot-loop` | warn | `for item in items.get()` inside a `ui!` / `jsx!` body — a frozen build-time snapshot | `for item in items, key = item.id { … }` (iterate the Signal itself) |
| `signal-across-await` | warn | a scope-owned signal touched inside a detached `spawn_async` (or the future half of `spawn_then` / `spawn_then_in`). "Scope-owned": a `signal(…)` / `memo(…)` created in a `#[component]` body (incl. one held in a struct field, `form.v`), or — in any fn — a `Signal` / `ReadSignal` / `WriteSignal` / `Memo` parameter or same-file props-struct field (the parent owns it). "Touched": read or written directly, through a local closure / `Rc<dyn Fn>` that touches one, or by calling a callback parameter / props field (`on_done(v)`, `(props.on_done)(v)`) whose caller's closure can't be seen. After an `.await` the scope can die at the flush boundary; **before** the first `.await` (or with none) it can too on web, where the task's first poll is queued behind the spawning event's flush | `spawn_then(future, \|result\| { … })` — the callback runs inside a turn or not at all; for the pre-await part, do it before calling `spawn_async`; also `resource(deps, fetcher)` / `mutation(handler)`, or hoist the signal so it is root-owned |
| `spawn-then-handler-anchor` | warn | a closure that writes a component signal and calls a bare `spawn_then`, where that signal shapes the same fn's `ui!` tree (a `loading =` / `disabled =` prop on a non-`Button` tag, or an `if` / `match` condition) and the closure isn't wrapped by `ScopeAlive::wrap*` / `run_within`. The task anchors to the handler's control; the write rebuilds it and the callback is silently dropped (spinner forever) | `let alive = ScopeAlive::current();` in the component body, then `spawn_then_in(&alive, future, \|result\| { … })` (or `alive.wrap0(handler)`). idea-ui's `Button` already re-anchors its `on_click`, so its `loading` prop is not evidence |

> **Why un-expanded source?** After macro expansion, `signal(0)` *is*
> `Signal::new(0)` and `ui! { … }` *is* `BuildElement::build(…)` — the idiom
> choice has vanished. A clippy/rust-analyzer lint pass runs post-expansion
> and can't see it. This linter parses with `syn` and walks the tree before
> expansion, which is the only place the question "did the author use the
> macro?" still has an answer. As a bonus, `syn` never descends into macro
> token streams, so anything *inside* `ui! { … }` / `signal( … )` is
> invisible — legitimate macro use is never flagged. (Rules that *do* need to
> see inside — `snapshot-condition`, `snapshot-loop` — deliberately tokenize
> the visible `ui!` / `jsx!` invocation bodies and scan lexically, and
> `signal-across-await` re-parses `effect! { … }` bodies as real blocks,
> because the mount-time-load idiom puts the `spawn_async` in there. `vec![…]`
> is re-parsed too — it's plain expressions, and it's where a hand-built
> child list lives.)

### `prefer-ui-control-flow` — the two legitimate direct calls

`ui!` lowers `if` / `match` to `when` / `switch` itself, choosing static vs
reactive and supplying an out-of-flow placeholder for a missing branch, so a
hand call is almost always an `if` / `match` written outside the tree. Two
shapes can't be spelled in the macro and keep the direct call, each with a
reasoned suppression:

- **static-prop fast path** — the branch is picked by a value *derived* from
  a `Reactive<T>` prop (`src.is_some()`), and a static prop must build its
  branch directly with no reactive hole. The macro's type-driven static
  dispatch only covers a bare `Reactive<bool>` / `Signal<bool>` path.
- **keyed rebuild of one shape** — one subtree rebuilt whenever a derived
  key changes (a pager row on `(page, total)`). The `ui!` form would be a
  single-arm `match`, the same call in disguise.

```rust
// idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape
runtime_core::switch(move || (page.get(), total.get()), move |&(p, t)| row(p, t))
```

A choice between *different* shapes is always a `ui!` `match`, even over a
tuple key. Framework tests that exist to pin the glue itself (a fixture whose
point is the element variant its component returns, a builder-layer test
file with no `ui!`) suppress the same way, naming that as the reason. `crates/ui/idea-ui/src/components/mod.rs` documents both cases.

### `signal-across-await` — known false positives

A **root** component (`#[component] fn app()`) never unmounts, so its
signals outlive every task — but the rule cannot tell a root from a screen.
Suppress at the top of an app-root file:

```rust
// idealyst-lint-disable-file signal-across-await
```

A callback parameter called from a task is reported at the call, because
the callee cannot see what the caller's closure writes. If every caller
passes a root-owned closure, suppress that line with the reason; otherwise
route the callback through `spawn_then(future, move |r| on_done(r))`, which
runs it only while the calling scope is alive.

Why the pre-await prelude is reported: on web, `spawn_async` hands the
future to web-glue's executor, whose drain loop runs every queued
microtask — and the dispatch's flush is one — before it polls any task. So
`busy.set(true); fetch().await;` runs *after* the flush of the event that
spawned it. Apple and Android poll the first segment synchronously inside
`spawn`, so there it runs in the caller's turn; the rule reports the web
behaviour, which is the one that aborts. Doing the synchronous part before
calling `spawn_async` is correct everywhere.

## CLI

```sh
idealyst lint                  # lint ./ , human report
idealyst lint crates/ui        # lint a subtree
idealyst lint --rules          # list rules + default levels
idealyst lint --deny-warnings  # CI strict mode: warnings fail the exit code
idealyst lint --format json    # cargo-style JSON (for editors / CI tools)
```

Exit status mirrors `cargo check`: non-zero when any **error**-level
diagnostic fires (or a file fails to parse), or any warning under
`--deny-warnings`.

## Configuration — `idealyst-lint.toml`

Discovered by walking up from the lint target. Every rule is individually
settable to `off` / `warn` / `error` (the ESLint model):

```toml
# idealyst-lint.toml
[rules]
component-pascal-case = "error"   # keep the hard line on naming
prefer-signal-fn      = "warn"
prefer-effect-macro   = "warn"
snapshot-loop         = "warn"
prefer-ui-macro       = "off"     # e.g. a crate that hand-builds elements
```

### Inline suppression

```rust
// Whole file:
// idealyst-lint-disable-file
// Whole file, one rule:
// idealyst-lint-disable-file prefer-signal-fn

// Next line, all rules:
// idealyst-lint-disable-next-line
let s = Signal::new(0);

// Same line, specific rules (comma- or space-separated):
let s = Signal::new(0); // idealyst-lint-disable-line prefer-signal-fn

// With a reason — everything after ` -- ` is prose, never rule ids:
// idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape
```

A directive with no rule ids after it suppresses **all** rules on its target
line/file. When a suppression marks a deliberate exception rather than a
false positive, give the reason after ` -- ` so the next reader learns why
the rule doesn't apply.

## rust-analyzer integration (inline editor squiggles)

rust-analyzer has no lint-plugin API, but its flycheck runs an arbitrary
command and renders the cargo-JSON diagnostics it prints. Point it at
`idealyst lint --format json` and the lint findings show up as squiggles
next to `cargo check`'s.

`.vscode/settings.json` (or the equivalent RA client setting):

```jsonc
{
  // Run BOTH cargo check and the idealyst linter. RA merges the
  // diagnostics from each line of JSON the command prints.
  "rust-analyzer.check.overrideCommand": [
    "idealyst", "lint", "--format", "json", "."
  ]
}
```

To keep `cargo check`'s type errors *and* add lint diagnostics, run a small
wrapper script that emits both streams' JSON, or use
`rust-analyzer.check.extraArgs` strategies per your client — see the
"Combining with cargo check" note in the framework lint guide.

The JSON is the `cargo check --message-format=json` shape: one
`{"reason":"compiler-message", …}` per finding plus a trailing
`{"reason":"build-finished", …}`. Diagnostic codes are `idealyst::<rule>`
(e.g. `idealyst::prefer-signal-fn`), so they're easy to filter.

## The hard-stop companion: `strict-naming`

`component-pascal-case` is a *lint* (warns/errors in the linter). For a
build that must never compile a misnamed component, turn on the
`strict-naming` Cargo feature (forwarded `runtime-core/strict-naming` →
`runtime-macros/strict-naming`): `#[component]` then emits a
`compile_error!` on any non-PascalCase fn name. The lint catches it while
you type; the feature stops the build. Use the feature in CI, the lint
everywhere.
