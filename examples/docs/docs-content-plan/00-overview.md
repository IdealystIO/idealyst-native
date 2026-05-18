# Overview

This page answers one question: what does an Idealyst app actually look
like, and what happens when it runs?

## The model

Idealyst splits an app into two halves.

The **application** is your code. You write it once, in one Rust crate.
It contains the screens, the components, the state, the styles, the
navigation — everything about *what* the app does and *what* it looks
like. It does not contain any code that is specific to a platform.

The **backend** is the other half. A backend is the piece of code that
decides *how* your app runs on a particular platform. It is the API
that translates the framework's vocabulary — `View`, `Text`, `Button`,
a style being applied, a button being clicked — into operations on a
specific platform's native UI system.

The two halves never reach into each other. The application only knows
about the framework's vocabulary. The backend only knows how to
implement that vocabulary on its target. You pick which backend to use
by picking a build target; the CLI takes care of wiring them together.

## Which backends ship with Idealyst

Four backends come in the box. They exist so you have a sensible
default for the major platforms — none of them are the only way to
render to that platform, just the one we wrote.

- **Web** — renders into the DOM, compiled to WebAssembly.
- **iOS** — renders into UIKit views, via `objc2`.
- **Android** — renders into native Android views, via JNI.
- **Roku** — renders into a SceneGraph component tree. This one is
  experimental; it exists partly to show that the backend model isn't
  tied to traditional GUI toolkits.

If none of those fits — say you want to render to a terminal, an
embedded display, a game engine, or a platform we haven't covered —
you can write your own. Writing a backend means implementing a small
trait with a handful of methods. Everything else stays the same: the
same application code, the same components, the same styles.

> **From React.** The app + backend split is the same idea as
> splitting `react` from `react-dom` / `react-native` / a custom Fiber
> renderer. The difference is that Idealyst backends don't reconcile:
> there's no second tree being diffed against the first. The framework
> tells the backend exactly which node to create, insert, or update.

> **From Vue 3.** This is what Vue's `createRenderer` is for — a
> custom renderer that targets a non-DOM platform. Idealyst treats
> that seam as the primary interface, not an escape hatch.

> **From Solid.** Same conceptual split as `solid-js` plus its
> renderers (`solid-dom`, `solid-native`, etc.). The backend contract
> is small for the same reason it is in Solid: fine-grained reactivity
> means the framework can tell the renderer exactly what to change.

> **From Svelte.** Svelte doesn't really split this way — the compiler
> emits platform-specific code directly, and targeting mobile means a
> separate project (Svelte Native, NativeScript) with significant
> divergence. Idealyst's split is closer to React's renderer model.

## The pieces

An Idealyst app is built out of four kinds of things. You will use all
four in even the smallest app.

**Primitives** are the building blocks the framework knows how to
render. There is a fixed list of them: `View`, `Text`, `Button`,
`Pressable`, `ScrollView`, `Icon`, and a handful more. Every primitive
is something a backend has to know how to draw. Application code
composes primitives into trees; the framework hands those trees to the
backend.

**Components** are functions you write that return a tree of
primitives. A component can take props, can hold its own state, and can
contain other components. Components are how you give names to pieces
of your UI. The framework gives you a `#[component]` attribute that
turns a regular Rust function into a component the rest of the system
can use.

**Signals** are how state works. A signal holds a value. When you read
it, the framework remembers that you read it. When you change it, the
framework re-runs the small pieces of UI that depended on that value —
not the whole tree, just the pieces that read the signal. This is the
only way reactive updates happen in Idealyst. There is no virtual DOM
and no top-down re-render.

**Styles** are written in a separate macro called `stylesheet!`. A
stylesheet is a typed description of how a component should look: its
colors, spacing, borders, text size, and so on. Stylesheets are tied to
a theme, so the same stylesheet can produce a light or dark version of
itself without any code changes at the call site.

That's the surface. The rest of this guide is mostly about how those
four things connect to each other and to the platform underneath.

## Writing the tree: `ui!` and `jsx!`

Trees of primitives and components are written using a macro. Two are
built in:

- `ui!` is the default. It reads roughly like Rust, with component
  names, parenthesized prop lists, and braced children.
