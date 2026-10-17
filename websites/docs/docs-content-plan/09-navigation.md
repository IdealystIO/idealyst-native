# Navigation

Navigation is how your app moves between screens. Idealyst ships two
navigator behaviors as SDK crates — **`swap-navigator`** (a flat set of
co-equal screens the user switches between) and **`stack-navigator`**
(push/pop with depth and a back-stack) — plus a **`link`** primitive that
dispatches navigation without you wiring up a handle.

There is no tab navigator and no drawer navigator. A tab bar or a drawer
panel is ordinary author layout wrapped around the navigator's single
**outlet** — the analog of react-router's `<Outlet/>`. A tab UI is a swap
navigator whose `.layout(|nav| …)` puts a bar under `{nav.outlet}`; a
drawer UI is the same navigator with the outlet wrapped in a side panel.
`idea_ui_nav` ships themed chrome for the common shapes: `TabBar`,
`Drawer`, `AppShell` (pinned sidebar ⇄ drawer) and `StackHeader`.

Both navigators share a substrate: typed routes, per-screen reactive
scopes, an ambient-navigator stack `link` reads, URL-pattern matching,
and web URL sync. What differs is the command vocabulary — `Select` for
swap, `Push` / `Pop` / `Replace` / `Reset` for stack — and whether
covered screens exist below the visible one.

## Routes

A route is a typed name plus a URL pattern. Declare each route once and
reuse it on navigators, in links, and in pushes:

```rust
use runtime_core::Route;

pub const HOME:    Route<()>            = Route::new("home",    "/");
pub const PROFILE: Route<ProfileParams> = Route::new("profile", "/profile/:id");
pub const ABOUT:   Route<()>            = Route::new("about",   "/about");
```

The `name` (first arg) is the in-navigator key that the framework and
chrome components use. The `path` (second arg) is the URL pattern web and
SSR match `window.location` against. Native backends ignore `path`; they
work from `name` plus boxed params.

`P` is the typed payload the route carries. Use `()` for no params; for
data, declare a struct and implement `RouteParams`:

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

`to_path` builds the URL when you navigate; `from_segments` parses it
back when the browser location changes (back/forward, deep links).
Native backends pass the boxed struct through and call `from_segments`
only to open a deep link.

### Why params are typed

`nav.push(&PROFILE, ProfileParams { id: 42 })` is checked at compile
time. Inside the framework the params are boxed into `Box<dyn Any>`, and
each screen builder downcasts back to its declared type. The downcast can
only fail if you fabricate a `Route<X>` at runtime with the wrong `P`,
and then the framework panics with a clear message.

## Screens

A `Screen` is what a route's render closure returns: an element plus
optional per-screen options.

```rust
use runtime_core::{Screen, ui};
use stack_navigator::StackScreenExt;

fn render_home(_params: ()) -> Screen {
    Screen::new(ui! {
        // ...home page content
    })
    .title("Home")
}
```

For stack screens, `StackScreenExt` adds header options:

- `.title("...")` — the screen title. The author's `StackHeader` shows
  it on every backend.
- `.header_left(HeaderButton::text("Edit").on_press(...))` — leading
  header slot.
- `.header_right(...)` — trailing header slot.
- `.hide_header(true)` — hide the header for this screen.
- `.back_enabled(false)` — while this screen is on top, iOS's
  swipe-back and Android's system Back don't pop it (an imperative
  `pop` still does; at the root, Back still leaves the app). Web and
  desktop have no back gesture to lock.
- `.fullscreen(true)` — full-screen (system bars hidden) while this
  screen is on top, on iOS and Android. The stack calls
  `set_fullscreen` only when the top screen's request changes, so your
  own calls stand between full-screen screens.

Swap screens draw no navigator chrome, so they take no options. A bare
`Element` works too — `Into<Screen>` wraps it.

## The swap navigator

`SwapNavigator` manages a flat set of co-equal screens. Selecting a
screen swaps the one visible screen in the outlet. It is the behavior
behind tab UIs, sidebar UIs, wizards and site shells.

```rust
use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};
use idea_ui_nav::{TabBar, TabItem};
use runtime_core::{ui, Ref, Screen};

let nav: Ref<SwapHandle> = Ref::new();

let builder = SwapNavigator::new(&HOME)
    .screen(HOME,    |_| Screen::new(home_page()))
    .screen(SEARCH,  |_| Screen::new(search_page()))
    .screen(PROFILE, |p: ProfileParams| Screen::new(profile_page(p)))
    .layout(|nav| ui! {
        view(style = fill_column) {
            view(style = grow) { { nav.outlet } }
            TabBar(
                items = vec![
                    TabItem::new("home", "Home"),
                    TabItem::new("search", "Search"),
                    TabItem::new("profile", "Profile"),
                ],
                active_route = nav.active_route,
                on_select = nav.on_select,
            )
        }
    });

ui! { builder.bind(nav) }
```

