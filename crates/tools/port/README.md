# `port/` — source porters

Tools that translate UI code from another framework into idealyst-native Rust.
Each frontend is its own sub-crate; they share IR + emitter + hole machinery
via `port-core`. The output is the same idealyst Rust regardless of which
source framework you started from.

| Crate | Source language | Status |
| --- | --- | --- |
| [`port-core`](./core) | — (shared IR, emitter, CLI, hole machinery) | Done |
| [`port-tsx`](./tsx) | Shared swc-backed TSX parser + lifter trait for React + Solid | Done |
| [`port-react`](./react) | TypeScript + React `.tsx` (real swc parser) | Done |
| [`port-solid`](./solid) | Solid `.tsx` (real swc parser) | Done |
| [`port-vue`](./vue) | Vue SFC `.vue` (real SFC split + swc on script + handrolled template walker) | Done |
| [`port-svelte`](./svelte) | Svelte SFC `.svelte` (real block split + swc on script + handrolled markup walker) | Done |
| [`port-project`](./project) | Project-level driver: walks a dir or clones a git URL, ports every file, writes Rust output + markdown report | Done |
| [`port-preview`](./preview) | Port + scaffold a scratch Cargo crate + run `cargo check` to verify the output compiles. Surfaces type-check defects the porter alone can't see. | Done |

## Design philosophy

Three deliberate splits make porting actually shippable rather than a research
project:

### 1. Mechanical compiler + AI final pass

The porter does the part that has to be exact and verifiable: **structure
and reactivity**. Framework primitives → idealyst signals/effects, template
syntax → `jsx!`, TS types → Rust types where they map cleanly.

The porter **does not** translate business logic, third-party calls, or
unknown framework primitives. Those become explicit holes
(`todo!("port handler-body: …")`) with the original source snippet embedded
inline. An LLM final pass — or a human — fills the holes with the
surrounding Rust already in place and type-checking.

This split is what makes the tool tractable. The compiler emits a
deterministic skeleton; the AI fills small, well-scoped holes with rich
local context; reviewers audit each hole individually.

### 2. Framework-agnostic IR + emitter, per-framework frontend

```text
   .tsx / .vue / .svelte
       │
       ▼
   ┌──────────────────────────────┐
   │  per-framework frontend       │   port-react / port-solid /
   │  (Parser + reactive taxonomy) │   port-vue / port-svelte
   └──────────────────────────────┘
       │
       ▼ port_core::ir::Module + PortReport
   ┌──────────────────────────────┐
   │  port-core: IR + emitter      │   framework-agnostic
   └──────────────────────────────┘
       │
       ▼
   idealyst Rust
```

The IR (`Module`, `Component`, `Reactive`, `JsxNode`, `Hole`) and the
emitter live in `port-core`. They have no knowledge of which source
framework a module came from — only that "a component has props, some
reactive declarations, and a returned UI tree."

Adding a new frontend is: (a) implement `Parser` against your source
language, (b) declare a reactive-primitive taxonomy, (c) emit IR. The
emitter and hole machinery come for free.

### 3. Reactive taxonomy as a first-class coverage promise

Each porter ships an explicit table (e.g.
[`port-react/src/hooks.rs`](./react/src/hooks.rs),
[`port-vue/src/composition.rs`](./vue/src/composition.rs),
[`port-svelte/src/reactivity.rs`](./svelte/src/reactivity.rs)) classifying
each source-language call shape:

- **Mechanical** — call maps to an idealyst primitive
  (`useState`/`createSignal`/`ref` → `signal!`,
  `useEffect`/`createEffect`/`watchEffect` → `effect!`).
- **Unknown** — everything else. Ported as a plain function call.

That's the entire surface. There is no "Shimmed" tier — the porter does
not invent a React-flavored runtime layer on top of idealyst.

### Hooks port as plain Rust functions

