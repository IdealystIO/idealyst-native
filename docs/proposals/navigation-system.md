# Proposal: navigation system — tabs, drawers, stacks

> **Status: implemented, in a different shape than this proposal
> sketched.** The proposal extended the single stack `Navigator` into a
> family of navigator *kinds* — a stack, a `TabNavigator`, a
> `DrawerNavigator` — each with its own `Element` variant, callbacks
> bundle, and native widget (`UITabBarController`, a hand-rolled drawer).
> That is not what was built. What shipped is **two navigator SDKs** —
> `stack-navigator` (`crates/sdk/client/navigators/stack`: push / pop /
> replace / reset, native back stack, swipe-back, browser History) and
> `swap-navigator` (`crates/sdk/client/navigators/swap`: flat Select, one
> visible screen, no back stack) — on a shared substrate in
> `crates/runtime/shared/src/primitives/navigator/` and
> `crates/runtime/vocabulary/src/prims/navigator.rs`. **Tabs and drawers are
> author layout, not navigator kinds**: a `.layout(...)` shell wraps the
> navigator's `outlet` and renders the tab bar or drawer chrome around it
> (`idea-ui-nav` ships ready-made `AppShell`, `TabBar`, `Drawer`,
> `StackHeader` components). Nesting — a drawer whose body is tabs whose
> tabs are stacks — is navigators inside screens. The author-facing
> reference is the `navigation` MCP guide
> (`crates/mcp/catalog/guides/navigation.md`).

What survives from the original proposal is the reference model it was
measured against.

## Reference model: React Navigation

React Navigation's mental model is the starting point because it's what
every mobile developer already maps onto:

- **Stack** — push/pop, native back stack, swipe-to-back on iOS, system
  back button on Android. → `stack-navigator`.
- **Tabs** — a tab bar (bottom on phones, top or sidebar on tablets/web)
  where each tab owns its own subtree; only the active tab is visible. →
  `swap-navigator` plus a `TabBar` in the layout shell.
- **Drawer** — a slide-in side panel with route entries, optionally
  fixed-open on wide screens (a sidebar). The body is whatever route the
  drawer last selected. → `swap-navigator` plus a `Drawer` in the layout
  shell.
- **Nested navigators** — the shape of a real app. React Navigation
  composes these by nesting; so does idealyst.

The screens are the same in every case — a render closure that takes typed
params and produces an `Element` subtree. What differs is how the navigator
decides which screen is active and how the user moves between them: the
stack's `Push`/`Pop`/`Replace`/`Reset` vocabulary versus the swap
navigator's `Select`.

## Non-goals

- **Replacing React Navigation 1:1.** The navigators cover the use cases
  React Navigation set as the baseline, but the API stays in idealyst's
  voice — typed routes, declarative `link`, `Ref`-driven imperative
  handles, ambient capture — with no `createNativeStackNavigator()`-style
  factory functions.
