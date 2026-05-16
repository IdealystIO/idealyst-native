# Proposal: navigation system v2 — Tab, Drawer, Stack

Today the framework ships a single `Navigator` primitive — a stack
navigator. It's well-built: typed routes, ambient-nav capture for the
`Link` primitive, per-screen reactive scopes, native UIKit /
FragmentManager integration, an opt-in `.layout(...)` slot for web
chrome. What it doesn't have is the rest of the navigator vocabulary
that React Navigation set as the de-facto baseline for mobile UIs:
tabs, side drawers, nested combinations.

This proposal sketches **how to extend the existing `Navigator` into a
family of navigator kinds** without rebuilding the type-erased screen
table, the `Route<P>` machinery, ambient capture, or the `Link`
primitive. They all stay; we add two new navigator kinds alongside the
stack one and a third "composite" pattern for the
tabs-of-stacks-with-a-drawer-on-top layouts mobile apps actually ship.

> Status: proposal. Not implemented.

---

## Reference model: React Navigation

React Navigation's mental model is the right starting point because
it's what every mobile developer already maps onto:

- **Stack Navigator** — what we have. Push/pop, native back stack,
  swipe-to-back on iOS, system back button on Android.
- **Tab Navigator** — a tab bar (usually bottom on phones, top or
  sidebar on tablets/web) where each tab owns its own subtree. Tabs
  preserve state on switch; only the active tab's content is visible.
- **Drawer Navigator** — a slide-in side panel with route entries.
  Optionally fixed-open on wide screens (becomes a sidebar). The body
  is whatever route the drawer last selected.
- **Nested navigators** — the actual shape of a real app: a drawer
  whose body is a tab bar whose tabs each contain a stack. React
  Navigation composes these by nesting. We should too.

The screens themselves are the same in all three cases — a render
closure that takes typed params and produces a `Primitive` subtree.
What differs is **how the navigator decides which screen is active and
how the user moves between them**.

That's the seam: we already have screen registration, screen mounting,
per-screen scopes, and the typed-route machinery as a shared
substrate. The thing that varies between navigator kinds is small —
the "active-screen-selection UI" and the dispatch shape (`push`/`pop`
vs `select`/`select` vs `open`/`close`).

---

## Surface (author-facing)

The headline: the existing `Navigator` keeps its name and shape (it
*is* the stack navigator). Two new navigator builders sit alongside
it, sharing the same `Route<P>` + `.screen(...)` registration syntax.

### Stack — unchanged

```rust
let home = Route::<()>::new("home", "/");
let detail = Route::<DetailParams>::new("detail", "/detail/:id");

ui! {
    Navigator(initial = &home) {}
        .screen(home, |_| ui! { HomeScreen() })
        .screen(detail, |p| ui! { DetailScreen(id = p.id) })
}
```

(This is what's there today. The examples below show what's *new*.)

### Tabs

```rust
let inbox  = Route::<()>::new("inbox", "/inbox");
let search = Route::<()>::new("search", "/search");
let me     = Route::<()>::new("me", "/me");

ui! {
    TabNavigator(initial = &inbox) {}
        .tab(inbox,  TabSpec::new("Inbox",  Icon::Inbox),  |_| ui! { Inbox() })
        .tab(search, TabSpec::new("Search", Icon::Search), |_| ui! { Search() })
        .tab(me,     TabSpec::new("Me",     Icon::User),   |_| ui! { Profile() })
}
```

`TabSpec` is presentation metadata — label, icon, optional badge. The
*screen registration* (route + render closure) is identical to stack's
`.screen(...)`; `.tab(...)` is just `.screen(...)` plus that extra
metadata.

Tabs preserve state on switch. Each tab's subtree mounts once, into
its own per-screen `Scope`, and stays mounted while the tab is hidden
(more on the lazy-mount toggle below).

The default placement is **bottom on phones, top on web/tablet** —
inferable from the backend. Override:

```rust
TabNavigator::new(&inbox)
    .placement(TabPlacement::Top)  // or Bottom, Sidebar
    .tab(...)
```

### Drawer

