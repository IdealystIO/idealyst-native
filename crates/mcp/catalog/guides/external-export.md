+++
title = "Exporting to front-end frameworks"
order = 85
tags = ["export", "web-components", "react", "vue", "svelte", "angular", "interop", "wasm", "deployment"]
+++

# Exporting components to React, Vue, Svelte, Angular & vanilla JS

`idealyst export` turns a project's components into a **framework-agnostic Web
Component suite**: each exported component becomes a wasm-backed
[custom element](https://developer.mozilla.org/docs/Web/API/Web_components)
that works unchanged in React, Vue, Svelte, Angular, Solid, Lit, or a plain
HTML page — plus generated `.d.ts` declarations and typed per-framework
wrappers. The author writes **no JavaScript and no bridge code**.

This is the outbound counterpart to the rest of the framework: instead of
shipping a whole Idealyst app, you publish individual components for a
foreign codebase to consume.

## 1. Tag a component

Add `external` to the `#[component]` attribute and derive
`IdealystSchema` on its props (the derive is what publishes the prop
names + types the generator reads):

```rust
use runtime_core::{component, text, ui, Element, IdealystSchema, Reactive};
use std::rc::Rc;

#[derive(Default, IdealystSchema)]
pub struct GreeterProps {
    /// Who to greet.
    pub name: Reactive<String>,
    /// Fired when the Greet button is pressed.
    pub on_greet: Option<Rc<dyn Fn()>>,
}

#[component(external)]
pub fn Greeter(props: &GreeterProps) -> Element {
    let name = props.name.clone();
    let on_greet = props.on_greet.clone();
    ui! {
        view {
            text(move || format!("Hello, {}!", name.get()))
            button("Greet".to_string(), move || {
                if let Some(cb) = &on_greet { cb(); }
            })
        }
    }
}
```

`#[component(external)]` accepts an optional tag override:
`#[component(external(tag = "my-greeter"))]`. The default tag is
`idl-<kebab-case-name>` (here, `idl-greeter`). Doc comments on the props
flow through to the generated TypeScript types.

The component is ordinary platform-agnostic code — it also renders natively
on every [[backends|backend]] and can be styled the normal way (see
[[styling]]); the styling renders identically in every framework consumer
because it's the *same* compiled component.

## 2. Export

```bash
idealyst export                          # current directory
idealyst export path/to/project --release
idealyst export --frameworks react,svelte   # subset; default is all
```

Output lands in `<project>/dist/external/`. **Each framework gets its own
self-contained folder** — its own copy of the wasm `pkg/` and the custom
elements, so a consumer pulls in exactly one folder and nothing reaches
across them. The universal (vanilla / any-framework) layer lives in `web/`;
`web` is reserved for it and never mixed with the framework wrappers:

```
dist/external/
├── package.json                  # umbrella package (main → web/index.js)
├── README.md                     # usage for every framework
├── web/                          # universal layer — works in any framework
│   ├── pkg/                      #   wasm + wasm-bindgen JS glue
│   ├── idl-greeter.js            #   the custom element (self-registers on import)
│   ├── idl-greeter.d.ts          #   TypeScript declarations
│   ├── Greeter.js                #   imperative create*/bind* helpers
│   ├── index.js                  #   barrel — imports every element
│   ├── index.d.ts                #   barrel types
│   └── index.html                #   a vanilla demo page
├── react/                        # self-contained React package
│   ├── pkg/                      #   its own copy of the wasm
│   ├── idl-greeter.js            #   its own custom element
│   ├── idl-greeter.d.ts
│   └── Greeter.tsx               #   typed React wrapper (imports ./idl-greeter.js)
├── vue/      pkg/ · idl-greeter.js · Greeter.js
├── svelte/   pkg/ · idl-greeter.js · Greeter.svelte
└── angular/  pkg/ · idl-greeter.js · greeter.component.ts
```

Because each folder is independent, the wasm is duplicated across them — the
trade-off that buys "drop one folder into a foreign project" portability.

## 3. Consume it

The bare custom element works in **any** framework. The per-framework
folders are typed convenience wrappers on top.

**Vanilla / any framework that renders DOM:**

```html
<script type="module" src="./dist/external/web/index.js"></script>
<idl-greeter name="World"></idl-greeter>
<script type="module">
  const el = document.querySelector("idl-greeter");
  el.name = "Idealyst";                 // reactive property — re-renders, no remount
  el.addEventListener("greet", () => {}); // callbacks are DOM CustomEvents
</script>
```

**React** (the generated wrapper handles prop/event binding + typing):

```tsx
import { Greeter } from "./dist/external/react/Greeter";
<Greeter name="World" onGreet={() => {}} />
```

**Vue / Svelte / Angular**: import the wrapper from the matching folder. The
generated `README.md` and `consumers/` (in the `external-export-suite`
example) show a complete app per framework. For bundler projects, declare a
dependency on the emitted package — `"my-components": "file:./dist/external"`
— and import `my-components/react/Greeter`, etc.

## The prop boundary — what can cross to JS

Only prop types representable in JS are exported. Everything else is
**skipped with a printed warning** (never silently dropped):

| Author prop type | JS surface | Behaviour |
|------------------|-----------|-----------|
| `Reactive<String>` / `Reactive<bool>` / `Reactive<{int,float}>` | property + attribute | **reactive** — a JS write re-renders the live tree |
| plain `String` / `bool` / number | property + attribute | set once at mount |
| `Rc<dyn Fn()>` / `Option<Rc<dyn Fn()>>` | `el.onName` property **and** a DOM `CustomEvent` | component event → JS |
| `Rc<dyn Fn(T)>`, `T` a primitive | callback with one argument (in `event.detail`) | component event → JS, with a value |

Each prop becomes a `Signal<T>` inside the bridge, so reactive props stay
live across JS writes; callbacks store the supplied `js_sys::Function`.

**Not exportable** (skipped): handles / refs (`Ref<…>`, `…Handle`), style
structs, other framework-internal types; callbacks with more than one
argument or a non-primitive argument. Keep the *exported* surface of a
component to primitives + primitive-argument callbacks. The idiomatic shape
is a **controlled** component: the host owns state, sets props in, and
updates them in response to the component's callbacks.

## How it works

1. **Discover** — an ephemeral wrapper crate links the project with
   `runtime-core/catalog` on (the same mechanism `idealyst docs` / `idealyst
   mcp` use) and prints the external-component manifest as JSON.
2. **Generate the bridge** — one `#[wasm_bindgen]` class per component:
   props → `Signal<T>`, callbacks → stored `js_sys::Function`. Each element
   builds its own subtree into its host via `build_detached`.
3. **Build wasm** — `cargo build --target wasm32-unknown-unknown` then
   `wasm-bindgen --target web`.
4. **Generate the JS/TS surface** — custom-element shells, `.d.ts`, the
   per-framework wrappers, a barrel, a `package.json`, a demo page, and a
   usage README.

### Multiple components on one page

Several exported elements share **one** wasm module and coexist on a page —
each instance mounts its own independent subtree. Three details make this
safe (and matter if you ever hand-write a bridge):

- **One wasm init.** Every element module shares a single `globalThis`-keyed
  `init()` promise. wasm-bindgen's re-init guard only fires *after* init
  completes, so two element modules each calling `init()` concurrently would
  both re-instantiate the module and orphan the first elements' closures
  (a `null function` trap). One shared promise guarantees a single instance.
- **One shared backend.** All elements use the same web backend, so its
  id-keyed JS shims and node-id counter are allocated once.
- **Unique identity per element.** Each subtree builds under a distinct
  identity seed, so two elements' node ids never collide.

## Requirements

- `wasm32-unknown-unknown` target
  (`rustup target add wasm32-unknown-unknown`).
- `wasm-bindgen-cli` (`cargo install wasm-bindgen-cli`); `idealyst doctor`
  checks for it.
- Exported components and their props structs must be reachable down their
  `module_path` (declared in `pub` modules).

See the in-repo `examples/external-export-suite` for two components exported
to all five frameworks with a runnable consumer app each.
