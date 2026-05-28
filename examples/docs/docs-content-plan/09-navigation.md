# Navigation

Navigation is how your app moves between screens. Idealyst ships
three navigator primitives — **stack**, **tabs**, **drawer** — plus
a **`Link`** primitive that declaratively dispatches navigation
without you wiring up a handle.

All three navigators share a substrate: typed routes, per-screen
reactive scopes, an ambient-navigator stack the `Link` primitive
reads, URL-pattern matching, and a per-screen `ScreenOptions`
bundle for header chrome. What differs is the active-screen
selection UI (a stack on a stack navigator, a tab bar, a slide-in
panel) and the imperative commands the handle exposes.

This page covers all of it: routes and params, declaring screens,
the three navigator kinds, headers, the `Link` primitive, nested
navigators, and the breakpoint behavior the drawer leans on.

## Routes

A route is a typed name plus a URL pattern. You declare each route
once, give it a stable identifier, and reuse it everywhere — on
navigators, in `Link`s, in pushes:

```rust
use runtime_core::Route;

pub const HOME:    Route<()>            = Route::new("home",    "/");
pub const PROFILE: Route<ProfileParams> = Route::new("profile", "/profile/:id");
pub const ABOUT:   Route<()>            = Route::new("about",   "/about");
```

The `name` (first arg) is the in-stack key — what the framework
and native backends use to identify the route. The `path` (second
arg) is the URL pattern web (and any future SSR backend) maps
`window.location` against. Native backends ignore `path`; they
work purely from `name` plus boxed params.

The generic `P` is the typed payload the route carries. For
no-params routes, use `()`. For routes that take data, declare a
struct and implement `RouteParams`:

```rust
use runtime_core::RouteParams;
use std::collections::HashMap;

pub struct ProfileParams {
    pub id: u32,
}

impl RouteParams for ProfileParams {
    fn to_path(&self, _pattern: &str) -> String {
        format!("/profile/{}", self.id)
    }

    fn from_segments(segments: &HashMap<String, String>) -> Option<Self> {
        Some(Self {
            id: segments.get("id")?.parse().ok()?,
        })
    }
}
```

The two methods round-trip your typed struct through a URL on web
and SSR. `to_path` builds the URL when you push the route;
`from_segments` parses it back when the browser's location changes.
Native backends never call either method — they pass the boxed
struct through directly.

### Why params are typed

`nav.push(&PROFILE, ProfileParams { id: 42 })` is a compile-time
check that the params match the route. If you try to push a
mismatched payload, the compiler rejects it. Inside the framework,
the params get boxed into `Box<dyn Any>` for storage, and each
screen builder downcasts back to its declared type before
rendering. There's only one place that downcast can fail — if you
fabricate a `Route<X>` at runtime with the wrong `P` — and the
framework panics with a clear message rather than silently using
the wrong data.

## Screens

A `Screen` is what a route's render closure returns: a primitive
tree plus optional header configuration.

```rust
use runtime_core::{Screen, ui};

fn render_home(_params: ()) -> Screen {
    Screen::new(ui! {
        // ...home page content
    })
    .title("Home")
    .header_shown(true)
}
```

The `Screen::new(...)` builder takes anything that converts to a
`Element`. Chainable methods set per-screen header options:

- `.title("...")` — the title shown in the header bar.
- `.header_shown(bool)` — whether the header is visible.
- `.header_left(HeaderButton::new("back", on_press))` — left slot
  (replaces the default back button if set).
- `.header_right(HeaderButton::new("settings", on_press))` — right
  slot.
- `.header_background(|| my_color())` / `.header_tint(...)` /
  `.title_color(...)` — colors. These are closures so they
  re-evaluate when the active theme changes, retinting the header
  reactively without a screen rebuild.

If you don't need options, return a bare `Element` — the
`Into<Screen>` impl wraps it for you:

```rust
fn render_home(_: ()) -> Element {
    ui! { /* ... */ }
}
```

…and the navigator builder accepts it the same way.

