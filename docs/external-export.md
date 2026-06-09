# External export — Idealyst components in React, Vue, and vanilla JS

`idealyst export` turns a project's `#[component(external)]`-tagged
components into a **framework-agnostic Web Component suite**: each component
becomes a wasm-backed [custom element](https://developer.mozilla.org/docs/Web/API/Web_components)
that works unchanged in React, Vue, Svelte, Angular, or a plain HTML page,
plus generated `.d.ts` declarations and typed per-framework wrappers.

The author writes **no bridge code** and no JavaScript. The component is
ordinary platform-agnostic framework code; one attribute opts it into
export, and the CLI generates everything else.

## Tag a component

Add `external` to the `#[component]` attribute and derive `IdealystSchema`
on its props (the derive is what publishes the prop names + types the
generator reads):

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
            button(label = "Greet".to_string(), on_click = move || {
                if let Some(cb) = &on_greet { cb(); }
            })
        }
    }
}
```

`#[component(external)]` accepts an optional tag override:
`#[component(external(tag = "my-greeter"))]`. The default tag is
`idl-<kebab-case-name>` (here, `idl-greeter`).

## Run the export

```
idealyst export                 # current directory
idealyst export path/to/project --release
```

Output lands in `dist/external/`:

```
dist/external/
├── pkg/                       # shared wasm + wasm-bindgen JS glue
│   ├── external_bridge.js
│   └── external_bridge_bg.wasm
├── idl-greeter.js             # the custom element (self-registers on import)
├── idl-greeter.d.ts           # TypeScript declarations
├── vanilla/Greeter.js         # imperative create*/bind* helpers
├── react/Greeter.tsx          # typed React wrapper
├── vue/Greeter.js             # Vue wrapper
├── svelte/Greeter.svelte      # Svelte wrapper
├── angular/greeter.component.ts  # standalone Angular component
├── index.js                   # barrel — imports every element
├── index.html                 # a vanilla demo host page
└── README.md                  # usage for every framework
```

Pick a subset with `--frameworks`:

```
idealyst export --frameworks react,svelte
```

The bare custom element works in **every** framework regardless — these
folders are just typed convenience wrappers. The default emits all of them.

## Use it

**Vanilla / any framework that consumes custom elements** (Solid, Lit,
Qwik, … all work the same way):

```html
<script type="module" src="./dist/external/index.js"></script>
<idl-greeter name="World"></idl-greeter>
<script type="module">
  const el = document.querySelector("idl-greeter");
  el.name = "Idealyst";                 // reactive: re-renders, no remount
  el.addEventListener("greet", () => console.log("greeted"));
</script>
```

Or the imperative helpers (no markup):

```js
import { createGreeter } from "./dist/external/vanilla/Greeter.js";
document.body.append(createGreeter({ name: "World", onGreet: () => {} }));
```

**React** (the generated wrapper handles prop/event binding + typing):

```tsx
import { Greeter } from "./dist/external/react/Greeter";
<Greeter name="World" onGreet={() => console.log("greeted")} />
```

**Vue:**

```js
import { Greeter } from "./dist/external/vue/Greeter";
```

**Svelte:**

```svelte
<script>import Greeter from "./dist/external/svelte/Greeter.svelte";</script>
<Greeter name="World" onGreet={() => {}} />
```

**Angular** (standalone — add to a component's `imports`):

```ts
import { GreeterComponent } from "./dist/external/angular/greeter.component";
// <idl-greeter-ng [name]="'World'" (greet)="onGreet()"></idl-greeter-ng>
```

Value props → `@Input`, callbacks → `@Output` `EventEmitter`s.

## How props and callbacks cross the boundary

| Author prop type | JS surface | Behaviour |
|------------------|-----------|-----------|
| `Reactive<String>` / `Reactive<bool>` / `Reactive<{int,float}>` | property + attribute | **reactive** — a JS write re-renders the live tree |
| plain `String` / `bool` / number | property + attribute | set once at mount |
| `Rc<dyn Fn()>` / `Option<Rc<dyn Fn()>>` | `el.onName` property **and** a DOM `CustomEvent` | component event → JS |
| `Rc<dyn Fn(T)>` where `T` is a primitive | callback with one argument (in `event.detail`) | component event → JS, with a value |

Each prop becomes a signal inside the wasm bridge, so reactive props stay
live across JS writes. Callbacks store the supplied `js_sys::Function` and
invoke it when the component fires.

### What can't cross (yet)

Props whose type can't be represented in JS are **skipped** with a printed
warning (never silently dropped):

- handles / refs (`Ref<…>`, `…Handle`), style structs, and other
  framework-internal types;
- callbacks with more than one argument, or whose argument isn't a
  primitive.

A skipped prop simply doesn't appear on the element; the rest export
normally. Keep the *exported* surface of a component to primitives +
primitive-argument callbacks.

## How it works

1. **Discover.** An ephemeral wrapper crate links the project with
   `runtime-core/catalog` on (the same mechanism `idealyst docs` / `idealyst
   mcp` use) and prints the external-component manifest as JSON — each
   tagged component joined to its prop schema.
2. **Generate the bridge.** From that manifest the CLI generates an
   ephemeral `cdylib` crate with one `#[wasm_bindgen]` class per component:
   props → `Signal<T>`, callbacks → stored `js_sys::Function`. Each element
   builds its own subtree into its host via `build_detached`, under a
   **single shared `WebBackend`** ([`WebBackend::new_in`](../crates/backend/web/src/lib.rs))
   and a unique identity seed.
3. **Build wasm.** `cargo build --target wasm32-unknown-unknown` then
   `wasm-bindgen --target web`.
4. **Generate the JS/TS surface.** Custom-element shells, `.d.ts`, the
   per-framework wrappers (vanilla/React/Vue/Svelte/Angular), a barrel, a
   demo page, and a usage README.

### Multiple components on one page

Several exported elements share **one** wasm module and coexist on a page
the framework doesn't own — each instance mounts its own independent
subtree. Three details make this safe, and matter if you ever hand-write a
bridge:

- **One wasm init.** Every element module shares a single `globalThis`-keyed
  `init()` promise. wasm-bindgen's re-init guard only fires *after* init
  completes, so two element modules each calling `init()` concurrently would
  both re-instantiate the module and orphan the first elements' closures (a
  `null function` trap). One shared promise guarantees a single instance.
- **One shared backend.** All elements use the same `WebBackend`, so the
  backend's id-keyed JS shims (reactive-text/class batchers) and node-id
  counters are installed and allocated once.
- **Unique identity per element.** Each subtree builds under a distinct
  identity seed, so two elements' node ids never collide.

## Requirements

- `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`)
- `wasm-bindgen-cli` (`cargo install wasm-bindgen-cli`); `idealyst doctor`
  checks for it.
- Exported components and their props structs must be reachable down their
  `module_path` (i.e. declared in `pub` modules).