```rust
let home    = Route::<()>::new("home", "/");
let library = Route::<()>::new("library", "/library");
let settings = Route::<()>::new("settings", "/settings");

ui! {
    DrawerNavigator(initial = &home) {}
        .item(home,     DrawerItem::new("Home",     Icon::Home))
        .item(library,  DrawerItem::new("Library",  Icon::Library))
        .item(settings, DrawerItem::new("Settings", Icon::Settings))
        .screen(home,     |_| ui! { HomeBody() })
        .screen(library,  |_| ui! { LibraryBody() })
        .screen(settings, |_| ui! { SettingsBody() })
}
```

`.item(...)` registers a drawer entry (label, icon); `.screen(...)`
registers what to render when that route is active. The split mirrors
how `.tab(...)` rolls metadata + screen into one call — for the
drawer, splitting them is more useful, because authors often want the
same route reachable from a drawer *and* deep-linkable from elsewhere.

Width breakpoint for the "fixed sidebar on tablets" behavior:

```rust
DrawerNavigator::new(&home)
    .pinned_above(900)  // px; default disables the pinned mode (always overlay)
```

Above the breakpoint the drawer is always-visible (a sidebar); below,
it's an off-canvas overlay with a hamburger trigger the navigator
exposes via its layout slot.

### Nesting (the real shape)

```rust
ui! {
    DrawerNavigator(initial = &main) {}
        .item(main,     DrawerItem::new("App",      Icon::App))
        .item(settings, DrawerItem::new("Settings", Icon::Settings))
        .screen(main, |_| ui! {
            TabNavigator(initial = &feed_root) {}
                .tab(feed_root,  TabSpec::new("Feed",  Icon::Home), |_| ui! {
                    Navigator(initial = &feed) {}
                        .screen(feed,   |_| ui! { Feed() })
                        .screen(post,   |p| ui! { Post(id = p.id) })
                })
                .tab(notifs_root, TabSpec::new("Notifs", Icon::Bell), |_| ui! {
                    Navigator(initial = &notifs) {}
                        .screen(notifs, |_| ui! { Notifications() })
                })
        })
        .screen(settings, |_| ui! { SettingsScreen() })
}
```

This is the canonical "drawer of tabs of stacks" mobile app. Each
nested navigator is a self-contained subtree; the ambient-navigator
stack (already there) gives each one its own `Link` target.

`Link(&post, PostParams { id: 7 })` placed inside a `Feed()` screen
captures the innermost ambient navigator — the stack — and pushes
within that stack, exactly like today.

---

## How the framework changes

### One `Primitive` variant — or three?

The cleanest answer is **three**, mirroring the runtime shapes:

```rust
pub enum Primitive {
    // existing
    Navigator(Box<navigator::Navigator>),

    // new
    TabNavigator(Box<navigator::tabs::TabNavigator>),
    DrawerNavigator(Box<navigator::drawer::DrawerNavigator>),
    // …
}
```

Why not a single `Navigator { kind: NavKind, … }`? Because the per-kind
data is genuinely different:

- Stack carries an ordered stack of mounted screens.
- Tabs carries a set of "always-mounted" (or lazy-mounted) screens
  plus a single active id signal.
- Drawer carries the same kind of "select-active-by-id" state as Tabs,
  but also drawer-open-state, optional pinned breakpoint, and
  drawer-side metadata.

Squashing them into one variant means every backend `match` arm has to
inspect `kind` and demux internally, which is exactly the work three
variants eliminate. The cost — three backend creation functions
instead of one — is negligible compared to the readability win.

### Shared substrate

The bits that *are* genuinely shared move out of the stack-specific
module into a `primitives::navigator::shared` module (or just stay
where they are and get re-exported):

- `Route<P>`, `RouteParams`, `match_pattern` — unchanged.
- `ScreenBuilder`, `RouteEntry` — unchanged.
- `AmbientNavGuard`, `ambient_navigator()` — unchanged. All three
  navigator kinds push their `Rc<NavigatorControl>` onto the same
  ambient stack while building screens.
- `NavCommand`, `NavigatorControl`, `NavState` — extended (below).
- `NavigatorHandle` — generalized (below).