## The stack navigator

`Navigator` is the classic push/pop stack. It's what iOS calls a
`UINavigationController` and Android calls a back-stack-driven
`FragmentManager`. On web, it's an inline subtree swap that
threads through `history.pushState` / `popstate`.

```rust
use runtime_core::{Navigator, Ref};

let nav: Ref<NavigatorHandle> = Ref::new();

ui! {
    Navigator::new(&HOME)
        .screen(HOME,    |_| render_home(()))
        .screen(PROFILE, |p| render_profile(p))
        .screen(ABOUT,   |_| render_about(()))
        .bind(nav)
}
```

The handle's commands:

- `nav.push(&PROFILE, ProfileParams { id: 42 })` — push a new
  screen onto the top.
- `nav.pop()` — pop the top screen.
- `nav.replace(&route, params)` — replace the top of the stack.
- `nav.reset(&route, params)` — clear the stack, mount the new
  route as the root. Useful for post-login redirects.

Each pushed screen runs inside its own reactive `Scope`. Popping
drops the scope — every signal, effect, and ref allocated inside
that screen is freed in one shot. You don't write screen-teardown
code.

### What backends do

- **iOS** — `UINavigationController` with a child `UIViewController`
  per pushed screen. The back-swipe gesture, the slide animation,
  the navigation bar all come from UIKit.
- **Android** — `FrameLayout` driven by `FragmentManager`. Each
  push commits a new `Fragment` and adds it to the back stack so
  the system Back button pops correctly.
- **Web** — an inline container. push/pop swap the active subtree
  atomically. `history.pushState` writes a URL built from
  `params.to_path(route.path())`; `popstate` events drive pops.

## The tab navigator

`TabNavigator` is a tab bar plus a switched content region.

```rust
use runtime_core::{TabNavigator, TabSpec};

let tabs: Ref<TabsHandle> = Ref::new();

ui! {
    TabNavigator::new(&HOME)
        .tab(HOME,    TabSpec::new("Home").icon("house"),       |_| home_screen())
        .tab(SEARCH,  TabSpec::new("Search").icon("magnifyingglass"), |_| search_screen())
        .tab(PROFILE, TabSpec::new("Profile").icon("person"),   |p| profile_screen(p))
        .bind(tabs)
}
```

The `TabSpec` carries the chrome — label, icon, optional reactive
badge:

```rust
TabSpec::new("Messages")
    .icon("envelope")
    .badge(move || {
        let count = unread.get();
        if count == 0 { String::new() } else { count.to_string() }
    })
```

The badge closure runs in an Effect; reading signals subscribes it.
Returning an empty string hides the badge.

### Tab state preservation

How a tab's screen behaves when it's not the active one is
controlled by `MountPolicy`:

- **`LazyPersistent`** (default) — mount the screen the first time
  its tab is activated, then keep it mounted forever. Switching
  away preserves its state (scroll position, nested stack depth,
  form fields). Switching back is instant — the screen is still
  there. Matches React Navigation's default.
- **`EagerPersistent`** — mount every tab's screen at navigator
  creation. Higher memory; tab switches are pure visibility
  toggles. Use this for apps where all tabs are "always live."
- **`LazyDisposing`** — drop the inactive tab's scope on switch.
  Lowest memory; loses state. Use for tabs whose contents are
  cheap to rebuild.

Set it per-tab via `.tab(...).mount_policy(MountPolicy::EagerPersistent)`
or as a navigator default.

### What backends do

- **iOS** — `UITabBarController`. The bar is iOS-rendered (icons
  from SF Symbols by default; you can supply your own).
- **Android** — `BottomNavigationView` (bottom placement) or
  `TabLayout` (top placement), hosting child fragments.
- **Web** — a `<nav role="tablist">` plus a content region. The
  bar's HTML is the framework's; the icons are SVGs.

The handle exposes `tabs.select(&route, params)` for programmatic
switching. Users tap the bar normally.

## The drawer navigator

