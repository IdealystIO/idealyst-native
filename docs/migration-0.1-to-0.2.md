# Migrating navigation from 0.1.x to 0.2

0.2 reworks navigation around **two primitives** and a **react-router-style
outlet**. This guide shows how to move 0.1 apps over. The old `tab` / `drawer` /
`stack` navigators still compile in 0.2, so you can migrate incrementally.

## What changed, in one paragraph

There used to be three navigator kinds (`tab`, `drawer`, `stack`), each owning
its own chrome. 0.2 collapses that to **two behaviors**:

- **Swap** — depth-less, one-of-N screens, `Select`. Needs no native surface.
  **Subsumes tab and drawer.**
- **Stack** — push/pop depth, keeps a back-stack.

And **chrome becomes author layout**. A navigator now exposes a single
**outlet** (`{nav.outlet}`); *you* wrap it. A "tab bar" is a bar you draw around
the outlet; a "drawer" is an `idea-ui-nav` component around the outlet. This is
why `tab` and `drawer` are no longer navigator *kinds* — they're layout.

### New crates

| Purpose | Crate |
| --- | --- |
| Swap navigator (replaces tab + drawer) | `swap-navigator` |
| Stack navigator (outlet model) | `stack-navigator-v2` |
| Themed nav chrome (TabBar / Drawer / StackHeader) | `idea-ui-nav` |

Navigators **self-register** at backend construction (`inventory`), so an app's
`register_extensions` can stay a no-op — the same as the 0.1 nav SDKs.

Working examples: [`crates/sdk/client/navigators/swap/examples/swap-demo`](../crates/sdk/client/navigators/swap/examples/swap-demo) (3-tab router) and
[`crates/sdk/client/navigators/stack-v2/examples/stack-demo-v2`](../crates/sdk/client/navigators/stack-v2/examples/stack-demo-v2) (push/pop with header slots).

---

## Tab navigator → Swap + `TabBar`

The tab bar is now author layout. Wrap `{nav.outlet}` in an
`idea_ui_nav::TabBar`, wired to the navigator's `active_route` / `on_select`.

**Before (0.1):**

```rust
use tab_navigator::{TabNavigator, TabSpec, TabsBuilder, TabsHandle};

TabNavigator::new(&HOME)
    .tab(HOME, TabSpec::new("Home").icon("house"), |_| Screen::new(home()))
    .tab(SEARCH, TabSpec::new("Search"), |_| Screen::new(search()))
    .placement(TabPlacement::Bottom)
    .bind(nav);
```

**After (0.2):**

```rust
use swap_navigator::{SwapNavigator, SwapBuilder, SwapHandle};
use idea_ui_nav::{TabBar, TabItem};

SwapNavigator::new(&HOME)
    .screen(HOME, |_| Screen::new(home()))
    .screen(SEARCH, |_| Screen::new(search()))
    .layout(|nav| ui! {
        view {                       // your own column: outlet grows, bar at bottom
            { nav.outlet }
            TabBar(
                items = vec![TabItem::new("home", "Home"), TabItem::new("search", "Search")],
                active_route = nav.active_route,
                on_select = nav.on_select,
            )
        }
    })
    .bind(nav);
```

Notes:
- Tab *ids* are the **route names** (`TabItem::new("home", …)` matches
  `Route::new("home", …)`). Highlighting tracks `nav.active_route`.
- A `Link` inside a swap screen dispatches `Select` automatically (no push).
- Switch imperatively with `nav.get().select(&HOME, ())` (was `.select(...)`).
- Want the bar on top, a side rail, icons/badges? It's your layout now — draw it.

---

## Drawer navigator → Swap + `Drawer`

The drawer panel is now an `idea_ui_nav::Drawer` component that **owns its own
`is_open`** — there's no `DrawerCmd` / `Custom` command any more, and no
`is_open` on the navigator.

**Before (0.1):**

```rust
use drawer_navigator::{DrawerNavigator, DrawerBuilder, DrawerHandle};

DrawerNavigator::new(&HOME)
    .screen(HOME, |_| Screen::new(home()).title("Home"))
    .screen(INBOX, |_| Screen::new(inbox()).title("Inbox"))
    .sidebar_with(|slot| sidebar(slot))     // slot.on_select / slot.on_close
    .drawer_width(280.0)
    .bind(nav);
```

**After (0.2):**

```rust
use swap_navigator::{SwapNavigator, SwapBuilder, SwapHandle};
use idea_ui_nav::{Drawer, DrawerSide};

let open = signal(false);   // the drawer's open state lives with YOU now

SwapNavigator::new(&HOME)
    .screen(HOME, |_| Screen::new(home()))
    .screen(INBOX, |_| Screen::new(inbox()))
    .layout(move |nav| ui! {
        Drawer(
            sidebar = sidebar(&nav, open),   // your links call nav.on_select(route) then open.set(false)
            is_open = open,
            side = DrawerSide::Start,
            width = 280.0,
        ) {
            { nav.outlet }
        }
    })
    .bind(nav);
```