The layout closure receives a `SwapContext`:

- `nav.outlet` — splat it once, in one stable spot. It is a one-shot,
  non-`Clone` value and cannot sit inside a reactive `if` / `when`.
- `nav.active_route` — `Signal<&'static str>`, the active route name.
- `nav.active_path` — the active screen's path.
- `nav.on_select` — switch to a sibling screen by route name.

`.bind(...)` fills a `Ref<SwapHandle>`; `handle.select(&PROFILE,
ProfileParams { id: 42 })` builds the URL and dispatches `Select`.
Selecting the active screen is a no-op. Without `.layout(...)` the outlet
fills the navigator with no chrome.

### MountPolicy

Set navigator-wide with `.mount_policy(...)`:

- `LazyPersistent` (default) — mount on first select, then keep it.
  Switching away preserves scroll, nested stack depth and form state.
- `EagerPersistent` — mount every screen up front.
- `LazyDisposing` — drop the inactive screen's scope on switch and
  remount fresh on return. On web, a rebuilt section re-reads the
  address bar, so nested navigators come back on the screen the URL names.

## The stack navigator

`StackNavigator` is the push/pop sibling. The visible screen is the top
of the stack, swapped into the same single outlet. Chrome is author
layout here too — typically a `StackHeader` above the outlet:

```rust
use stack_navigator::{
    header_state, StackBuilder, StackHandle, StackNavigator, StackScreenExt,
};
use idea_ui_nav::StackHeader;
use runtime_core::{rx, ui, Ref, Screen};

let nav: Ref<StackHandle> = Ref::new();

let builder = StackNavigator::new(&LIST)
    .screen(LIST, |_| Screen::new(list_page()).title("Inbox"))
    .screen(DETAIL, |p: DetailParams| Screen::new(detail_page(p)).title("Detail"))
    .layout(|nav| {
        let screen_chrome = nav.screen_chrome;
        let state = rx!(header_state(&screen_chrome));
        ui! {
            view(style = fill_column) {
                StackHeader(
                    state = state,
                    show_back = nav.can_go_back,
                    on_back = Some(nav.pop.clone()),
                )
                view(style = grow) { { nav.outlet } }
            }
        }
    });

ui! { builder.bind(nav) }
```

`StackHandle` commands: `push(&route, params)`, `pop()` (no-op at the
root), `replace(&route, params)`, `reset(&route, params)`. The
`_with_state` verbs live on the underlying `NavHandle`:
`nav.inner().push_with_state(&route, params, state)`, and likewise
`replace_with_state`, `reset_with_state`, `select_with_state`.

`StackContext` carries the same `outlet` / `active_route` / `active_path`
as swap, plus `depth`, `can_go_back`, `pop`, and `screen_chrome` (read it
with `stack_navigator::header_state` inside `rx!`).

Each screen runs in its own reactive scope. Popping drops the scope and
frees every signal, effect and ref inside it.

### Stack retention

Covered screens follow `StackRetention`:

- `Retain` — covered screens stay alive; pop reveals them intact. The
  default everywhere except web.
- `Rebuild` — covered screens are disposed on push; pop re-mounts the
  revealed screen from its URL. The default on web, and how cold-start
  deep links work: the synthesized parent never mounts until you pop to it.
- `PlatformDefault` — the default: `Rebuild` on web, `Retain` elsewhere.

Force either with `.retention(...)`. A native stack presenter forces
`Retain`, because the platform container keeps covered screens on screen.

## Screen state

Route params say which screen you are on (`/items/5`); screen state says
how it is configured (tab, filter, search). State travels as query
params, so it survives reload, sharing, and back.

```rust
use runtime_core::{screen_state, QueryParams, ScreenState};

#[derive(Clone, Debug, Default, PartialEq)]
struct Filters { tab: String, page: u32 }

impl ScreenState for Filters {
    fn to_query(&self) -> QueryParams {
        QueryParams::new()
            .with("tab", self.tab.clone())
            .with("page", self.page.to_string())
    }
    fn from_query(q: &QueryParams) -> Option<Self> {
        Some(Filters {
            tab: q.get("tab").unwrap_or("all").to_string(),
            page: q.get_as("page").unwrap_or(1),
        })
    }
}

// Navigating writes /inbox?tab=starred&page=2
nav.inner().push_with_state(&INBOX, (), Filters { tab: "starred".into(), page: 2 });

// Reading, inside the screen builder:
.screen(INBOX, |_| {
    let filters = signal(screen_state::<Filters>().unwrap_or_default());
    // …
})
```

