+++
title = "Navigation"
order = 50
tags = ["navigation"]
+++

# Navigation

Idealyst ships a navigator system that maps the platform-native navigation chrome (`UINavigationController`, Fragment back-stack, browser History) to a single author API.

## The two navigator SDKs

- **`stack-navigator`** — push/pop depth: `push` mounts a screen on a back stack, `pop` reveals the one below. Native swipe-back on iOS, system back on Android, browser History on web.
- **`swap-navigator`** — flat Select: one visible screen, no back stack. This is the substrate for **tab** and **drawer** experiences — the tab bar / drawer chrome is author layout (idea-ui-nav ships ready-made `AppShell`, `TabBar`, `Drawer`, `StackHeader` components). There is no separate tab- or drawer-navigator crate.

Both share the same builder shape (`::new(&ROOT)` + `.screen(...)` + `.layout(...)` + `.bind(...)`); they differ only in command vocabulary (Push/Pop/Replace/Reset vs Select) and screen retention.

Per [[native_first_layout_for_web]], per-screen chrome data (title, header buttons, back-lock) rides the navigator **screen options** (`.title(...)`, `.header_right(...)` — not the `style` system). The `.layout(...)` builder is the author shell that wraps the navigator's outlet and renders that data; the same shell renders on every backend, and on mobile the native bar machinery consumes the same screen-option slots.

## Building a stack navigator

Navigators are built with a fluent **builder** — there is no `ui!` tag sugar for the stack navigator. `ui! { StackNavigator { Screen(name = ...) } }` **does not compile**; routes are typed consts and screens are registered with `.screen(route, |params| ...)`:

```rust
use runtime_core::primitives::navigator::HeaderButton;
use runtime_core::{component, ui, Element, Ref, Route, Screen};
// BOTH extension traits must be in scope: `StackBuilder` provides
// `.screen(...)` / `.layout(...)` / `.bind(...)` on the builder, and
// `StackScreenExt` provides `.title(...)` / `.header_right(...)` on `Screen`.
// Omitting `StackBuilder` fails with "no method named `screen`".
use stack_navigator::{
    header_state, StackBuilder, StackContext, StackHandle, StackNavigator, StackScreenExt,
};

const HOME: Route<()> = Route::<()>::new("home", "/");
const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");

#[component]
pub fn app() -> Element {
    // Filled by the framework at mount; screen closures capture it and
    // read it when a press actually happens.
    let nav: Ref<StackHandle> = Ref::new();

    let builder = StackNavigator::new(&HOME)
        .screen(HOME, move |_| Screen::new(home_page(nav)).title("Home"))
        .screen(DETAIL, |_| {
            Screen::new(detail_page())
                .title("Detail")
                .header_right(HeaderButton::text("Edit").on_press(|| {}))
        });

    ui! { builder.bind(nav) }
}
```

Navigate imperatively through the bound handle:

```rust
nav.get().map(|h| h.push(&DETAIL, ())).unwrap_or_default(); // pushes /detail
// h.pop() / h.replace(...) / h.reset(...) are the other commands.
```

**Typed route params**: a route can carry a typed payload that also round-trips through the URL. Declare `const NOTE: Route<NoteId> = Route::<NoteId>::new("note", "/note/:slug");`, implement `RouteParams` for `NoteId` (`to_path` fills `:slug`; `from_segments` parses it back), and the `.screen(NOTE, |params: NoteId| ...)` closure receives the typed value — including on a web cold load of `/note/<slug>`. Pushing with the wrong param shape is a compile error.

The compile-checked **`stack_two_screens` recipe** (visible in `list_recipes` once `stack-navigator = { workspace = true }` is in your `Cargo.toml`) is the full list + detail skeleton with a typed param and a layout shell — copy-paste it as a starting point.

## Writing a `.layout(...)` shell

`.layout(|nav: StackContext| ...)` supplies the author chrome that wraps the navigator's single **outlet** (the slot where the active screen mounts). The closure gets reactive nav state (`nav.active_route`, `nav.can_go_back`, `nav.depth`, `nav.screen_chrome`) plus `nav.pop` and the one-shot `nav.outlet`. Three traps:

1. **Splat the outlet bare — don't wrap it in a styled view.** The outlet ships its own fill rules (`flex: 1 1 0; min-height: 0` — see `outlet_fill_rules` / `screen_flow_fill_rules` in `runtime-core`'s `primitives/navigator/shared.rs`), which are what make the active screen absorb the remaining height and let grow-based screens (`flex_grow` canvases, fill-the-viewport editors) actually fill. A wrapper view with its own style replaces those rules and collapses such screens to content height.

2. **A braced splat directly after a component call parses as that component's children.** Inside `ui!`, a PascalCase component call may take an optional `{ ... }` children block — so this attaches the outlet to the header instead of making it a sibling:

   ```rust
   // WRONG — `{ nav.outlet }` becomes StackHeader's children block:
   ui! {
       view(style = shell) {
           StackHeader(state = state, show_back = nav.can_go_back, on_back = Some(nav.pop.clone()))
           { nav.outlet }
       }
   }
   ```

   Put the component call inside its own `view { ... }` slot (a primitive's children block is closed by its braces, so what follows is an ordinary sibling splat):

   ```rust
   // RIGHT — the header slot view closes, `{ nav.outlet }` is a sibling:
   ui! {
       view(style = shell) {
           view(style = header_slot) {
               StackHeader(state = state, show_back = nav.can_go_back, on_back = Some(nav.pop.clone()))
           }
           { nav.outlet }
       }
   }
   ```

3. **`StackHeader` roots in a growing `Surface` (`grow = 1.0`)** — dropped directly into a column shell it flex-grows and splits the viewport with the outlet. Keep it in a non-growing slot: the `header_slot` wrapper above should set `flex_grow: 0` / `flex_shrink: 0` (i.e. `flex: 0 0 auto`) so only the outlet grows. This wrapper does double duty with trap 2.

Derive the header's data from the active screen's slots — the same data the native bar renders on mobile: `let state = rx!(header_state(&nav.screen_chrome));` feeds `idea_ui_nav::StackHeader`, and `nav.can_go_back` gates the back affordance (`nav.pop` is a no-op at the root).

## Layout vs sidebar reactive scopes

If you wire layout or sidebar contents reactively, keep a keepalive `Effect` alive past the `build_*_navigator` return (see [[navigator_scope_keepalive]]). Without it the reactive scope is dropped and updates stop firing.

## Drawer responsiveness

The drawer navigator should switch between modal (mobile) and pinned (desktop) based on the active theme breakpoint, not a magic pixel threshold ([[drawer_pinned_above_smell]]). Read [[current_breakpoint]] and let the theme own the threshold.