- `jsx!` is the same thing in JSX syntax, for people who prefer angle
  brackets.

Both macros produce the exact same output. The framework has no opinion
about which one you use; you can mix them inside the same file. They
exist because different teams have different reading preferences, not
because one is more capable than the other.

You can also write your own macro that emits the same shape of output.
Nothing else in the framework cares which one produced the tree.

## How a render happens

When the app starts, the host crate calls `app()`. That function
returns a `Primitive` — a tree of view, text, button, and other
primitive nodes, with reactive expressions interleaved inside. The
framework hands the tree to a piece of code called the **render
walker**.

The walker has two jobs: build the initial tree on the backend, and
wire up the reactive expressions inside that tree so future changes
update the right nodes. It does both in a single pass.

### Building the initial tree

The walker visits each primitive in order. For each one:

1. It calls a `create_*` method on the backend (`create_view`,
   `create_text`, `create_button`, …). The backend returns a node
   handle — a DOM element, a `UIView`, an Android `View`, or whatever
   the platform uses to represent a thing on screen.
2. If the primitive has a style, the walker resolves it against the
   active theme and calls `apply_style(node, rules)`.
3. The walker recurses into children. Each child returns its own
   handle, which the walker passes to `insert(parent, child)`.

The pass is purely additive: create, style, attach. There is no diff,
no patch, no second tree being compared.

### Wiring up reactivity

Whenever the walker meets a reactive expression — a `Text` whose
contents read a signal, a style that depends on the theme, a `for`
loop over a signal-backed list, the condition of a reactive `if` —
it wraps that expression in an **Effect**.

An Effect is the framework's lowest-level reactive primitive: a
closure with a stable identity that the framework re-runs whenever
its dependencies change. Each Effect created by the walker does three
things in its body:

- Reads the source signals (each `.get()` records the read).
- Computes the resulting value (a string, a style ruleset, a list of
  child nodes).
- Calls the matching backend method to apply it (`update_text`,
  `apply_style`, `clear_children` + a series of `insert` calls).

The Effect runs once immediately. On that first run, every signal it
reads is recorded as a dependency, and the Effect's id is added to
those signals' subscriber lists. That is the wiring.

By the time the walker reaches the end of the tree, the screen is on
the platform and every reactive expression is connected to its
sources.

### When a signal changes

`signal.set(new_value)` runs synchronously:

1. The signal writes the new value into its arena slot.
2. It looks up its subscriber set — the Effects whose most recent
   run read this signal.
3. The framework runs each subscriber Effect in turn:
   1. The Effect's previous dependency set is cleared.
   2. The Effect is marked as currently running, then its closure
      runs. Any signal it reads now is recorded as a fresh
      dependency.
   3. The closure produces a new value and (almost always) makes one
      backend call to apply it.
4. When all subscribers have run, `set` returns.

No other nodes are visited. No other Effects are touched. The
framework knows exactly which nodes care about this signal because
they told it on their last run.

### Cascading updates

A cascade is what happens when one Effect's body writes a signal
that other Effects read. The cascade is the chain of re-runs that
follows.

Cascades are synchronous and depth-first. If Effect A runs and writes
signal X (with subscribers B and C), B and C run on the same call
stack before A's call frame returns. If B writes signal Y (subscriber
D), D runs before B returns. By the time the outermost `set` returns,
every downstream Effect has either run or been skipped, and every
backend call those Effects produced has already been made.

Cascades terminate naturally:

- An Effect that reads but doesn't write doesn't extend the chain.
- An Effect that writes the same signal it reads is **skipped** —
  a re-entry guard prevents same-id loops, matching how Solid,
  MobX, and Reactively handle the same pattern.
- An Effect that writes to a signal with no subscribers stops the
  chain immediately.

There is no scheduler queue, no batch boundary, no microtask drain.
The framework walks the dependency graph and calls into the backend
as it goes.

### Dependencies are recomputed every run

Every Effect re-run starts by dropping its previous dependency set.
Whatever signals the body reads on this run become the new set.

An Effect that reads signal A on its first run and signal B on its
second (because a branch inside it took a different turn)
automatically stops listening to A and starts listening to B. There
is no explicit dependency array to maintain. No staleness, no
forgotten unsubscribes.