- Query params never change route identity, so a query change updates
  state without remounting. To react to it, read the `query` signal on
  the navigator's world context — `inject::<StackNav>()` or
  `inject::<SwapNav>()` from `runtime_vocabulary::prims`. The layout's
  `StackContext` / `SwapContext` do not carry it.
- Use `nav.inner().replace_with_state(...)` for filter changes so back
  leaves the screen instead of stepping through every filter value.
- A route pattern can carry defaults: `Route::new("inbox",
  "/inbox?tab=unread")`.

On backends with no address bar the URL round-trip is a no-op; the
in-memory path is the same.

## Chrome — idea-ui-nav

- `TabBar` — wire `active_route = nav.active_route` and
  `on_select = nav.on_select`.
- `Drawer` — a side panel around the outlet with a tap-to-close scrim.
  `is_open` is a `Signal<bool>` the author owns.
- `AppShell` — pinned sidebar on wide screens, drawer on narrow ones.
  The sidebar builds once; pinned vs drawer is breakpoint styling that
  compiles to real `@media` rules on web and SSR. Configure `is_open`,
  `pin_at` (default `Breakpoint::Lg`) and `width`; `sidebar_pinned(pin_at)`
  tells you whether it is pinned.
- `StackHeader` — the header for stack screens on every backend. It
  renders nothing when the screen set `hide_header`. The native bar is
  always hidden, so this is the only header.

## What backends do

Both SDKs lower to the vocabulary's built-in `swap_navigator` /
`stack_navigator` handlers, installed by `register_builtins` on every
host (including SSR/SSG and the runtime-server recorder). The handlers
use the ordinary node ops every backend has, so there is no per-backend
navigator code and nothing for an app to register.

- **Web** — URL sync is automatic: `Select` and `Push` write
  `history.pushState` entries, back/forward reconcile into nav commands,
  cold deep links seed the right screen, and scroll offsets restore on
  back.
- **iOS / Android** — the stack SDK installs a native presenter. Inside
  the outlet lives a real `UINavigationController` (swipe-back, native
  transitions) or, on Android, a view-level container
  (`RustStackPresenter`) that plays Material's shared-axis transition and
  takes system Back while more than one screen is stacked. Chrome is still
  the author's `StackHeader`.

## Deep links

A link from outside the app (`myapp://settings/about`,
`https://example.com/settings/about`) opens the screen at its path,
whether it launched the app or arrived while running. Declare links
under `[package.metadata.idealyst.app.links]`; the `deep-link` SDK
(`on_link`, `intercept`, `route_link`) lets you see, hold, or replay them.

## The `link` primitive

```rust
ui! {
    link(route = &PROFILE, params = ProfileParams { id: 42 }) {
        text { "Open profile" }
    }
}
```

- **Web** — a real `<a href="/profile/42">`: right-click, cmd-click,
  keyboard activation and screen readers all work.
- **Native** — a tappable wrapper that dispatches in-process.
- **No prop drilling** — `link` reads the ambient navigator, the closest
  enclosing navigator whose screen is building the subtree.

Inside a swap navigator a link dispatches `Select`; inside a stack it
dispatches `Push`.

## Nested navigators

Put one navigator inside another's screen. A drawer shell at the root,
tabs in one section and a stack inside a tab is three navigators deep
(the `examples/nav-showcase` structure). A `link` targets the innermost
navigator; to reach an outer one, capture its handle `Ref` and call it
directly. With `LazyPersistent`, a nested stack keeps its depth when you
switch tabs and back.

## Sizing

The navigator root fills its container (width/height 100% +
`flex-grow: 1`), and a style-less `{nav.outlet}` is a bounded flex region
(`flex: 1 1 0` + `min-height: 0`), so screens and scroll views fill with
no configuration. Override either by styling it: `.style(s)` on the
`SwapNavigator` / `StackNavigator` builder replaces the root's default,
and `navigator_outlet().style(s)` in the layout, in place of
`{nav.outlet}`, replaces the outlet's.

## Where to read more

- [Primitives](primitives) — where the navigator and outlet sit among
  the primitives.
- [Writing a backend](writing-a-backend) — a backend needs nothing
  navigator-specific.
- [Styles](styles) — the breakpoints `AppShell` pins on.
