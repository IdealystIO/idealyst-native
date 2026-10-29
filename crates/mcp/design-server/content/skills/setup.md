+++
title = "Setting up an idea-ui app"
summary = "Dependencies, the module layout (brand, shell, page_layout, screens, components), what app() does in which order, and the idea-ui code conventions the design recipes assume."
when = "Starting an idea-ui app, adding idea-ui to a scaffold, or deciding where a new screen/component file goes."
order = 15
tags = ["setup", "structure", "boot", "dependencies", "conventions"]
+++
# Setting up an idea-ui app

## Dependencies

On top of the scaffold's `idealyst`, `runtime-core`, `runtime-vocabulary`
and `runtime-scene`:

```toml
idea-ui = { version = "13", registry = "idealyst" }
idea-ui-nav = { version = "10", registry = "idealyst" }    # AppShell, TabBar, StackHeader, Drawer
swap-navigator = { version = "3", registry = "idealyst" }  # sections
stack-navigator = { version = "4", registry = "idealyst" } # drill-down
icons-lucide = { version = "3", registry = "idealyst" }    # icons (or the brand's icon set)
```

`table` (idea-ui's `Table`) is on by default; an app that renders a
`Table` registers it in `register_scene_extensions`
(`table::register(registry)`) — an unregistered payload panics at realize.

## Module layout

```
src/
  main.rs          idealyst::entry!(my_app)
  lib.rs           pub fn app(); register_scene_extensions
  brand.rs         install(): theme pair + restyles     (brand_module recipe)
  shell.rs         the navigator tree + shell chrome    (app_shell_* recipes)
  page_layout.rs   Page / PageHeader / Section          (page_layout recipe)
  routes.rs        every Route const + RouteParams
  screens/         one file per screen: <name>.rs exports one #[component]
  components/      app components used by 2+ screens
  api.rs           data access (the resources' fetchers)
```

- **Screens own their state**: signals and resources are created in the
  screen component. Shared app state (the signed-in user, the active
  workspace) is `provide` / `inject` context set up in `app()`, not
  `thread_local!` globals.
- **A screen file grows components, not helpers**: a piece that takes
  parameters or is used twice becomes a `#[component]` (inline props) so it
  composes in `ui!`.

## `app()` order

```rust
pub fn app() -> Element {
    brand::install();          // 1. theme pair + restyles, before anything builds
    // 2. install_breakpoints(…) only if the brand needs a different wide line
    // 3. app context: provide(Session::new()) …
    shell::root()              // 4. navigators + chrome (+ the one ToastHost)
}
```

## Conventions the recipes assume

- `#[component]` with **inline props**; PascalCase names; bodies in `ui!`.
- Primitives lowercase (`view`, `text`, `scroll_view`), components
  PascalCase (`Button`, `Stack`).
- **Bind handlers before `ui!`** as typed locals —
  `let on_save: Rc<dyn Fn()> = Rc::new(move || …);` — a bare
  `Rc::new(closure)` in a prop position doesn't coerce to `Rc<dyn Fn…>`.
- **Optional callbacks bind only when present**
  (`if let Some(cb) = on_dismiss { … }`), never a no-op closure.
- **Conditional and repeated UI inside `ui!`** (`if`, `match`, `for`), not
  `Vec<Element>` assembled outside it. Reactive lists iterate the signal
  with a key: `for item in items, key = item.id { … }`.
- **Signal props take the narrowest half**: `ReadSignal<T>` to observe,
  `Signal<T>` only for two-way controls.
- Run `idealyst lint` — it catches most idiom drift.
