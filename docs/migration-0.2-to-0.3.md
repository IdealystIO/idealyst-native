# Migrating from 0.2.x to 0.3

0.3 unifies the reactive authoring surface on **plain functions and
capability-typed handles**. The `signal!` and `memo!` macros are gone
(`signal(value)` / `memo(move || …)` are the canonical — and only — forms),
`memo` now returns a read-only handle, and `Signal<T>` gained
`ReadSignal`/`WriteSignal` capability halves. Component declaration also
gained **inline props** (Leptos-style fn parameters), which is now the
preferred form — the explicit props-struct form still compiles unchanged.

Most of the migration is mechanical (two `sed`s); the one type-level change
that can require thought is `memo`'s new return type.

## What changed, in one paragraph

The rule behind every change: **a macro must do real token work to earn its
`!`**. `signal!` expanded verbatim to `Signal::new` and `memo!` only inserted
`move ||` around a closure you write anyway — so both became plain functions.
`effect!` stays a macro (it wraps a bare *block* in a `move` closure, which a
function cannot accept), as does `rx!` (expression capture). `text_fmt!` and
`bind!` are gone too — reactive text interpolation moved into the text
literal itself (see "Text f-strings" below), which subsumed both. On top of that, signal handles can now be **narrowed by type**:
`.split()` / `.read_only()` / `.write_only()` produce zero-cost
`ReadSignal<T>` / `WriteSignal<T>` views over the same slot, and `memo`
returns `ReadSignal<T>` so a derivation's output is unwritable at compile
time.

## Breaking changes at a glance

| 0.2.x | 0.3 | Failure mode if unmigrated |
| --- | --- | --- |
| `signal!(v)` | `signal(v)` | compile error: cannot find macro `signal` |
| `memo!(expr)` | `memo(move || expr)` | compile error: cannot find macro `memo` |
| `memo(…) -> Signal<T>` | `memo(…) -> ReadSignal<T>` | type mismatch at annotated bindings / `.set()` on the output no longer compiles |
| `memo_with(…) -> Signal<T>` | `-> ReadSignal<T>` | same |
| `current_breakpoint() -> Signal<Breakpoint>` | `-> ReadSignal<Breakpoint>` | type mismatch at annotated bindings only |
| lint rule `prefer-signal-macro` | `prefer-signal-fn` (direction flipped) | unknown-rule warning from `idealyst-lint.toml` |
| lint rule `prefer-memo-macro` | `prefer-memo-fn` (direction flipped) | unknown-rule warning |
| MCP `list_macros` includes `signal`/`memo` | moved to `list_utilities` | agents/tooling querying `describe_macro("signal")` get a miss |

Additive (no action needed): `ReadSignal<T>` / `WriteSignal<T>`,
`Signal::split()` / `.read_only()` / `.write_only()`, the `RwSignal<T>`
porting alias, inline-props `#[component]`, per-arg `#[prop(...)]`, and
`robot::watch_signal` now accepting either a `Signal` or a `ReadSignal`.

Also additive — the hoisted-snapshot guardrails: `.get_untracked()` on
`Signal`/`ReadSignal`/`Reactive` (untracked read with declared intent),
a **debug-build runtime warning** when a `.get()` runs during a component
build with no tracked consumer (the `let ok = x.get()…; if ok {…}` trap —
the warning names the component and suggests the fix), and the
`snapshot-condition` lint rule, which `idealyst dev` now runs ambiently at
startup. If a component *intentionally* snapshots at build time, migrate
that read to `.get_untracked()` to declare it (and silence both
diagnostics).

---

## `signal!` / `memo!` → plain functions

**Before (0.2):**

```rust
let count = signal!(0);
let doubled = memo!(count.get() * 2);
```

**After (0.3):**

```rust
let count = signal(0);
let doubled = memo(move || count.get() * 2);
```

No import changes: `use runtime_core::signal;` (and `memo`) already resolves
to the function — macros and functions share the name across namespaces, which
is also *why* a deprecated macro alias was impossible (the deprecation warning
fires on the import line of every fn-form user).

Mechanical migration for a whole tree:

```bash
# BSD sed (macOS); on Linux drop the '' after -i
git ls-files '*.rs' | xargs grep -l 'signal!(' | xargs sed -i '' 's/signal!(/signal(/g'
git ls-files '*.rs' | xargs grep -l 'memo!('   | xargs sed -i '' 's/memo!(/memo(move || /g'
```

Both replacements are paren-safe: `signal!` and `memo!` each took a single
expression, so the existing closing paren still closes the call, and
`memo(move || <expr>)` parses with the closure body extending to it.