Notes:
- A hamburger button in your layout does `open.set(true)`; sidebar links do
  `nav.on_select(route)` then `open.set(false)` (auto-close is your call now,
  not a navigator command).
- The `Drawer` slides in over the content with a tap-to-close scrim (built from
  `overlay`/`presence`), so it works the same on every backend.

---

## Stack navigator → `stack-navigator-v2`

Stack keeps push/pop, but chrome moves out of the navigator. **Header is now
per-screen slot data** (`title` / `header_left` / `header_right` / `hide_header`)
that you render however you like:

- **Mobile (planned):** the same slots drive the **native** `UINavigationController`
  bar / Android `Toolbar` (native back + swipe-back).
- **Web/desktop:** you place an `idea_ui_nav::StackHeader` in your `.layout()`,
  fed by the slots — or draw nothing.

`StackHeader` **self-suppresses** when a native bar is rendering the header, so
you write it once and it does the right thing per platform.

**Before (0.1):**

```rust
use stack_navigator::{Navigator, StackBuilder, StackHandle, StackScreenExt, BarButton};

Navigator::new(&HOME)
    .screen(HOME, |_| Screen::new(home()).title("Home"))
    .screen(DETAIL, |p: DetailParams| {
        Screen::new(detail(p))
            .title("Detail")
            .header_right(BarButton::new("edit", || { /* … */ }))
    })
    .bind(nav);
```

**After (0.2):**

```rust
use stack_navigator_v2::{
    StackNavigator, StackBuilder, StackHandle, StackScreenExt, header_state,
};
use runtime_core::primitives::navigator::HeaderButton;
use idea_ui_nav::StackHeader;

StackNavigator::new(&HOME)
    .screen(HOME, |_| Screen::new(home()).title("Home"))
    .screen(DETAIL, |p: DetailParams| {
        Screen::new(detail(p))
            .title("Detail")
            .header_right(HeaderButton::text("Edit").on_press(|| { /* … */ }))
    })
    .layout(|nav| ui! {
        view {
            StackHeader(
                state = rx!(header_state(&nav.screen_chrome)),  // the active screen's slots
                show_back = nav.can_go_back,
                on_back = Some(nav.pop.clone()),
            )
            { nav.outlet }
        }
    })
    .bind(nav);
```

Notes:
- `push` / `pop` / `replace` / `reset` on `StackHandle` are unchanged.
- `BarButton` → `HeaderButton` (`HeaderButton::text("Edit")` /
  `HeaderButton::icon("edit")`, then `.on_press(...)`).
- `.hide_header(true)` on a screen hides the header (native bar hidden on mobile;
  `StackHeader` renders nothing on web).
- Lower screens stay mounted beneath the top, so pop-back restores their state
  (unchanged from 0.1).

---

## Composition (drawer over a stack)

The point of the split: a swap/drawer (author chrome) can wrap an outlet that
hosts a **stack** with its own standard headers. On mobile the stack renders its
native bar *inside* the drawer's outlet; on web each draws its own header.

```rust
// Outer: a swap navigator with a Drawer around the outlet.
// One of its screens builds an inner StackNavigator (native/standard header).
```

---

## Cheatsheet

| 0.1 | 0.2 |
| --- | --- |
| `tab_navigator::TabNavigator` | `swap_navigator::SwapNavigator` + `idea_ui_nav::TabBar` |
| `drawer_navigator::DrawerNavigator` | `swap_navigator::SwapNavigator` + `idea_ui_nav::Drawer` |
| `stack_navigator::Navigator` | `stack_navigator_v2::StackNavigator` |
| `TabsHandle::select` / drawer `select` | `SwapHandle::select` |
| navigator-owned drawer `is_open` / `open()`/`close()` | your `Signal<bool>` + `Drawer(is_open = …)` |
| `BarButton::new(icon, cb)` | `HeaderButton::icon(name).on_press(cb)` |
| stack `.title()` (built-in chrome) | stack `.title()` (slot) + `StackHeader` / native bar |
| chrome owned by navigator | chrome is your `.layout(\|nav\| … { nav.outlet } … )` |

Component sources:
[TabBar](../crates/ui/idea-ui-nav/src/tab_bar.rs) ·
[Drawer](../crates/ui/idea-ui-nav/src/drawer.rs) ·
[StackHeader](../crates/ui/idea-ui-nav/src/stack_header.rs)
