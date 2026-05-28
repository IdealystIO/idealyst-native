# Cross-Platform Native Framework — Architecture Research

> A research document outlining the conceptual architecture for a Rust-based, cross-platform application framework with fine-grained reactivity, native bindings, WASM web target, and first-class support for server-driven UI.

---

## Vision

A cross-platform application framework that lets developers write app logic once and deploy natively to mobile, desktop, and web — without an interpreted runtime acting as a permanent abstraction tax. The framework provides a stable set of primitive UI elements, a fine-grained reactive state system, and a pluggable backend abstraction that lets each target platform implement rendering in the most native way available to it.

The same app code targets:
- iOS (via UIKit)
- Android (via JNI to View system or Compose)
- macOS (via AppKit)
- Web (via WASM + DOM)
- Server-side HTML (SSR)
- Server-driven native UI (SDUI, future)

## Non-Goals

- Replicating the JS package ecosystem culture
- Pixel-perfect identical visual rendering across platforms (we prefer platform-native feel)
- Web-framework feature parity with React/Next for v1
- A visual design system (theming is exposed; the framework stays unopinionated about design tokens)

---

## Core Principles

1. **One application, many backends.** App code is backend-agnostic Rust. Each platform binding is a separate crate implementing a common `Backend` trait. Adding a new platform means writing a new backend implementation, never modifying app code.

2. **Native everywhere, no permanent JS runtime.** On mobile and desktop, the framework compiles to native binaries that bind directly to platform UI APIs. On web, the framework compiles to WASM that drives DOM updates through wasm-bindgen — with JS treated as the web's *platform binding language* rather than the runtime.

3. **Fine-grained reactivity, no shadow tree.** State lives in signals. UI bindings subscribe to specific signals at construction time. State changes trigger surgical updates to specific nodes, not full-tree re-renders. The framework has no virtual DOM or fiber tree.

4. **Compile-time analysis via Rust macros.** The component DSL is a `view!`-style procedural macro that analyzes expressions for signal dependencies and emits the necessary effect closures. App developers write code that reads like a function of state; the compiler distributes the reactivity.

5. **Wire-ready from day one.** Every primitive, op, and action is `serde`-serializable from the outset. This enables HTML SSR and (eventually) native SSR / server-driven UI without architectural changes.

6. **Module boundaries are dyn-dispatched.** Code splitting at runtime (WASM modules) and at the network boundary (server-driven UI) both rely on stable trait-object interfaces. The framework's component model is designed to be `dyn`-friendly at these seams.

---

## System Overview

```
┌──────────────────────────────────────────────────────┐
│              Application Code (Rust)                 │
│       components, signals, routes, actions           │
└──────────────────────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────┐
│                Framework Core (Rust)                 │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐        │
│  │ Reactivity │ │ Primitives │ │   Router   │        │
│  │  (signals) │ │  + Layout  │ │            │        │
│  └────────────┘ └────────────┘ └────────────┘        │
│  ┌──────────────────────────────────────────┐        │
│  │   Op Stream / View Tree (serializable)   │        │
│  └──────────────────────────────────────────┘        │
└──────────────────────────────────────────────────────┘
                          │
                          ▼  (Backend trait)
   ┌──────────┬───────────┬──────────┬──────────┬──────────┐
   ▼          ▼           ▼          ▼          ▼          ▼
[Cocoa]    [UIKit]    [Android]    [Web]     [SSR]    [SDUI]
NSView     UIView      JNI/        WASM+JS    HTML     wire
                      Compose                          format
```

---

## Subsystems

### 1. Reactivity (Signals)

Observable state primitives that track subscribers at access time. When a signal changes, only effects that read it during their setup re-execute.

**Primitives:**
- `Signal<T>` — read/write reactive value
- `Memo<T>` — computed value derived from other signals
- `Effect` — side-effecting subscription that runs when its dependencies change
- `Resource<T>` — async data fetched from a source (server fn, network, etc.)
- `Owner` / `Scope` — tracks subscription lifetimes for cleanup

**Render modes:**
- *Live* — full subscription tracking; effects re-run on change. Used in mobile/desktop runtime and web hydrated runtime.
- *Static* — initial values evaluated, no subscriptions. Used in SSR.

**Known footgun:** Direct expressions in component bodies run once, not on every state change. Derived values must be wrapped in closures or memos. Mitigated with linting and documentation; intrinsic to fine-grained reactivity.

### 2. Primitives and View Tree

A small, stable vocabulary of UI elements every backend must implement. Initial set:

- `View` — container with layout
- `Text` — string content with style
- `Image` — source, sizing
- `Pressable` / `Button` — interactive surface
- `Input` — text entry
- `Scroll` — scrollable container
- `List` — virtualized collection (with item recycling)

**Layout:** Flexbox-like. Strong recommendation: use Taffy (Rust port) rather than Yoga (C dep). Each platform backend maps primitive layout to its native system (Auto Layout, ConstraintLayout, CSS).

**Wire format:** Every primitive and prop is `serde`-serializable. Start with JSON for debuggability; swap to MessagePack or Postcard for size when stable.