### `NavCommand` extensions

Today's commands are stack-shaped: `Push`, `Pop`, `Replace`, `Reset`.
We add tab/drawer-shaped commands:

```rust
pub enum NavCommand {
    // stack
    Push    { name, url, params },
    Pop,
    Replace { name, url, params },
    Reset   { name, url, params },

    // tabs / drawer
    Select  { name, url, params },     // switch the active id
    OpenDrawer,
    CloseDrawer,
    ToggleDrawer,
}
```

Each navigator kind's dispatcher handles the commands it understands
and panics on the ones it doesn't (matches the existing "handle used
before mount = silently drop" posture, but a kind mismatch is a
programming error worth surfacing).

The `Link` primitive doesn't need to know which command shape its
ambient navigator uses; we add a `NavKind::Select` variant alongside
the existing `Push`/`Replace`/`Reset`. A link inside a tab or drawer's
screen targets that navigator with the right command kind, configured
either via `.kind(NavKind::Select)` or — better — inferred from the
ambient navigator's type when the link is constructed.

Implementation note: store the dispatch shape on `NavigatorControl`
itself (e.g. a `Vec<NavCommand>` of the commands it accepts) so the
link constructor can pick its default `NavKind` based on what the
captured ambient supports. Stack → `Push`, Tabs → `Select`, Drawer →
`Select`.

### `NavigatorHandle` generalization

The current handle is stack-shaped. We split it:

```rust
pub struct StackHandle  { inner: NavigatorHandle }      // current API
pub struct TabsHandle   { inner: NavigatorHandle }
pub struct DrawerHandle { inner: NavigatorHandle }
```

…where `NavigatorHandle` becomes the "any nav, dispatches commands"
inner, and the wrappers expose only the methods that make sense for
their kind:

```rust
impl TabsHandle {
    pub fn select<P: RouteParams>(&self, route: &Route<P>, params: P);
    pub fn active(&self) -> &'static str;
}

impl DrawerHandle {
    pub fn select<P: RouteParams>(&self, route: &Route<P>, params: P);
    pub fn open(&self);
    pub fn close(&self);
    pub fn toggle(&self);
    pub fn is_open(&self) -> bool;
}
```

`.bind(...)` on each navigator builder takes the matching ref type,
so `Ref<StackHandle>` can't accidentally be `.open()`'d.

### Per-screen mount lifetimes — the lazy/eager toggle

Stack mounts a screen on push and drops its scope on pop. That's
natural. Tabs and Drawer have a real choice to make:

- **Eager + persistent** — all registered screens mount on navigator
  creation and stay mounted forever. Switching tabs is just visibility
  swapping. State preserved trivially, no rebuild cost on switch,
  highest memory.
- **Lazy + persistent** — a screen mounts the first time it's
  activated, then stays mounted. Most apps want this. Memory grows
  monotonically with explored screens; switch cost is one mount on
  first visit, zero after.
- **Lazy + dispose-on-hide** — drop the inactive screen's scope on
  switch. Cheap memory; expensive (and state-losing) switches. Useful
  for memory-tight scenarios but a footgun by default.

Default to **lazy + persistent**, expose the others via builder:

```rust
TabNavigator::new(&inbox)
    .mount_policy(MountPolicy::EagerPersistent)  // or LazyPersistent (default), LazyDisposing
    .tab(...)
```

The framework already has the machinery: each screen runs in its own
`Scope`, and `release_screen(scope_id)` is the dispose call. The
walker hands the backend a per-screen `(N, u64)` pair so the backend
can keep the node alive across reactive rebuilds; the navigator's
dispatcher decides when to release.

### Web-side: URL, history, deep linking

The stack navigator already maps push/pop onto `history.pushState` /
`popstate`. Tabs and Drawer have a question to answer: **does a tab
switch / drawer item selection produce a URL change?**

The answer should be **yes by default**, because that's the whole
point of the `Route<P>` + path machinery — deep linking. Selecting a
tab uses `history.pushState` with the tab's route path. The Back
button then traverses the *tab history* (which is exactly how every
web app with a tab UI behaves correctly).

