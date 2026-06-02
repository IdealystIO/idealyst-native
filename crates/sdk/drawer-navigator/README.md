# `drawer-navigator`

The first-party **Drawer** navigator — a hamburger side panel that
switches between co-equal screens, responsive between a modal
(off-canvas) drawer on narrow viewports and a pinned (in-flow) sidebar on
wide ones. Structurally "tabs with a side panel": one screen visible at a
time, switched via the sidebar, plus an open/close state for the panel.
One of the three first-party navigator SDKs, alongside [`stack-navigator`]
and [`tab-navigator`]. Like every SDK under `crates/sdk/`, it is **not**
part of `runtime-core` — an app opts in by calling `register` once at
startup.

```rust
use drawer_navigator::{DrawerNavigator, DrawerBuilder, DrawerScreenExt, DrawerSide};
use runtime_core::{Ref, primitives::navigator::{Route, Screen}};

# fn demo(backend: &mut impl runtime_core::primitives::navigator::RegisterNavigator, sidebar: runtime_core::Element) {
drawer_navigator::register(backend);

let home = Route::<()>::new("home", "/");
let nav: Ref<drawer_navigator::DrawerHandle> = Ref::new();

let _tree = DrawerNavigator::new(&home)
    .screen(home.clone(), |_| Screen::new(/* body */).title("Home"))
    .sidebar(sidebar)
    .drawer_width(280.0)
    .side(DrawerSide::Start)
    .bind(nav.clone());

// nav.get().toggle();              // open/close the panel
// nav.get().select(&settings, ()); // switch screen
# }
```

## What you get

A `DrawerNavigator::new(initial)` builder, `.screen(...)` registration,
two sidebar/chrome APIs (see below), drawer geometry (`drawer_width`,
`side`, `drawer_type`, `swipe_to_open`), `MountPolicy`, header styling,
and a typed `DrawerHandle` (bound via `.bind(ref)`) with `select`,
`open`, `close`, `toggle`, `is_open` / `is_open_signal`.

### Two sidebar/chrome APIs

- **Original:** `.sidebar(element)` / `.sidebar_with(closure)` supply the
  side panel as one closure receiving `DrawerSlotProps`.
- **Next-gen slot system:** `.leading_with(...)` / `top_with` /
  `bottom_with` / `trailing_with` mount persistent chrome around the
  screen outlet, each receiving the uniform `SlotProps`. Both coexist;
  the handler prefers the leading slot when both are set. Slot chrome
  mounts **once** at init and survives screen swaps.

## Architecture — the `Element::Navigator` path

The navigator system has two parallel paths: the legacy
`Element::Navigator` / `Element::TabNavigator` / `Element::DrawerNavigator`
variants, and the newer `Element::NavigatorExt`. **This SDK rides the
`Element::Navigator` path** — `DrawerNavigator::new` produces an
`Element::Navigator` carrying a `DrawerPresentation` payload, and
`register` installs a per-backend `NavigatorHandler` keyed by it.
Selecting a nav-link dispatches `NavCommand::Select`; opening/closing the
panel rides `NavCommand::Custom` carrying a `DrawerCmd`.

## Per-backend chrome

Uniform author tree, native-equivalent rendering:

| Backend | Mechanism |
| --- | --- |
| iOS | `UIView` body wrapped in a self-owned `UINavigationController` (native header) + a sidebar `UIView` that slides in. |
| Android | `RustExactFrameLayout` wrapping a `RustDrawerLayout` (androidx `DrawerLayout`) with a body `LinearLayout` + Toolbar. |
| macOS | Single-window, persistent always-visible sidebar; outlet swaps on `Select` (no scrim / slide). |
| Web (wasm32) | Flex column: optional top, a middle row [sidebar? · body-outlet · trailing?], optional bottom. Modal⇄pinned collapse is a pure CSS `@media` query keyed to `navigator_pin_width` — same for live web and SSR. |
| terminal | Persistent sidebar column beside the outlet; no animation / scrim / open-close. |
| SSR / any primitive backend | `drawer_navigator::chrome::register` builds the same `ui-nav-drawer-*` layout from primitives for a flash-free first paint. |

Per the framework's *native-first* convention, header chrome is
configured through screen options (`DrawerScreenOptions`) and builder
methods, not `style`. With `.native_header(false)` the app owns its
header at the page level for an identical look across backends.

## Internal glue

The per-platform machinery lives in the internal helper crates pulled in
per target: [`web-navigator-helpers`] (wasm32), [`ios-navigator-helpers`]
(iOS), [`android-navigator-helpers`] (Android). Those are not
author-facing.

## Tests

- `tests/recording.rs` — the runtime-server recording handler (host-side,
  `runtime-server` feature). Includes the regression that a fresh
  navigator carries the default fill style so the native container
  doesn't collapse.
- `tests/ssr.rs` — registers the backend-neutral `chrome` handler on the
  SSR backend and checks first-paint markup.

[`stack-navigator`]: ../stack-navigator
[`tab-navigator`]: ../tab-navigator
[`web-navigator-helpers`]: ../web-navigator-helpers
[`ios-navigator-helpers`]: ../ios-navigator-helpers
[`android-navigator-helpers`]: ../android-navigator-helpers
