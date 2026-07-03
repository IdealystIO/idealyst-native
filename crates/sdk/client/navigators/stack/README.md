# `stack-navigator`

The first-party **Stack** navigator — push/pop screens with a native
header bar and the platform's back gesture. A stack owns an ordered
stack of screens: pushing a route slides a new screen in on top; popping
(or iOS swipe-back / the browser back button) returns to the one
beneath. One of the three first-party navigator SDKs, alongside
[`tab-navigator`] and [`drawer-navigator`]. Like every SDK under
`crates/sdk/`, it is **not** part of `runtime-core` — an app opts in by
calling `register` once at startup.

```rust
use stack_navigator::{Navigator, StackBuilder, StackScreenExt, BarButton};
use runtime_core::{Ref, primitives::navigator::{Route, Screen}};

# fn demo(backend: &mut impl runtime_core::primitives::navigator::RegisterNavigator) {
stack_navigator::register(backend);

let home = Route::<()>::new("home", "/");
let nav: Ref<stack_navigator::StackHandle> = Ref::new();

let _tree = Navigator::new(&home)
    .screen(home.clone(), |_| {
        Screen::new(/* body element */)
            .title("Home")
            .header_right(BarButton::new("ellipsis", || { /* open menu */ }))
    })
    .bind(nav.clone());

// Later, from an event handler:
// nav.get().push(&details, DetailsParams { id: 7 });
// nav.get().pop();
# }
```

## What you get

A `Navigator::new(initial)` builder, fluent `.screen(...)` registration,
per-screen options via the `StackScreenExt` trait (`title`,
`header_left`, `header_right`, header colors, `unmount_on_blur`,
`back_enabled`), and a typed `StackHandle` (bound via `.bind(ref)`) that
drives the stack imperatively: `push`, `pop`, `replace`, `reset`,
`depth`.

`back_enabled(false)` is a **full back-lock**: the iOS swipe-back +
chevron and the Android edge-swipe + system back button are all
suppressed while that screen is on top (handy for a canvas or carousel
that owns the edge gesture). Imperative `StackHandle::pop` still works.
No-op on web (browsers don't allow disabling the back button) and on
backends with no system back affordance.

## Architecture — the `Element::Navigator` path

The navigator system has two parallel paths in the framework: the legacy
`Element::Navigator` / `Element::TabNavigator` / `Element::DrawerNavigator`
variants, and the newer `Element::NavigatorExt`. **This SDK rides the
`Element::Navigator` path** — `Navigator::new` produces an
`Element::Navigator` carrying a `StackPresentation` payload, and
`register` installs a per-backend `NavigatorHandler` keyed by that
presentation type. The framework walker mounts the path-matched screen
and routes push/pop/replace/reset to the handler, which drives the
native chrome.

## Per-backend chrome

The author tree is uniform; each backend renders the equivalent native
push/pop stack:

| Backend | Mechanism |
| --- | --- |
| macOS / Windows / Linux desktop (terminal) | Minimalist single-screen outlet — no chrome, no animation. |
| iOS | `UINavigationController`; a delegate reconciles interactive swipe-back. |
| Android | `FragmentManager` back-stack inside a `RustNavigator` host. |
| macOS (window) | Single-window outlet that swaps its child on each command (no animated push/pop). |
| Web (wasm32) | SPA router — `history.pushState` per push, the browser back button drives pop; one screen mounted at a time. |
| SSR / any primitive backend | `stack_navigator::chrome::register` builds the header from `view` + `text` primitives for first paint. |

Per the framework's *native-first* convention, header chrome (title, bar
buttons, colors) is configured through **screen options**
(`StackScreenOptions` via `StackScreenExt`) and navigator-level builder
methods — never the `style` system.

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

[`tab-navigator`]: ../tab-navigator
[`drawer-navigator`]: ../drawer-navigator
[`web-navigator-helpers`]: ../web-navigator-helpers
[`ios-navigator-helpers`]: ../ios-navigator-helpers
[`android-navigator-helpers`]: ../android-navigator-helpers

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet. The author
tree is uniform; each backend renders its own native push/pop chrome (see the
table above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p stack-navigator --features runtime-server` — the
  runtime-server recording handler emits the right push/pop/replace/reset wire
  commands (`tests/recording.rs`, host-side only)
- [ ] `cargo test -p stack-navigator` — SSR first-paint markup from the
  backend-neutral `chrome` handler (`tests/ssr.rs`)
- [ ] `cargo test -p stack-navigator --features robot --test robot_screen_tree`
  — a navigator's screen content is captured by `Robot::snapshot()`
- [ ] `cargo build -p stack-navigator --target wasm32-unknown-unknown` — web
  target

**Behavior**
- [ ] **Web** — push slides a new screen in (`history.pushState`); the browser
  back button pops; back-stack depth correct; `back_enabled(false)` is a no-op
  (browsers can't disable back).
- [ ] **iOS** — `UINavigationController` push/pop animates with the native
  header; interactive swipe-back reconciles; `back_enabled(false)` suppresses
  swipe-back + chevron while imperative `pop` still works. ⚠️ not yet
  device-confirmed.
- [ ] **Android** — `FragmentManager` back-stack push/pop; system back button +
  edge-swipe pop; `back_enabled(false)` locks both. ⚠️ not yet
  device-confirmed.
- [ ] **macOS** — single-window outlet swaps its child on each push/pop command
  (no animated transition). ⚠️ not yet device-confirmed.