Drawer item selection works the same. Opening / closing the drawer
itself does *not* change the URL — it's a transient UI state, not
navigation.

A future builder method (`.url_strategy(UrlStrategy::None)`) can opt
out for tabs that should be ephemeral, but the default matches user
expectations.

### Native-side: the right widget per kind

- **iOS Tabs**: `UITabBarController`. Each tab is a child VC; the tab
  bar handles selection, swipe, scrollEdgeAppearance. We don't draw
  it; iOS does.
- **iOS Drawer**: there is no native UIKit drawer. We render one
  ourselves — a `UIView` overlay that slides in, with a tap-outside
  recognizer to dismiss. On `regular`-width size classes (iPad), pin
  the drawer beside the content (matches the system's
  `UISplitViewController` posture without going through that API,
  which has too much opinion baked in for our use).
- **Android Tabs**: a `BottomNavigationView` (or `TabLayout` for top
  placement) hosting fragments. FragmentManager handles the
  back stack.
- **Android Drawer**: `DrawerLayout` + `NavigationView`. Standard.
- **Web Tabs**: a `<nav role="tablist">` plus a content region. The
  active tab's screen sits in the content region; inactive screens'
  DOM nodes are detached or `display: none` per the mount policy.
- **Web Drawer**: a `<aside>` plus a content region. At
  `pinned_above` widths, the aside is always visible; below, it's
  positioned off-canvas with a transform transition and a focus trap
  while open.

The shared interface stays at the backend trait boundary — same
shape as today's `create_navigator(callbacks)`, but with
`create_tab_navigator(callbacks)` and `create_drawer_navigator(callbacks)`
siblings, each receiving a kind-specific callbacks bundle.

---

## What the new callbacks bundles look like

Reuse most of `NavigatorCallbacks<N>` (initial route, mount_screen,
release_screen, match_path, nav_state, depth_changed). Per-kind
extensions:

### `TabNavigatorCallbacks<N>` adds

- `tabs: Vec<TabRegistration>` — ordered list (label, icon, route
  name, badge signal) so the backend can build the tab bar.
- `placement: TabPlacement`.
- `mount_policy: MountPolicy`.
- `active_changed: Rc<dyn Fn(&'static str)>` — backend notifies the
  framework when the user taps a tab.

### `DrawerNavigatorCallbacks<N>` adds

- `items: Vec<DrawerItemRegistration>` — same shape as tabs minus
  badge.
- `side: DrawerSide` (Start / End).
- `pinned_above: Option<u32>`.
- `mount_policy: MountPolicy`.
- `open_changed: Rc<dyn Fn(bool)>` — backend notifies on open/close.
- `active_changed: Rc<dyn Fn(&'static str)>`.

Both kinds reuse the existing `nav_state.active_route` / `active_path`
signals — they describe the active screen identity in the same shape
across all navigator kinds, which means a top-bar component that
displays "the current screen's title" works inside any of them.

---

## What `Link` does inside each kind

The `Link` primitive captures the ambient `Rc<NavigatorControl>`
unchanged. The only difference is which command it dispatches by
default:

| Ambient navigator | Default `Link` command |
|-------------------|------------------------|
| Stack             | `Push`                 |
| Tabs              | `Select`               |
| Drawer            | `Select`               |