**Open question:** Where to draw the line on primitive count. Too few and apps reinvent everything; too many and platform backends become unwieldy.

### 3. Backend Trait

The framework core emits an operation stream; backends consume it. Conceptual sketch:

```rust
trait Backend {
    type Node;
    type Event;

    fn create(&mut self, primitive: &Element) -> Self::Node;
    fn update(&mut self, node: &Self::Node, diff: &PropDiff);
    fn insert(&mut self, parent: &Self::Node, child: Self::Node, before: Option<&Self::Node>);
    fn remove(&mut self, node: Self::Node);
    fn set_event_handler(&mut self, node: &Self::Node, event: EventKind, handler: Handler);
    fn schedule_main(&self, work: BoxedWork);
    fn now(&self) -> Instant;
}
```

**Targets:**
| Backend | Platform | Notes |
|---|---|---|
| `cocoa-backend` | macOS | `objc2`, `block2` |
| `uikit-backend` | iOS | `objc2` |
| `android-backend` | Android | JNI; consider emitting Compose Composables under the hood |
| `web-backend` | Web client | WASM + wasm-bindgen + DOM |
| `ssr-html-backend` | Server | HTML string output |
| `sdui-backend` | Server (future) | Serialized op stream to client |
| `mock-backend` | Testing | Records ops for assertion |

The `Backend` trait must remain `dyn`-friendly. App code never names a concrete backend type.

### 4. Component Model and DSL

A component is a Rust function taking props and returning a view subtree:

```rust
#[component]
fn counter(initial: i32) -> View {
    let count = signal(initial);
    view! {
        <View layout=row>
            <Text>"Count: " {count.get()}</Text>
            <Button on_click=move |_| count.set(count.get() + 1)>"+"</Button>
        </View>
    }
}
```

**The `view!` macro:** Procedural macro that analyzes expressions for signal reads and emits the necessary effect closures. Modeled on Leptos's `view!` macro; lift heavily from their patterns.

**dyn-friendly trait object:** Components implement a `Component` trait that's object-safe, so they can be passed across module boundaries for code splitting and SDUI.

### 5. Routing

Type-safe route definitions usable identically on server (URL → component) and client (in-app navigation):

```rust
routes! {
    "/" => Home,
    "/products" => ProductList,
    "/products/:id" => ProductDetail,
}
```

The route map is a shared data structure. SSR backend matches request URL; web client intercepts navigation; mobile uses it for typed in-app navigation. Native back-button semantics and stack-based navigation are platform-specific concerns handled inside the backend, not the route map itself.

**Open question:** Ergonomics for nested layouts and parallel routes (study Next.js parallel routes, Remix outlets).

### 6. SSR & Hydration

**HTML SSR:** Server backend renders the view tree to an HTML string. Signals operate in static mode. Resources resolve before serialization or stream chunks if async.

**Hydration:** Client-side WASM loads, walks server-rendered DOM in lockstep with the component tree, claims existing nodes, attaches event handlers, and rebuilds signal subscriptions. Hydration mismatches surface as developer errors with a clear "where" pointer.

**Server functions:** Annotated Rust functions that compile to server-only code with a client-side fetch shim:

```rust
#[server]
async fn get_user(id: UserId) -> Result<User, ServerFnError> {
    // server-side database access
}
```

On the client, `get_user` becomes an HTTP call; on the server, it runs natively. Same call site, different compilation.

### 7. Native SSR / Server-Driven UI

The server runs app code and produces a serialized view tree + action vocabulary. Mobile/desktop runtime receives the tree and renders it via its existing platform backend. App ships only the framework runtime, not the app logic.

**Wire format:** Same op stream/view tree the framework emits internally, serialized.

**Action vocabulary** — interactions cannot ship arbitrary code (App Store rules + security). Instead, interactions emit declarative actions the client interprets:

- `Action::Navigate(Route)`
- `Action::Mutate(SignalId, Mutation)`
- `Action::Submit(FormId)`
- `Action::Network(ServerFnId, Args)`
- Extensible via a registered handler table per app

**Element whitelist:** The server may only emit primitives the client recognizes. App-author components run server-side and expand into primitive trees. Versioning the protocol means versioning the primitives.

**State synchronization:** Local signals on client are mapped to server-known IDs. Server templates reference signals by ID; client wires them up locally. Optimistic local updates with eventual server reconciliation.

**Versioning:** Forward-compatible decoding (unknown primitives render as inert placeholders); semantic protocol version in the wire format.

**Research direction:** WASM-over-the-wire — shipping small WASM modules alongside UI descriptions for richer server-delivered logic. App Store review story is the open question.

### 8. Code Splitting Strategy

**Goal:** Lazy-load route-level chunks on web; potentially modular features on native.

**Primary technique:** Dyn-dispatched module boundaries. Each splittable unit (route, feature) implements a `Module` or `Feature` trait. Other modules hold trait objects, not concrete types — preventing Rust's monomorphization from duplicating framework code into each chunk.

**Tooling:**
- v1: Trunk/wasm-pack with manual chunks
- v1.5: Integrate `wasm-split` (Dioxus team) when stable
- Future: Component Model + `wit-bindgen` when browser support solidifies

