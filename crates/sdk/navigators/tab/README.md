# `tab-navigator`

The first-party **Tab** navigator — a flat set of co-equal screens the
user switches between, with at most one visible at a time. No push/pop
depth: selecting a tab swaps the active screen. One of the three
first-party navigator SDKs, alongside [`stack-navigator`] and
[`drawer-navigator`]. Like every SDK under `crates/sdk/`, it is **not**
part of `runtime-core` — an app opts in by calling `register` once at
startup.

```rust
use tab_navigator::{TabNavigator, TabsBuilder, TabSpec, TabPlacement};
use runtime_core::{Ref, primitives::navigator::{Route, Screen}};

# fn demo(backend: &mut impl runtime_core::primitives::navigator::RegisterNavigator) {
tab_navigator::register(backend);

let home = Route::<()>::new("home", "/");
let nav: Ref<tab_navigator::TabsHandle> = Ref::new();

let _tree = TabNavigator::new(&home)
    .tab(home.clone(), TabSpec::new("Home").icon("house"), |_| {
        Screen::new(/* body element */)
    })
    .placement(TabPlacement::Bottom)
    .bind(nav.clone());

// From the app's tab bar buttons:
// nav.get().select(&home, ());
# }
```

## What you get

A `TabNavigator::new(initial)` builder, `.tab(route, spec, render)`
registration carrying a `TabSpec` (label / icon / reactive badge), a
`TabPlacement` and `MountPolicy`, and a typed `TabsHandle` (bound via
`.bind(ref)`) with `select(...)`.

## Architecture — the `Element::Navigator` path

The navigator system has two parallel paths: the legacy
`Element::Navigator` / `Element::TabNavigator` / `Element::DrawerNavigator`
variants, and the newer `Element::NavigatorExt`. **This SDK rides the
`Element::Navigator` path** — `TabNavigator::new` produces an
`Element::Navigator` carrying a `TabPresentation` payload, and `register`
installs a per-backend `NavigatorHandler` keyed by it. Selecting a tab
dispatches `NavCommand::Select`; the handler swaps the active screen. The
builder also installs a *link activator* so `Link` primitives inside tab
screens select (not push) by default.

## Per-backend chrome

The author tree is uniform; the rendered chrome differs per backend.
Note: on web/iOS/Android the **tab bar itself is not a navigator
concern** — the navigator owns the screen-swap, and the visible bar is
just a styled `view` the app (or `idea-ui`) renders and wires to
`select`. The `TabSpec` metadata (label, icon, badge) is carried for that
bar to consume.

| Backend | Mechanism |
| --- | --- |
| iOS | Plain `UIView` body that swaps its single child on `Select`; tab bar is author chrome. |
| Android | `FrameLayout` body with a single active-screen child; tab bar is author chrome. |
| macOS | Tab bar (top/bottom per `TabPlacement`) + outlet that swaps on `Select` (no animated transition). |
| Web (wasm32) | Screen-swap; the bar is author chrome wired to `handle.select(...)`. `Select` maps to a `Replace` (no URL-stack growth). |
| terminal | No-op `register` — tabs are not rendered. |
| SSR / any primitive backend | `tab_navigator::chrome::register` builds the outlet from primitives for first paint. |

## Internal glue

The per-platform machinery lives in the internal helper crates pulled in
per target: [`web-navigator-helpers`] (wasm32), [`ios-navigator-helpers`]
(iOS), [`android-navigator-helpers`] (Android). Those are not
author-facing.

## Tests

- `tests/recording.rs` — the runtime-server recording handler (host-side,
  `runtime-server` feature).
- `tests/ssr.rs` — registers the backend-neutral `chrome` handler on the
  SSR backend and checks first-paint markup.

[`stack-navigator`]: ../stack-navigator
[`drawer-navigator`]: ../drawer-navigator
[`web-navigator-helpers`]: ../web-navigator-helpers
[`ios-navigator-helpers`]: ../ios-navigator-helpers
[`android-navigator-helpers`]: ../android-navigator-helpers

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet. The author
tree is uniform; the navigator owns the screen-swap while the visible tab bar is
author chrome (see the table above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p tab-navigator --features runtime-server` — the
  runtime-server recording handler emits the right `Select` wire commands
  (`tests/recording.rs`, host-side only)
- [ ] `cargo test -p tab-navigator` — SSR first-paint markup from the
  backend-neutral `chrome` handler (`tests/ssr.rs`)
- [ ] `cargo build -p tab-navigator --target wasm32-unknown-unknown` — web
  target

**Behavior**
- [ ] **Web** — selecting a tab swaps the active screen (`Select` → `Replace`,
  no URL-stack growth); the author tab bar wired to `handle.select(...)` reflects
  the active tab; badges render.
- [ ] **iOS** — `Select` swaps the single body child; author tab bar drives
  selection; at most one screen visible per `MountPolicy`. ⚠️ not yet
  device-confirmed.
- [ ] **Android** — `FrameLayout` swaps its active-screen child on `Select`. ⚠️
  not yet device-confirmed.
- [ ] **macOS** — tab bar (top/bottom per `TabPlacement`) + outlet swaps on
  `Select` (no animated transition). ⚠️ not yet device-confirmed.
