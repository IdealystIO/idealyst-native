+++
title = "App structure and navigation"
summary = "Choosing the shell (tabs ⇄ sidebar, drawer ⇄ sidebar), the navigator tree (swap for sections, stack for drill-down), route naming, active state, back behavior and deep links — wired with SwapNavigator, StackNavigator and idea-ui-nav."
when = "Starting an app, adding a top-level section, or deciding how a user gets from one screen to another."
order = 20
tags = ["navigation", "shell", "tabs", "sidebar", "drawer", "stack", "swap", "routes", "deep-links"]
+++
# App structure and navigation

Navigation is the app's information architecture made visible. Decide it
before designing screens: what are the top-level sections, what drills
down, and what's a task layered over the current screen.

## Three kinds of movement

| Movement | Tool | Example |
| --- | --- | --- |
| Between **co-equal sections** (no "back") | `SwapNavigator` (`swap-navigator`) | Inbox / Projects / Settings |
| **Down into** a thing and back out | `StackNavigator` (`stack-navigator`) | Projects → Atlas → Atlas settings |
| A **task over** the current screen | `Modal` (not a route, unless it must be linkable) | New project, confirm delete, filters |

Sections are a swap; each section that drills down owns a stack (nested
inside the swap's screen). Navigators nest freely — each `.layout(…)`
wraps only its own outlet.

## Choosing the shell

| The app has… | Phone | Wide (≥ `Lg`) | Recipe |
| --- | --- | --- | --- |
| 2–5 sections, used daily, consumer or mobile-first | bottom `TabBar` | pinned sidebar | `app_shell_tabs` |
| 3–9 sections, tool / admin / desktop-first | drawer behind a menu button (`AppShell`) | pinned sidebar (`AppShell`) | `app_shell_sidebar` |
| 1 main surface + settings | no shell: a stack with a settings entry in the header | same | `list_detail_adaptive` |
| > 9 destinations | group them: sidebar sections with headings, or a second-level nav inside a section | | |

House rules:

- **Order sections by frequency**, most used first. The first is the
  default route (`/`).
- **Every section has an icon and a one-word label.** Icon-only nav is for
  a collapsed rail only, never a phone tab bar.
- **Settings and account go last** (bottom of the sidebar, last tab or the
  account menu) — they're visited rarely.
- **The active section is always visible**: `ListItem(active = …)` in a
  sidebar, `TabBar`'s indicator in a tab bar. Derive it from the
  navigator's `active_route` — never track it in a separate signal.
- **Don't put the same destination in two places.** A screen reachable
  from both the tab bar and a menu confuses "where am I".

## Routes

- **Name = what it is, path = how it reads**: `Route::<()>::new("projects", "/projects")`,
  `Route::<ProjectId>::new("project", "/projects/:id")`. Names are stable
  identifiers (`on_select("projects")`); paths are user-visible URLs on web.
- **Typed params** via `RouteParams` (`to_path` / `from_segments`). A
  detail route takes an id, never a whole object — the screen loads its
  own data, so a deep link works.
- **Every param is user input.** A detail screen whose id doesn't resolve
  renders a "not found" page with a way back, never panics.
- **Swap routes are unit-param** (`Route<()>`): `TabBar`/`on_select`
  switch by route *name* and can't fill path segments.
- Deep links (`[package.metadata.idealyst.app.links]`) route automatically;
  if the URL scheme is sound, links need no extra code.

## Back behavior

- A stack's back is `nav.pop` + `can_go_back`; `StackHeader(show_back = nav.can_go_back, on_back = Some(nav.pop.clone()))`.
- **One visible title**: a pushed screen's title lives in the stack bar
  (`Screen::new(…).title("Atlas")`), so its page renders
  `Page(header = false)`. A section's root screen hides the bar
  (`.hide_header(true)`) because its `Page` header is the title.
- Header actions: at most one, as `HeaderButton::text("Edit")` via
  `.header_right(…)`. More actions go in the page.
- Desktop has no back gesture: never hide the back button below `Lg` *or*
  on desktop. In a wide split view the list replaces it.
- Unsaved changes: `Screen::back_enabled(false)` while dirty, plus a
  confirm `Modal` on the drawn back button.

## Wiring rules (idea-ui-nav)

- Import the builder traits with the navigator
  (`use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator}`,
  `use stack_navigator::{StackBuilder, StackHandle, StackNavigator, StackScreenExt}`),
  or `.screen`/`.layout` won't resolve.
- The outlet is one-shot: bind `let outlet = nav.outlet;` and place it
  **once, in a position that never changes** across breakpoints. Chrome
  (bars, panes) appears and disappears *around* it.
- Splat the outlet bare inside a column that fills its parent; it brings
  its own `flex: 1 1 0; min-height: 0`.
- `AppShell` builds the sidebar once; close the drawer after a nav press
  only when it's a drawer: `if !sidebar_pinned(PIN_AT) { drawer_open.set(false) }`.
- Bars get safe areas: the header slot `safe_area = SafeAreaSides::TOP`,
  a bottom tab bar `safe_area = SafeAreaSides::BOTTOM`.
- `MountPolicy::LazyPersistent` (swap default) keeps each visited section
  alive — switching tabs preserves scroll and input. Use `LazyDisposing`
  only for sections that are cheap to rebuild and should start fresh.
- Navigate imperatively through the bound handle (`nav.get().map(|h| h.push(&DETAIL, params))`);
  for in-tree links use `link(route = &R, params = p) { … }`.

## Planning output

When planning navigation, produce:

1. The section list in priority order, each with icon + label.
2. The shell choice and the reason (from the table above).
3. The navigator tree with every route name and path.
4. Which screens push details, and which tasks are modals.
5. The phone and wide presentation of each (read_design_skill("adaptive")).