**Bundle-size discipline:** `wasm-opt -Oz`, LTO, `codegen-units = 1`, `opt-level = "z"`, `panic = "abort"`, no-default-features on dependencies, careful proc-macro usage.

**Initial target:** Framework runtime alone under ~150KB compressed.

### 9. Compilation Targets

Same app source, multiple targets via Cargo features and target tuples:

| Target | Cargo Triple | Backend |
|---|---|---|
| iOS | `aarch64-apple-ios` | uikit-backend |
| Android | `aarch64-linux-android` | android-backend |
| macOS | `aarch64-apple-darwin` | cocoa-backend |
| Web (client) | `wasm32-unknown-unknown` | web-backend |
| Server (SSR) | `x86_64-unknown-linux-gnu` | ssr-html-backend |

**Discipline:** App code is target-agnostic. No `cfg(target_arch = ...)` in user code. All platform-conditional logic lives in framework crates.

---

## Open Research Questions

1. **Signal API in Rust.** How to expose ergonomic signal APIs given Rust's ownership model. Leptos's `Owner`/`Scope` machinery is a strong reference but has known ergonomic friction. Worth deep study before settling.

2. **Component trait shape.** What does the object-safe `Component` trait look like? Inputs (props), outputs (view tree), lifecycle hooks? Balance between flexibility and dyn-dispatch overhead.

3. **Layout engine choice.** Taffy (Rust, modern) is likely the right answer but evaluate against Yoga (C, mature) and custom.

4. **Effect scheduling and batching.** How to batch multiple signal updates into a single render pass without surprising the developer. Leptos's microtask-batched approach is one model.

5. **Async and Suspense.** How `Resource<T>` integrates with the view tree. SwiftUI's `.task` and Solid's `<Suspense>` are reference points.

6. **Server function security.** How to encode permissions, validation, CSRF protection into the `#[server]` macro.

7. **Wire protocol versioning.** Design of the version negotiation handshake for SDUI — needs to handle long-tail of older mobile clients gracefully.

8. **DevTools.** Inspecting reactive graphs at runtime, debugging hydration mismatches, profiling render performance. Feature-flagged debug runtime.

9. **App Store policy on WASM execution.** If WASM-over-the-wire is in scope, this needs an answer from Apple specifically.

---

## Prior Art to Study

- **Leptos** — closest existing Rust framework; study its `view!` macro, signal system, and SSR/hydration code paths
- **Dioxus** — VDOM-based Rust UI, multi-platform; reference for backend abstraction and `wasm-split`
- **Xilem** — Raph Levien / Linebender; iterating on Rust UI reactivity ergonomics under the borrow checker
- **Solid.js** — non-Rust, but the canonical reference for fine-grained reactivity + compile-time JSX analysis
- **SwiftUI** — Apple-style declarative UI + observation system
- **Jetpack Compose** — compiler-assisted reactivity; reference for Android backend
- **Airbnb's SDUI** — published architecture papers; reference for server-driven UI subsystem
- **Hyperview** — open-source HTML-for-native; reference for wire format and action vocabulary

---

## Phased Build Plan

### Phase 1 — Core foundations
- Signal / effect / memo primitives
- `Owner` / `Scope` lifetime tracking
- `Component` trait (object-safe) + `view!` macro skeleton
- `Backend` trait + `mock-backend` for testing
- Element vocabulary (initial set) with serde derives
- Op stream / reconciler emitter

### Phase 2 — First platform
- Pick one platform target (recommend `cocoa-backend` — lowest impedance match to declarative UI, mature `objc2` bindings)
- Build a non-trivial sample app (more than a counter — a small todo with persistence)
- Iterate on primitive vocabulary and component ergonomics

### Phase 3 — Web target
- `web-backend` via wasm-bindgen
- Router with client-side navigation
- First bundle-size optimization pass
- A working web sample of the same app from Phase 2

### Phase 4 — SSR
- `ssr-html-backend`
- Server functions
- Hydration walker

### Phase 5 — Additional native targets
- `uikit-backend`, `android-backend`
- Cross-platform navigation primitives (stack, modal, tab semantics)

### Phase 6 — Native SSR / SDUI
- Wire-format finalization
- Action vocabulary registry
- Server-addressable signals
- Versioning protocol and forward-compat decoder

### Phase 7 — Code splitting and polish
- Module-boundary trait design
- Bundle splitting tooling integration
- DevTools and profiling

---

## Design Disciplines to Maintain Throughout

These are the invariants that protect future capabilities. Violating any of these now creates a rewrite later:

1. **Framework core has zero platform assumptions.** No DOM access, no Objective-C, no JNI — only the `Backend` trait.
2. **App code is target-agnostic.** No `cfg(target_arch)` in user code. Ever.
3. **Every primitive and op is `serde`-serializable.** Even before SDUI ships, this discipline must hold.
4. **Module-boundary types are object-safe.** `Component`, `Module`, `Route` all dyn-compatible.
5. **Signals have an explicit render mode.** Live vs static is a first-class distinction.
6. **The reconciler emits ops; it never applies them directly.** The Backend always sits between the reconciler and any side effects.