This is configured by storing a `default_kind: NavKind` on the
`NavigatorControl` itself; each navigator sets its own when wiring up
the control plane. Authors who need to override (e.g. "this link
inside a tab should push a new stack screen in the parent stack")
chain `.via(parent_nav_ref)` — that builder method is already in the
link proposal.

---

## What the layout slot becomes

Stack's `.layout(...)` gives authors a chrome-rendering hook for the
web (top bar, back button, breadcrumbs). The pattern generalizes:

- Tabs probably doesn't need a layout slot — the tab bar is the
  chrome. We can add one later if we discover use cases (e.g.
  rendering a top app bar that *spans* tabs).
- Drawer almost certainly *does* want one, for the same reason stack
  does — the host app wants control over the toolbar that contains
  the hamburger trigger.

Implementation: factor the existing layout-slot machinery
(`LayoutBuilder`, `LayoutProps`, `LayoutPlan<N>`,
`NavigatorCallbacks::build_layout`) into the shared substrate so each
navigator kind can opt in. Tabs starts opted-out; Drawer opts in with
an extended `LayoutProps` that also exposes `on_drawer_toggle` and an
`is_drawer_open: Signal<bool>`.

Native backends keep ignoring the layout slot — UIKit's tab bar
controller and Android's `DrawerLayout` already own their chrome.

---

## Phasing

Roughly the order I'd land this:

1. **Refactor.** Pull the kind-shared bits of `primitives::navigator`
   into a `navigator::shared` submodule. Rename the file-level
   `Navigator` to `StackNavigator` *internally* (the `Primitive`
   variant stays `Navigator` for API stability), and move
   stack-specific code under `navigator::stack`. No new features, just
   set the room up. (Small PR, mostly mechanical.)

2. **`NavCommand::Select` + `NavigatorControl::default_kind`.** Wire
   up the link primitive to use the ambient navigator's default kind.
   Stack still works unchanged (its default is `Push`). No new
   navigators yet — this just unblocks (3) and (4).

3. **`TabNavigator`.** Smallest of the new kinds — no overlay
   geometry, no breakpoint logic. Lands as its own `Primitive`
   variant, its own backend hook, its own handle type. Validate the
   shared substrate is actually shared.

4. **`DrawerNavigator`.** Same shape as (3) but with the drawer-open
   state and the pinned-above breakpoint. The platform-native
   implementations are bigger (DrawerLayout on Android, hand-rolled
   on iOS, off-canvas CSS on web).

5. **Nesting validation.** Build a real example app — drawer of tabs
   of stacks — and shake out the ambient-nav + link-default-kind
   wiring under nesting. Most likely outcome: a few bugs around which
   navigator the `<Link>` inside a tab targets when there's a stack
   wrapping the tab navigator outside. Worth a dedicated pass.

6. **Polish.** `idea-ui` components for `TabNavigator` and
   `DrawerNavigator` (themed tab bar with active indicator, themed
   drawer with material/iOS variants), the `MountPolicy` controls,
   the URL-strategy opt-out, the `pinned_above` breakpoint.

(1) and (2) are prerequisites for the others; (3) and (4) can ship in
either order; (5) and (6) come last.

---

## Open questions

- **Animation.** Stack gets push/pop animation from UIKit/FragmentManager
  for free. Tabs and Drawer on web need animation policy decisions —
  fade vs slide on tab change? Spring vs linear on drawer slide?
  Default to no animation initially; expose hooks later.
- **Drawer overlay semantics on iOS.** Hand-rolling means we have to
  pick a gesture model. iOS apps vary wildly — some have edge-swipe
  open, some have drag-to-close, some have neither. Start with
  hamburger-only and add gestures behind a builder flag.
- **Tab switching while in a nested stack.** When the user is two
  screens deep into Tab A's stack and switches to Tab B, what happens
  when they switch back? React Navigation preserves the stack depth.
  We should too — falls out naturally from "tabs are persistent" but
  needs an explicit test.
- **Where does `MountPolicy::LazyDisposing` matter?** Probably never as
  a *default*, but it'd be a hammer for memory-tight scenarios
  (long-lived background tabs holding heavy state). Worth shipping but
  not promoting.

---

## Non-goals

- **Modal / overlay navigators.** Modals are a fundamentally
  different shape — they're not "switch the active screen," they're
  "push a screen on top of the current navigator." Today's
  `Overlay` primitive handles transient overlays; full modal
  navigator semantics (presentation styles, dismissal gestures) are
  a separate proposal.
- **Bottom-sheet navigators.** Same answer. The bottom-sheet
  primitive already exists conceptually; sequencing screens
  *through* a bottom sheet is a follow-up.
- **Replacing React Navigation 1:1.** We're not aiming for surface
  parity. The navigator kinds we add should *cover the use cases*
  React Navigation set as the baseline, but the API stays in our
  voice — typed routes, declarative `Link`, `Ref`-driven imperative
  handles, ambient capture. We won't be shipping
  `createNativeStackNavigator()`-style factory functions.