`Signal::new(v)` still compiles (it's what `signal(v)` delegates to) but is
flagged by the linter as the redundant spelling.

## `memo` returns `ReadSignal<T>`

A memo is a pure derivation; 0.2's writable `Signal<T>` return let you
`.set()` a memo's output — the write "worked" until the next dependency
change silently clobbered it. 0.3 closes that hole at the type level.

What this means for existing code:

- **Reads are unchanged.** `.get()`, reading inside `effect!`/closures,
  f-string text slots, `ui!` conditions (`if my_memo { … }`) and keyed loops
  (`for row in my_memo, key = …`) all work identically on `ReadSignal`.
- **Prop passing is unchanged.** A memo output flows into a `Reactive<T>`
  prop exactly as before (`From<ReadSignal<T>> for Reactive<T>` exists).
- **Type annotations need updating**:

  ```rust
  // Before
  let filtered: Signal<Vec<usize>> = memo(move || …);
  // After
  let filtered: ReadSignal<Vec<usize>> = memo(move || …);
  ```

  Same for struct fields and cached `OnceCell`s holding a memo.
- **If you were writing a memo's output — that was the bug this change
  exists to catch.** Either derive the value (make the write a real input
  signal the memo reads), or replace the memo with a plain `signal` you
  manage yourself.

`current_breakpoint()` (and any framework fn documented as "a cached memo")
now returns `ReadSignal` for the same reason.

## New: capability halves

```rust
let (count, set_count) = signal(0).split();  // (ReadSignal<i32>, WriteSignal<i32>)
let view_of = state.read_only();             // observe-only view
let report  = state.write_only();            // write-only view (cannot subscribe)
```

Zero-cost `Copy` newtypes over the same arena slot — identical tracking,
identical generational stale-write no-op. Only the *type* narrows, and there
is deliberately no `Deref` between the halves (deref would hand the other
capability back).

Prop guidance (CLAUDE.md §9.6a): declare the **narrowest half the component
needs**. `ReadSignal<T>` when it only observes — the signature then proves it
can't mutate the caller's state; `WriteSignal<T>` when a child only reports
upward — it can't accidentally subscribe itself; unified `Signal<T>` only for
genuinely two-way props (`TextInput.value`, `Toggle.value`, `Slider.value`).
Apply when touching a component; no repo-wide sweep required — a `Signal<T>`
prop still compiles and behaves as before.

For code ported from Leptos: `let (count, set_count) = signal(0);` becomes
`let (count, set_count) = signal(0).split();`, and `RwSignal::new(v)`
resolves as-is via the `RwSignal<T> = Signal<T>` alias.

## New (preferred): inline component props

The two-declaration form (a `#[props]` struct + `fn Foo(props: &FooProps)`)
still compiles unchanged. 0.3 adds the inline form and makes it the
preferred declaration style:

**Before (0.2, still valid):**

```rust
#[props]
pub struct BadgeProps {
    pub label: String,
    pub count: i32,
}

#[component]
pub fn Badge(props: &BadgeProps) -> Element {
    ui! { text(move || format!("{} ({})", props.label.get(), props.count.get())) }
}
```

**After (0.3, preferred):**

```rust
#[component]
pub fn Badge(label: String, #[prop(default = 3)] count: i32) -> Element {
    ui! { text(move || format!("{} ({})", label.get(), count.get())) }
}
```

The macro generates `BadgeProps` from the parameters — each data param `T`
wrapped `Reactive<T>` by the same rules as `#[props]` (so the body reads
`label.get()`, and call sites may pass a literal, a `Signal`, or `rx!(…)`).
Per-arg attributes: `#[prop(default = expr)]`, `#[prop(static)]`,
`#[prop(reactive)]` (plus `#[prop(optional)]` / `#[prop(into)]` as accepted
no-ops for Leptos parity — every idealyst prop is already optional and
already `.into()`-coerced). A param named `children: Vec<Element>` receives
the call site's `{ … }` block. Doc comments on a param become the prop's
hover docs.

Keep the explicit-struct form when the props need extra derives
(`IdealystSchema`, doc-controls) or a hand-rolled `Default`.

## Lint configuration

If your `idealyst-lint.toml` pins the reactive rules, rename the keys:

```toml
[rules]
# prefer-signal-macro = "error"   # 0.2
prefer-signal-fn = "error"        # 0.3 — flags Signal::new AND leftover signal!
# prefer-memo-macro = "warn"      # 0.2
prefer-memo-fn = "warn"           # 0.3 — flags leftover memo! (memo(…) calls are now clean)
```

Note both rules **flipped direction**: the linter now steers toward the
functions. Leftover `signal!(…)` / `memo!(…)` invocations get a lint
diagnostic with the exact fix (more helpful than rustc's "cannot find
macro"). Inline `// idealyst-lint-disable` directives naming the old rule
ids should be renamed the same way.

## MCP catalog consumers

`signal` and `memo` moved from the authoring-macros table to the utilities
table (`list_utilities` / `describe_utility`, category `reactive`).
`list_macros` no longer returns them, and `describe_macro("signal")` misses.
Agents and tooling that hard-code the macro list should query utilities for
signal creation and memoization.

## Text f-strings (replaces `text_fmt!` / `bind!`)

String literals in text position now interpolate `{name}` placeholders
like Rust's own `format!` inline args — and the slots are live or
static by the value's TYPE (the text analog of `if is_high`):

```rust
// 0.2 — closure form
text(move || format!("count: {}   doubled: {}", count.get(), doubled.get()))
// 0.2 — web-optimized binding form
text { text_fmt!("count: {}", bind!(count)) }

// 0.3 — one form, optimized automatically
text { "count: {count}   doubled: {doubled}" }
```

Signals and memo outputs interpolate live and produce the same
`TextSource::JsBinding` fast path `text_fmt!` did (web updates without a
wasm round-trip; Effect fallback elsewhere); `Display` values bake in
statically; `Reactive<T>` props work either way. Format specs pass
through (`{r:.2}` — a capability `text_fmt!` never had); `{{` escapes.
Existing literals are safe: interpolation only activates when a literal
contains a valid `{ident}` placeholder — brace-containing prose and
`{{`-escaped strings keep their exact 0.2 meaning, and positional
`{}`/`{0}` or `{x:?}` mixed into an interpolating literal are compile
errors (use the closure form for those).

**BREAKING: `text_fmt!` and `bind!` are removed** (same doctrine as
`signal!`/`memo!` — the sentinel marked reactivity that the type system
already knows). Migration is mechanical:

```rust
// before
text { text_fmt!("leaf {}: g={}", id, bind!(global)) }
// after — name the args inline; classification is by type
text { "leaf {id}: g={global}" }

// non-ident args need a let first:
// before: text { text_fmt!("row: c={}", bind!(sigs[i])) }
let sig = sigs[i];
// after
text { "row: c={sig}" }

// prop positions (label = text_fmt!(…)) with only captured values:
// use plain format!; with signals: rx!(format!(…)) or pass the signal.
```

The `prefer-text-fstring` lint flags leftover invocations with the
rewrite; `JsBindingSpec` remains public for hand-constructed bindings.

## New: editor & tooling (all additive)

Nothing here requires migration; it's what 0.3 adds to the editing
experience.

- **`ui!`/`jsx!` IDE recovery got real.** The macros' error-recovery
  expansion now salvages far more of a mid-typing block, so
  rust-analyzer keeps working while you type: a half-typed prop
  (`Counter(sta|)`) is re-emitted in struct-literal position so RA
  completes prop NAMES; one broken child no longer takes its siblings'
  type info down with it; the conditions of `if`/`for`/`match`
  headers stay analyzable even when their ui!-flavored bodies aren't
  valid Rust; and `if let` / closure / `let`-statement bindings survive
  into the salvage, so dot-completion on a handle mid-handler
  (`c.|` inside `if let Some(c) = counter.get() { … }`) offers the
  handle's real methods. Under rust-analyzer, valid expansions carry
  the same salvage copy so completion stays aligned no matter where in
  the block the cursor is; real `rustc` builds never see any of this.
  Original token spans are preserved throughout.
- **`idealyst catalog-json [DIR]`** — new CLI command printing the
  project's full catalog JSON to stdout (components, props schemas with
  docs, primitives, guides). The stable machine-facing entry for editor
  tooling and CI; same wrapper pipeline as `idealyst mcp`, so the first
  run compiles the catalog wrapper and later runs are cached.
- **VS Code extension** (`editors/vscode-idealyst/`) — DSL-vocabulary
  completion inside `ui!`/`jsx!`: tag completion (all primitives +
  components with docs) and per-tag prop completion (names, types, doc
  comments), fed by `catalog-json`. Dependency-free plain JS; install by
  symlinking the folder into `~/.vscode/extensions` (see its README).
  Complements rust-analyzer rather than replacing it: RA owns types and
  expressions, the extension owns the vocabulary.
- **rust-analyzer project wiring** (per-project `.vscode/settings.json`,
  not yet scaffolded automatically): run lint inline by overriding the
  check command with a script that emits `cargo check --message-format=json`
  plus `idealyst lint --format json "$PWD"` (invoke the script as
  `["sh", ".vscode/ra-check.sh"]` — RA does not reliably expand
  `${workspaceFolder}` in `check.overrideCommand`, and lint paths must be
  absolute). Also disable RA's built-in case diagnostic
  (`"rust-analyzer.diagnostics.disabled": ["non_snake_case", "incorrect-case"]`):
  it can't see the `#[allow(non_snake_case)]` that `#[component]` injects
  for PascalCase component fns, while `cargo check` on save honors it —
  so components stay exempt and real case mistakes still surface.

## Not changed

- `effect!`, `rx!`, `children!`, `node_ref!`,
  `animated!`, `stylesheet!`, `ui!` / `jsx!` — unchanged (the recovery
  work above changes only what the IDE sees on BROKEN input; valid-input
  expansion is byte-identical).
- Reactive semantics — synchronous fan-out, `batch`, generational
  stale-write no-ops, dispose-on-hide — unchanged. The capability halves are
  views, not a new reactive system.
- `watch_signal` call sites passing a `Signal` — unchanged (the parameter
  widened to accept `ReadSignal` too).