`DrawerNavigator` is a slide-in side panel plus a switched body
region. Users open the drawer with a hamburger button (or the
platform's edge-swipe gesture); the drawer's content panel renders
whatever navigation UI the design calls for; tapping an entry
switches the body to that entry's screen.

```rust
use runtime_core::{DrawerNavigator, DrawerSide};

let drawer: Ref<DrawerHandle> = Ref::new();

ui! {
    DrawerNavigator::new(&HOME)
        .screen(HOME,    |_| Screen::new(home_page()).title("Home"))
        .screen(LIBRARY, |_| Screen::new(library_page()).title("Library"))
        .screen(SETTINGS,|_| Screen::new(settings_page()).title("Settings"))
        .content(|props| drawer_panel(props))
        .side(DrawerSide::Start)
        .bind(drawer)
}
```

The `content` closure renders the drawer's panel. It receives a
`DrawerContentProps` with the nav callbacks and reactive state, so
the panel can build whatever shape — a list of `Link`s, a brand
header, a settings toggle at the bottom, anything.

Handle commands:

- `drawer.select(&route, params)` — switch the active screen.
- `drawer.open()` / `drawer.close()` / `drawer.toggle()` — control
  the panel.
- `drawer.is_open()` — current state (non-reactive read; subscribe
  to the signal directly if you need reactivity).

### Drawer vs sidebar — the breakpoint behavior

This is the part of the drawer that's interesting cross-platform.

On a phone, the drawer slides over the body — a temporary modal
panel with a scrim. On a tablet, desktop browser, or any wide
viewport, the drawer pins beside the body permanently, becoming
the sidebar. Tapping inside the body doesn't dismiss it because
there's nothing to dismiss; it's just always there.

The framework doesn't make you wire this up. Each backend chooses
based on viewport width:

- **iOS** uses the size class — regular-width = pinned sidebar,
  compact-width = modal drawer. Same posture as
  `UISplitViewController` without adopting that API's opinions.
- **Android** uses `Configuration.screenWidthDp`.
- **Web** uses a CSS media query.

You don't get a knob to override this from app code. The reasoning
is that phone vs tablet adaptation is a *backend* concern: each
platform has conventions about what "tablet" means, and the
backend respects those conventions.

If you genuinely need different layouts at different widths beyond
the drawer's auto-adapt, that's a stylesheet concern — see
[Styles](#).

### `DrawerType` — the animation

Two animation styles:

- **`Front`** — the drawer slides over the body, body stays put,
  scrim dims the body. Material's default; React Navigation's
  `"front"` type.
- **`Slide`** — both drawer and body slide together. The body
  moves to reveal the drawer underneath. iOS-leaning default;
  React Navigation's `"slide"` type.

Set with `.drawer_type(DrawerType::Slide)`. Defaults to the
platform's idiomatic choice.

## The `Link` primitive

Imperative navigation works fine — `nav.push(&route, params)` from
a button's `on_click` — but it has costs:

- You have to thread a `Ref<NavigatorHandle>` through every
  component that needs to navigate.
- The browser's link semantics (right-click "copy link",
  cmd-click new tab, hover URL preview, keyboard activation, the
  `link` accessibility role) need separate wiring.
- Static analysis tooling can't see what links your screens
  expose, because they're hidden inside click handlers.

`Link` solves all three:

```rust
ui! {
    Link(route = &PROFILE, params = ProfileParams { id: 42 }) {
        Text { "Open profile" }
    }
}
```

What you get:

- **Web**: emits a real `<a href="/profile/42">` so the browser's
  link contract works. Right-click, cmd-click, keyboard
  activation, screen readers — all of it.
- **Native**: an invisible tappable wrapper. The press dispatches
  in-process against the captured navigator.
- **Static introspection**: future tooling can extract the link
  graph by walking the primitive tree.
- **No prop drilling**: the `Link` reads the **ambient navigator**
  — the closest enclosing navigator whose `mount_screen` is
  building this subtree — and dispatches through it.

### Picking the right `NavKind`

Activation dispatches a `NavCommand` against the ambient
navigator. The kind picks which command:

- **`Push`** — push the route. Default inside a stack navigator.
- **`Replace`** — replace the top of the stack.
- **`Reset`** — clear the stack and mount the route as the root.
  Useful for post-login redirects.
- **`Select`** — switch active screen without changing stack
  depth. Default inside tabs and drawer navigators.

```rust
Link(route = &HOME, params = (), kind = NavKind::Reset) {
    Text { "Sign out" }
}
```

The constructor picks a default based on the ambient navigator
kind, so `Link(route = ..., params = ...)` (no `kind`) does the
right thing in any context.

## Nested navigators

Navigators nest. A tab navigator at the root with a stack
navigator inside each tab is a common shape:

```rust
ui! {
    TabNavigator::new(&TAB_HOME)
        .tab(TAB_HOME, TabSpec::new("Home"), |_| ui! {
            Navigator::new(&HOME)
                .screen(HOME, |_| home_screen())
                .screen(DETAIL, |p| detail_screen(p))
        })
        .tab(TAB_PROFILE, TabSpec::new("Profile"), |_| ui! {
            Navigator::new(&PROFILE_ROOT)
                .screen(PROFILE_ROOT, |_| profile_root_screen())
                .screen(EDIT_PROFILE, |_| edit_profile_screen())
        })
}
```

A `Link` inside the home tab's `DETAIL` screen targets the home
tab's stack — not the root tabs. The ambient-navigator stack
pushes each navigator's control plane as its screens build, so
`Link` always finds the innermost navigator by default.

If you need to break out — e.g. a "log out" link inside a deeply
nested screen needs to reset the root navigator — capture a
`Ref<NavigatorHandle>` to the outer navigator and call its
imperative methods directly, or use a future `.via(ref)` Link
override.

### Tab state survives switches

Combined with `MountPolicy::LazyPersistent`, nested stacks keep
their state: navigate three levels deep into the Home tab, switch
to Profile, switch back — you're still three levels deep. The
nested stack's screens are still mounted; their signals still
hold their values.

## Headers and theming

Each navigator kind exposes a top-level `.header(...)` helper that
takes a `HeaderStyle` (or a closure producing one):

```rust
DrawerNavigator::new(&HOME)
    .screen(/* ... */)
    .header(|theme: &MyTheme| HeaderStyle {
        background: Some(theme.surface.value().clone()),
        title: Some(theme.text.value().clone()),
        tint: Some(theme.text.value().clone()),
        body_background: Some(theme.background.value().clone()),
    })
```

The closure runs against the *active* theme, so swapping themes
re-tints the bar without a screen rebuild. Per-screen options
(set via `Screen::new(...).title(...).header_left(...)`) layer on
top of the navigator-level defaults.

idea-ui ships an `idea_header(...)` helper that bundles common
patterns. The docs site uses it; you can use it, fork it, or
write your own.

## Scopes and lifecycle, recap

Three lifecycle properties worth keeping in mind:

- **Each mounted screen has its own reactive scope.** When the
  screen unmounts (pop, tab switch with `LazyDisposing`, drawer
  select away from), the scope drops. Every signal, effect, and
  ref allocated inside is freed.
- **Backend nodes survive when their identity is stable.** Hot
  reload preserves a screen's nodes if the screen's place in the
  tree hasn't moved.
- **Navigation state itself is reactive.** Each navigator
  publishes a `NavState` signal bundle that layout closures and
  external code can subscribe to. The drawer's open-state, the
  tab navigator's active index, the stack's depth — all
  observable as signals.

## Where to read more

- [Routes and params](#) — the full `RouteParams` trait, the
  pattern matching algorithm, and the URL ↔ typed-payload story.
- [The `Link` primitive](#) — every prop, every `NavKind`, and
  how the ambient stack works in detail.
- [Layout and chrome](#) — `LayoutPlan` / `LayoutBuilder`, how
  navigators feed into custom layout for web.
- [Hot reload](#) — how navigation state survives source edits.