Custom hooks are not a framework concept in idealyst. They're just
function composition. `function useToggle(initial) { … }` ports to a
regular Rust function. `const x = useThing(spec)` ports to
`let x = useThing(spec);` in the output. If a Rust `fn useThing` exists
(user-written, or hand-ported from the source's hook body), the
generated code links against it; otherwise the build fails with a clear
name resolution error pointing at the spot to address.

This is what lets the porter remain framework-agnostic. As idealyst
designs its own answers for context, refs, reducer-style state, etc., the
relevant call shapes graduate from `Unknown` to `Mechanical` with an
explicit mapping. Until then, they stay generic function calls.

## Counter, four ways

The same logical component, expressed in four different source languages,
ports to the same idealyst Rust.

### Input — React (`port-react/fixtures/counter.tsx`)

```tsx
import { useState, useEffect } from "react";

interface CounterProps { initial?: number; }

export function Counter({ initial = 0 }: CounterProps) {
    const [count, setCount] = useState(initial);
    useEffect(() => {
        console.log("count:", count);
    }, [count]);
    return (
        <View>
            <Text>Count: {count}</Text>
            <Button label="Inc" onClick={() => setCount(count + 1)} />
        </View>
    );
}
```

### Input — Solid (`port-solid/fixtures/counter.tsx`)

```tsx
import { createSignal, createEffect } from "solid-js";

interface CounterProps { initial?: number; }

export function Counter(props: CounterProps) {
    const [count, setCount] = createSignal(props.initial ?? 0);
    createEffect(() => {
        console.log("count:", count());
    });
    return (
        <View>
            <Text>Count: {count()}</Text>
            <Button label="Inc" onClick={() => setCount(count() + 1)} />
        </View>
    );
}
```

### Input — Vue (`port-vue/fixtures/counter.vue`)

```vue
<script setup lang="ts">
import { ref, watchEffect, withDefaults } from 'vue';

interface CounterProps { initial?: number; }
const props = withDefaults(defineProps<CounterProps>(), { initial: 0 });

const count = ref(props.initial);
watchEffect(() => {
    console.log('count:', count.value);
});
function increment() { count.value++; }
</script>

<template>
    <View>
        <Text>Count: {{ count }}</Text>
        <Button label="Inc" @click="increment" />
    </View>
</template>
```

### Input — Svelte (`port-svelte/fixtures/counter.svelte`)

```svelte
<script lang="ts">
    export let initial = 0;
    let count = initial;

    $: console.log('count:', count);

    function increment() {
        count = count + 1;
    }
</script>

<View>
    <Text>Count: {count}</Text>
    <Button label="Inc" on:click={increment} />
</View>
```

### Output — same idealyst Rust for all four

```rust
#[derive(Default)]
pub struct CounterProps {
    pub initial: i32,
}

#[component(default(initial = 0))]
pub fn counter(props: &CounterProps) -> Element {
    let count = signal!(props.initial);
    effect!({
        todo!("port handler-body (line N): console.log → idiomatic Rust logging — <original>");
    });
    jsx! {
        <View>
            <Text>{format!("Count: {}", count.get())}</Text>
            <Button label="Inc" on_click={move || count.set(count.get() + 1)} />
        </View>
    }
}
```

Each porter's hole carries the *original snippet from its source language*,
so the AI final pass sees both the surrounding Rust and the exact JS / Vue
expression / Svelte reactive statement it needs to translate.

## What the porters do *not* do (yet)

- **No real parser.** Each frontend ships a `StubParser` that hand-lowers
  the bundled fixture. Real parsers are the next step:
  - `port-react` / `port-solid`: swc_ecma_parser or oxc_parser.
  - `port-vue`: vue-sfc parser → swc for the script block, html5ever
    (or similar) for the template.
  - `port-svelte`: there's no Rust Svelte parser; either bind to the
    JS one or write a focused one (Svelte's template grammar is small).
- **No React Compiler / Svelte compiler validation upstream.** The plan
  for `port-react` is to require source files pass React Compiler
  validation before porting; the analogous gate exists for each frontend
  (Svelte's own compiler, Vue's template compiler).
- **No npm ecosystem coverage.** Calls to `lodash`, `axios`,
  `react-query`, Pinia stores, RxJS subjects all become handler-body
  holes for the AI pass. A `port-shim-registry` may map common ones.
- **No CSS / styled-components / Tailwind translation.** That's a
  separate `port-css` pass. Vue's `<style>` block is currently lowered
  as an `Unsupported` hole pending that work.
- **No class components, decorators, or Vue Options API.** Top-level
  holes (`HoleKind::Unsupported`).

## Running

### Per-file

```bash
cargo run -p port-react   -- crates/port/react/fixtures/counter.tsx
cargo run -p port-solid   -- crates/port/solid/fixtures/counter.tsx
cargo run -p port-vue     -- crates/port/vue/fixtures/counter.vue
cargo run -p port-svelte  -- crates/port/svelte/fixtures/counter.svelte
```

Each prints the rendered Rust to stdout and a hole summary to stderr.
Add `-o path.rs` to write to disk.

### Whole project

```bash
# Local directory
cargo run -p port-project -- path/to/my-react-app -o /tmp/ported

# GitHub URL (shells to `git clone --depth 1` into a temp dir)
cargo run -p port-project -- https://github.com/pmndrs/leva -o /tmp/leva-port
```

Walks the tree, routes each `.tsx`/`.jsx`/`.vue`/`.svelte` file to the right
frontend (`.tsx` with a `solid-js` import → port-solid; otherwise port-react),
writes Rust to a mirrored output tree with snake-cased filenames, and produces
`<output>/REPORT.md` with:

- top-level counts (ok / error / skipped),
- by-frontend breakdown,
- hole histogram by kind,
- top recurring hole reasons (which JS idioms are biting most),
- per-file table.

Skipped = the file parsed but had no exported components (entry points,
type-only modules, re-exports). Error = the porter actually failed.

Plain `.ts` / `.d.ts` / `.mts` / `.cts` files are scanned for type
definitions (`interface X { … }` and `type X = { … }`) to seed a
project-wide registry, then *not* emitted. That registry is what lets
`createContext<X>(...)` aliases reference cross-file types and still
produce a usable Rust struct declaration in the consuming file.

### Compile-check the output

```bash
# Port + scaffold a scratch Cargo crate + run `cargo check`:
cargo run -p port-preview -- path/to/my-react-app -o /tmp/preview
cargo run -p port-preview -- https://github.com/pmndrs/leva  -o /tmp/leva-preview
```

`port-preview` runs `port-project` into the scratch dir, generates
`Cargo.toml` + `src/main.rs` + a recursive `mod.rs` tree under
`src/ported/` so the ported files form valid Rust modules, then runs
`cargo check` against the scratch crate. Output summary:

```
  scratch crate: /tmp/preview-theme
  port: 1 files · 1 ok · 0 error · 0 skipped · 0 holes
  build: FAILED (2 compiler errors)
  --- cargo check stderr (first 30 lines) ---
    error[E0609]: no field `accent` on type `Option<Theme>`
      …
    error[E0507]: cannot move out of `props.accent` …
      …
```

This is the "did the port actually produce coherent Rust?" check. Even
fixtures with 0 holes can have real defects (`inject::<T>()` returns
`Option<T>` but the porter generated bare field access; non-`Copy`
struct fields moved from a shared reference; etc.) — preview is what
surfaces them. Each error is concrete, locatable, and points at a
specific porter improvement.

The exit code distinguishes: `0` = build ok, `3` = build failed,
`4` = `cargo` couldn't even run. The scratch dir is left on disk
for inspection.

## Real-world reference point

Running `port-project` against [pmndrs/leva](https://github.com/pmndrs/leva)
(a real third-party TSX-only UI library, ~97 source files):

```
97 files · 70 ok · 0 error · 27 skipped · 87 holes total
```

(Cross-file type resolution lights up here: `leva/src/context.tsx`
references `StoreType` and `PanelSettingsType` from `./types/internal.ts`;
the project driver pre-scans all `.ts` files for type definitions, builds
a project-wide registry, and emits full `pub struct StoreContext { … }` /
`pub struct PanelSettingsContext { … }` declarations with all their
fields and function-typed callbacks.)

The 27 skipped are app-bootstrap files with no exported components. The 70
"ok" all produced readable Rust with explicit `todo!()` holes where the porter
refused to guess. Top recurring holes:

```
 55× unsupported        — mostly object-destructure on unknown calls
                          (e.g. `const { x, y } = useControls(spec)`),
                          plus rest destructures and unrecognized stmts
 18× attribute-value    — JSX spread attrs `{...props}` (emitted as
                          `_spread={todo!()}` markers, visible at compile time)
 15× handler-body       — multi-statement effect setup / cleanup bodies
                          (11 setup, 4 cleanup; 4 effects in leva use
                          `return cleanup` and the porter splits each
                          into a setup hole + `on_cleanup(...)` cleanup)
  2× prop-type          — TS types that don't map cleanly to Rust
```

Custom hook calls (`useControls`, `useInputContext`, ...) used to drive
~164 holes here; they now port as plain function calls (`let x =
useControls(spec);`) and disappear from the hole histogram entirely.

### What the porter handles mechanically

The most common shapes in real React code are all covered:

| Source shape | Translation |
| --- | --- |
| `export function Foo()` | `pub fn foo` |
| `export default function Foo()` | `pub fn foo` |
| `export const Foo = (…) => …` | `pub fn foo` |
| `export default (…) => …` | `pub fn default_` (synthesized name) |
| `interface Props { x: number, on: (id: string) => void }` | `pub struct FooProps { pub x: i32, pub on: Option<Box<dyn Fn(String) + Send + Sync>> }` |
| `({ x, onClick })` (no interface) | Untyped fields harvested; `on*` keys typed as `Option<Box<dyn Fn() + Send + Sync>>` |
| `useState(x)` / `createSignal(x)` / `ref(x)` / Svelte reactive `let` | `signal!(x)` |
| `useEffect(fn, deps)` / `createEffect(fn)` / `watchEffect(fn)` / Svelte `$:` | `effect!({ … })` |
| `useEffect(() => { setup; return () => cleanup; })` | `effect!({ setup; on_cleanup(move \|\| cleanup); })` |
| Solid `onCleanup(fn)` inside `createEffect` | `on_cleanup(move \|\| fn_body);` inside `effect!({…})` |
| Vue `watchEffect((onCleanup) => { … onCleanup(fn) })` | Same — `onCleanup` call recognized regardless of frontend |
| `count` (state read) | `count.get()` |
| `setCount(x + 1)` | `count.set(x + 1)` |
| `cond ? a : b` (attr value or JSX child, JSX branches included) | `if cond { a } else { b }` — JSX branches lower to recursive `jsx! {}` |
| `e.target.value` (nested member chains) | `e.target.value` |
| `props.onToggle(id)` / `onToggle(id)` (destructured) | `props.on_toggle(props.id)` (args preserved, prop-qualified) |
| `<Foo {...spread} />` | `<Foo _spread={todo!("port …")} />` — visible compile-time TODO |
| `useContext(Ctx)` / Solid `useContext(Ctx)` | `let value = inject::<Ctx>();` — first arg's identifier becomes the Rust type |
| `<Ctx.Provider value={{ a, b: c }}>children</Ctx.Provider>` | `{ provide(Ctx { a: props.a, b: c }); jsx! { children… } }` — object-literal values render as struct construction using the context type |
| `const Ctx = createContext<Shape>(default)` (top level) | `#[derive(Default, Clone)] pub struct Ctx { … }` — passthrough struct declaration with `Shape`'s fields, named after the alias. The `inject::<Ctx>()` and `provide(Ctx { … })` references then resolve. |
| `const Ctx = createContext<Shape \| null>(null)` (nullable union) | Same as above — null/undefined arms peeled. Multi-arm unions stay as a `PropType` hole. |
| `interface X { … }` *or* `type X = { … }` referenced cross-file | When run via `port-project`, types defined in any `.ts`, `.tsx`, `.vue`, or `.svelte` file in the tree contribute to a project-wide registry. `createContext<X>` aliases resolve against this registry even when `X` is imported from another file. |
| `useThing(args)` (any custom hook) | `let result = useThing(args);` — plain function call |

### What still becomes a hole

- Custom or third-party hooks (route to AI pass; a registry override
  would let users teach the porter about `useControls`-style helpers).
- Multi-statement effect/handler bodies (the AI pass is the natural
  destination — JS imperative code rarely maps mechanically).
- Rest destructures `...rest` in props.
- Top-level statements other than `let`/`expr`/`return` (try/throw/if etc).

This is the canonical reference point: the porter handles ~70/97 of a real
TSX library out of the box, with an actionable next-fix list driven by
real-world frequency rather than guesswork.

## Roadmap

| Step | Status |
| --- | --- |
| `port-core` shared IR / emitter / CLI / hole machinery | Done |
| Four frontends with stub parsers + snapshot tests | Done |
| swc/oxc-backed `Parser` for `.tsx` (React + Solid) | TODO |
| `vue-sfc` + script/template parser for `.vue` | TODO |
| Svelte template parser (hand-rolled or JS bridge) for `.svelte` | TODO |
| React Compiler validation gate upstream of `port-react` | TODO |
| `port-shim` runtime crate (`use_context`, `use_reducer`, …) | TODO |
| `port-css` for CSS / styled-components / Tailwind / Vue scoped styles | TODO |
| Runtime diff harness (original JS app vs. ported, diff wire output) | TODO |