### Reactive subtrees and cleanup

`when(condition, then, otherwise)` — the reactive conditional behind
an `if` inside `ui!` that reads a signal — uses **scopes** to manage
the lifetime of a subtree:

1. The walker creates an Effect for the condition.
2. The Effect builds the active branch inside a fresh nested scope.
   Every signal and Effect created during that build is owned by the
   scope.
3. When the condition changes, the Effect drops the old scope — every
   signal, Effect, and backend node inside it is freed in one shot,
   and their subscriber entries are removed from the rest of the app
   — and builds the new branch in a fresh scope.

The same applies to a reactive `for`: each iteration lives in its
own scope, so removing an item from the list immediately frees
everything that item contributed.

This is why you don't write teardown code in components. The scope
owns the lifecycle.

### Why no diff

Other frameworks (React, Vue's template layer) compute the next
render as a tree, diff it against the previous one, and apply the
differences to the platform. The diff exists because the framework
doesn't know which parts changed — it has to compare to find out.

Idealyst doesn't compare. Every reactive expression registered itself
with the signals it reads. When a signal changes, the framework can
go straight to the Effects that care, run only them, and make only
the backend calls they produce. There is no "previous render" stored
anywhere; there is only the live tree, mutated in place.

The trade-off is that the framework carries a dependency graph and an
arena instead of a shadow tree. The payoff is that per-update cost is
proportional to the number of nodes that actually changed, not to the
size of the tree — the framework does less work as your app grows,
not more.

> **From React.** React's update model is reconciliation: a state
> change triggers a re-render of the component, which produces a new
> virtual DOM tree, which is diffed against the previous one, which
> is applied to the real DOM. The cost scales with the size of the
> subtree being re-rendered, which is why React has so much machinery
> — keys, memoization, fibers, lanes, `useMemo`, `React.memo` — for
> keeping that cost down.
>
> Idealyst has none of that. The framework already knows which
> backend node corresponds to which reactive read, because the read
> registered itself when the Effect was first built. A signal change
> goes straight to the Effect that reads it; the Effect makes one
> backend call; nothing else is visited or compared.
>
> The trade-off: Idealyst is faster on updates and uses less memory
> (no shadow tree to keep, no diff to compute). React is more
> forgiving — you can ignore the cost model entirely until your app
> is slow, then bolt on optimizations. Here, the cost model *is* the
> model: changes cost what they cost, and they cost almost nothing.

> **From Solid.** The update model is effectively identical. Solid
> is the framework Idealyst's reactivity is most directly inspired
> by: track-on-read, re-run on change, no diff, dynamic dependency
> sets, scoped subtrees that drop together. The differences are
> linguistic (Rust types vs JS closures) and structural (an arena
> rather than per-node `Rc` cells), not behavioral.

> **From Svelte 5.** The cascade is conceptually the same as
> Svelte 5's runes (`$state` → `$derived` → `$effect`). Svelte's
> advantage is that the compiler can inline the tracking into the
> compiled `.svelte` output — there's no runtime arena, no Effect
> handles. Idealyst's advantage is that there's no compiler step
> beyond rustc, so what the type system shows you is what runs.

> **From Vue 3.** Same dependency-tracking shape — reads inside an
> effect subscribe to the source, mutations notify the subscribers.
> The visible difference is that Vue's render pipeline still runs a
> virtual-DOM diff for the template portion of an update. Idealyst
> has no template-level diff because there is no template being
> re-rendered, just Effects mutating the tree in place.

### A backend's job, summarized

Once the framework's side is in place, a backend only has to know
how to create each kind of primitive, how to put one inside another,
how to update one when it changes, and how to apply a style. The
walker takes care of everything else.

## Reactivity

Reactivity is the mechanism behind every change in an Idealyst app — a
counter incrementing, a list growing, the theme switching from light
to dark, a screen transitioning in. It is one idea, applied
everywhere: when a value is read inside a place that depends on it,
the framework remembers the connection; when the value changes, those
places re-run.

There is no separate update system for state, for styles, for themes,
or for navigation. They all use the same machinery, and the machinery
is deliberately small.

### Signals

A **signal** holds a value. You make one with `signal!(initial)`,
read it with `.get()`, and change it with `.set(new)` or
`.update(|v| ...)`.

```rust
let count = signal!(0);
count.set(5);
count.update(|n| *n += 1);
```

Signals are the only kind of state the framework knows about. A
regular Rust variable is just data — if you want the UI to react when
it changes, it has to live in a signal.

> **From React.** A signal is **not** `useState`. `useState` re-runs
> the component (and its subtree) when its setter fires. A signal does
> not: it notifies only the specific reads that depended on it. The
> closer React analog is `useSyncExternalStore` over an observable, or
> a library like Jotai or Zustand. The shift in thinking: you're not
> handing new props to a component and asking it to render again —
> you're letting a value notify the few places that read it.

> **From Solid.** Signals here are conceptually identical to
> `createSignal`. Reads are tracked, effects re-run on change,
> components run once. The API names differ (`signal!`, `.get()`,
> `.set()` here vs `createSignal`, `value()`, `setValue` in Solid) but
> the model is the same.

> **From Svelte 5.** A signal is what Svelte 5 calls a `$state` rune.
> Same idea: a value that tracks its readers and notifies them on
> change. The difference is mostly in delivery — Svelte's runes are
> compiler-driven inside a `.svelte` file, while Idealyst signals are
> a regular Rust type, no preprocessor involved.

> **From Vue 3.** A signal is `ref()` with `.value` replaced by
> `.get()` / `.set()`. Reads inside a `computed`, an `effect`, or a
> template subscribe to the ref; mutations notify them. The model is
> the same; only the API surface differs.

### What is reactive

Anywhere a signal is read inside a **tracked context**, the framework
records the dependency. The tracked contexts you'll meet are:

- The contents of a `Text { format!("count: {}", count.get()) }`
- The condition of an `if` inside `ui!` that reads a signal — the
  branches re-evaluate when the condition changes
- A `for` loop whose source is a signal-backed list
- A prop passed as a closure that reads a signal
- A stylesheet — reading from the active theme is itself a tracked
  read

Underneath all of these is `Effect`, the framework's lowest-level
reactive primitive. Effects are how the framework wires UI to signals
internally; you rarely write one by hand, but it's the name to look
up if you ever need to understand what's happening below the surface.

Plain expressions that don't read a signal are not tracked. They run
once when the tree is built, and never run again.

### Components don't re-render

This is worth saying directly because it's the point that catches
people coming from most other frameworks.

A `#[component]` function runs **once**, when its part of the tree is
built. It returns a `Primitive` tree. After that, the component
function itself is gone; what's left on the platform is the tree it
produced, with Effects threaded through it. State changes don't
re-call the component. They re-run only the small reactive closures
inside the tree.

For the mechanism behind the re-runs — how a signal change flows
through Effects, how cascades terminate, how reactive subtrees clean
themselves up — see [How a render happens](#how-a-render-happens)
above.

> **From React.** This is the biggest mental shift from React. In
> React, a function component runs on every render. Here, a
> `#[component]` function runs **once**. It produces a tree;
> reactivity lives inside that tree as closures. You don't memoize
> components, you don't worry about prop reference equality, you don't
> think about render passes — there aren't any in the React sense.
> If you've used React with heavy `useMemo` / `useCallback` to keep
> re-renders from cascading, this is what it would look like if every
> reactive read were memoized by default and only the changed ones
> were re-evaluated.

> **From Solid.** Same model as Solid: components are constructor
> functions, not render functions. Mutations propagate through tracked
> effects, not through component re-runs. You'll feel at home.

> **From Svelte 5.** Conceptually close — Svelte 5 components also
> don't re-run as a whole on state change. The visible difference is
> in how you write them: a `.svelte` file is preprocessed into
> reactive code, while an Idealyst component is a plain Rust function
> that returns a primitive tree.

> **From Vue 3.** Composition-API `setup()` runs once and reactive
> updates flow through the template's tracked reads. Idealyst behaves
> the same way: the component function (the analog of `setup`) runs
> once, and updates flow through the tracked closures in the returned
> tree.

### Styles and themes use the same machinery

Styling and theming aren't a separate system in Idealyst. They use
the same reactivity that powers signals.

A stylesheet is a description that resolves against the active theme,
and the resolution is itself a tracked context. When the theme
changes — say, your "dark mode" toggle calls `set_theme(dark_theme())`
— the framework re-runs only the style resolutions that depended on
the tokens that changed, and calls `apply_style` on the backend for
those specific nodes.

The same goes for any value derived from a theme: a button color, a
border radius, a font size. Swap the theme, and the framework updates
exactly the parts that referenced what changed. You don't write code
to listen for theme changes, because there is no separate listener to
write — it's the same path that updates a `Text` when a counter
increments.

> **From React.** Theme changes in React typically go through Context,
> and changing the context value re-renders every consumer (unless
> you've wrapped them in memoization). Here, the theme is a signal
> and stylesheets read from it. Only the styles that read the tokens
> that actually changed re-resolve; only their nodes get an
> `apply_style` call from the backend. No consumer re-renders, no
> Provider tree to thread through.

> **From Solid.** Behaves like a Solid context backed by signals —
> only the reads that depended on the changed value re-run.

> **From Svelte 5.** Equivalent to a `$state` object shared through a
> module, where consumers' `$derived` reads update only where they
> matter. The styling system uses that same machinery — but you don't
> author the consumer code, because the stylesheet machinery is the
> consumer.

> **From Vue 3.** Equivalent to providing a reactive object via
> `provide` / `inject`: the consumers' computed and template reads
> update selectively. The difference is that you don't write the
> consumers — the stylesheet resolver already is one.

## What you don't write

You don't write the wasm-bindgen entry point for web.
You don't write the JNI bridge for Android.
You don't write the `UIApplicationDelegate` for iOS.
You don't write the Roku component manifest.

The CLI generates all of that for you when you build for a target. The
generated code is small — it exists to attach your app to the backend
and start the render — but you don't have to touch it.

When you run `idealyst new`, you get one crate: the app. When you run
`idealyst dev` or `idealyst build`, the CLI creates the host crate for
each platform in a build directory and compiles it against your app.
If you ever need to see what the host looks like, you can read the
generated source. You just don't write it by hand.

## The architecture in more depth

The Application → Backend split is the headline. In practice the
framework is a stack of small, sharply-scoped crates with clean
boundaries between them. Most apps only interact with the top of that
stack. The lower seams exist so the system can grow: hot reload,
app-as-server, test automation, IDE tooling, and your own extensions
all hook in at one of these layers without reaching past it.

```
  ┌─────────────────────────────────────────────────────────────┐
  │                       Application                            │
  │   components, signals, stylesheets, ui! / jsx!               │
  └─────────────────────────────────────────────────────────────┘
                              │
            ┌─────────────────┴─────────────────┐
            ▼                                   ▼
  ┌────────────────────┐               ┌────────────────────┐
  │  framework-macros  │               │      idea-ui       │
  │  ui!, jsx!,        │               │  component library │
  │  #[component],     │               │  (optional)        │
  │  stylesheet!,      │               └─────────┬──────────┘
  │  methods!          │                         │
  └─────────┬──────────┘                         │
            └─────────────────┬───────────────────┘
                              ▼
  ┌─────────────────────────────────────────────────────────────┐
  │                      framework-core                          │
  │  primitives  ·  signals + effects + scopes                   │
  │  render walker  ·  style resolution + theming                │
  │  identity  ·  scheduling  ·  Backend trait                   │
  │  (optional) Robot introspection                              │
  └─────────────────────────────────────────────────────────────┘
                              │
          ┌───────────────────┼───────────────────┐
          ▼                   ▼                   ▼
  ┌──────────────┐    ┌──────────────┐    ┌──────────────────┐
  │framework-hot │    │framework-wire│    │ framework-native-│
  │ hot patches  │    │ dev protocol │    │     layout       │
  │              │    │              │    │  (Taffy / flex)  │
  └──────────────┘    └──────┬───────┘    └──────────────────┘
                             │
                             ▼
                  ┌────────────────────┐
                  │framework-dev-client│
                  │ app-side replayer  │
                  └────────────────────┘
                             │
                       Backend trait
          ┌──────────┬─────────┬─────────┬─────────┐
          ▼          ▼         ▼         ▼         ▼
        web        ios     android     roku    aas-shell
       (DOM)    (UIKit)   (Views)   (SG/TV)  (dev mode)
```

### The layers in one line each

- **framework-macros** — Compile-time DSLs (`ui!`, `jsx!`,
  `#[component]`, `stylesheet!`, `methods!`). Lowers source into plain
  framework-core calls; nothing here exists at runtime.

- **framework-core** — The runtime everything else builds on.
  Primitives, signals + effects, render walker, style resolution,
  identity, the `Backend` trait. Your app code talks mostly to this
  crate.

- **idea-ui** — Optional component library on top of framework-core.
  Heading, Card, Stack, Btn, themed colors, breakpoints. Use it,
  replace bits of it, or skip it.

- **framework-hot** — Diff-and-patch for hot reload. Compares two
  `Primitive` trees by their identity hashes and produces the minimal
  sequence of backend operations to morph one into the other.

- **framework-wire** — The wire protocol. Pure data: a `Command` enum
  and three id namespaces (nodes, handlers, styles). No framework-core
  dependency. Used by hot reload, app-as-server, and any future
  server-driven mode.

- **framework-dev-client** — The app side of the wire. Receives
  commands from the dev server and replays them against the local
  backend, so `idealyst dev` updates a running app without
  recompiling.

- **framework-native-layout** — Wraps Taffy (flexbox + grid) for
  backends without a native layout engine. Web uses the browser's
  layout; iOS, Android, and Roku use this.

- **Robot** — Feature-gated introspection inside framework-core. With
  `--features robot` enabled, external processes can list components
  on screen, find elements by props or path, read frames, click, and
  type. Powers automated testing without per-platform harnesses.

- **Backends** — One crate per platform under `crates/backend/`. Each
  implements `Backend` by translating its method calls into native
  operations. The AAS backend is the odd one out — it serializes the
  tree onto the wire instead of rendering it.

- **CLI** — Orchestration. Scaffolds projects, runs the dev server,
  materializes the per-platform host crate, drives builds. Not part
  of the runtime; an Idealyst app can run without ever invoking the
  CLI.

### Seams that let you hook in

Each crate boundary is a place to plug something new in:

- **The Backend trait.** Write a new platform — terminal, embedded
  display, game engine, anything you can drive from Rust. Implement
  a handful of methods; everything above the seam stays the same.
- **framework-wire.** Write a new transport, a new viewer, or a
  server-driven UI host. The protocol is pure data; nothing about it
  assumes "the dev server" specifically.
- **framework-hot.** Substitute a different diff strategy, or
  intercept patches to log, replay, or transform them.
- **Robot.** Drive a running app from another process — IDE plugins,
  accessibility tooling, scripted demos, test runners. The
  `robot-mcp-proxy` crate is one such consumer; you can write your
  own.
- **framework-macros.** Write your own front-end syntax. Anything
  that emits the right framework-core calls slots in alongside `ui!`
  and `jsx!`.

None of these are required to ship an app. They exist because the
seams existed first — the framework is built so each of these was
already possible by the time anyone thought to want it.

### Going deeper

Topics that go past the overview, with their own pages:

- **Writing a backend** — the `Backend` trait, every method's
  contract, and what a minimal implementation looks like.
- **The dev server and wire protocol** — what `idealyst dev` does,
  what the wire commands carry, how reverse callbacks resolve.
- **Robot** — controlling a running app from another process, the
  MCP surface, what queries are available.
- **Writing your own DSL** — the contract a front-end macro has to
  satisfy to participate in reactivity and component composition.
- **Hot reload internals** — identity hashing, diff strategy, what
  changes survive a patch and what forces a full rebuild.

## What to read next

If you've never written an Idealyst app, go to the **Quickstart** next.
It walks you from `cargo install` through a running counter app on the
web.

If you want to understand the moving parts before writing anything,
read the pages in this order:

1. **Primitives** — the fixed vocabulary you build out of.
2. **Components** — how to define your own.
3. **Reactivity** — signals, effects, how updates propagate.
4. **Styles** — stylesheets, themes, variants.
5. **Navigation** — screens, routes, drawer/tab navigators.
6. **The CLI** — the workflow commands and what they do.
7. **Backends** — what a backend is responsible for, if you want to
   write one or just understand the seam.
