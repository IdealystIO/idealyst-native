+++
title = "Screen Layouts: idea-ui-layout"
order = 47
tags = ["layout", "responsive", "screens", "idea-ui", "header", "toolbar", "sidebar", "sheet", "dialog"]
+++

# Screen Layouts: idea-ui-layout

idea-ui decides what a control looks like. `idea-ui-layout` decides how a
screen is put together at each width: you pick a layout, hand it content,
and it draws the phone, tablet and desktop form. A screen built this way
never reads the breakpoint and writes no breakpoint overlay for gutters,
stacking or column counts.

This guide is how to build with it. The rules each component enforces are
in `crates/ui/idea-ui-layout/RULES.md`, and a working app with one screen
per layout is `crates/ui/idea-ui-layout/examples/layout-demo` (run it with
`idealyst dev --web` from that folder).

## The widths

| | Phone (under 640) | Tablet (640 to 1023) | Desktop (1024 up) |
|---|---|---|---|
| Content | phone layout | desktop layout | desktop layout |
| Sidebar | drawer | drawer | pinned, collapsible |
| A pane beside the content | never | from the layout's own width | yes |
| Menus and popovers | bottom sheet | anchored | anchored |
| Form dialogs | bottom sheet | card | card |

A screen has two layouts, never three: a tablet is the desktop layout with
the sidebar off screen.

## Setup

```toml
[dependencies]
idea-ui = { workspace = true }
idea-ui-layout = { workspace = true }
icons-lucide = { workspace = true }
```

Install the idea theme as usual ([[theming]]). The kit's own tokens (the
page gutter) have defaults and need no install; see "Changing the gutter"
below to override them.

## Step 1: the app frame

One `AppFrame` wraps the whole app. It owns the sidebar: pinned on a
desktop, a drawer on a tablet and a phone.

Make one `SidebarState` and hand the same value to the frame, to every
`SidebarItem` and to every header's `SidebarToggle`. It is `Copy`.

```rust
use idea_ui_layout::{AppFrame, FrameSurface, SidebarCollapse, SidebarItem, SidebarState};

pub fn app() -> Element {
    install_idea_theme(light_theme());
    let sidebar = SidebarState::new(SidebarCollapse::Icons);
    let route = signal(Route::Projects);
    ui! {
        AppFrame(
            state = sidebar,
            surface = FrameSurface::Raised,
            sidebar = vec![ui! {
                Stack(gap = StackGap::Xs, padding = StackPadding::Sm) {
                    SidebarItem(
                        sidebar = sidebar,
                        icon = Some(FOLDER),
                        label = "Projects".to_string(),
                        active = rx!(route.get() == Route::Projects),
                        on_press = to_projects.clone(),
                    )
                    SidebarItem(
                        sidebar = sidebar,
                        icon = Some(USERS),
                        label = "Workers".to_string(),
                        active = rx!(route.get() == Route::Workers),
                        on_press = to_workers.clone(),
                    )
                }
            }],
        ) {
            // The screen on show: a navigator's outlet, or a `match` over
            // your own route signal.
            { nav.outlet }
        }
    }
}
```

Two choices are made here, once, for the whole app:

- **`surface`**: `FrameSurface::Flat` puts the sidebar and the screen on one
  surface with a rule between them. `Raised` puts the screen on a card and
  the sidebar on the ground behind it. The card is only drawn where the
  sidebar is pinned. It takes a signal if you want to switch it live.
- **`SidebarCollapse`**: what the pinned sidebar becomes when collapsed.
  `Off` (it does not collapse), `Hidden` (it goes away), or `Icons` (a rail
  of icons). On a tablet and a phone the sidebar is always a drawer with
  full labels; the rail is a desktop form only.

`SidebarItem` draws icon and label when open and the icon alone, with the
label as its tooltip, in the rail. Give it a `count` for a destination
with something waiting: `count = Some(SidebarCount::new(open_tasks,
tone::Neutral))` (a signal, an `rx!` or a plain number) draws the number
at the row's end and a dot on the glyph in the rail, and nothing at zero. Sidebar content of your own (a logo, a
footer) can read `sidebar.is_rail()` to adapt.

## Step 2: choose a layout for the screen

Ask in order and stop at the first yes.

1. A canvas being edited, with settings for what is selected?
   **Workbench**: a `Blank` page holding `SplitView(mode = Inspector)`.
2. A sequence of steps a person finishes? **`Flow`**.
3. A set of destinations with one open (settings sections)?
   **`SplitView(mode = Nav)`**.
4. One table of records? **A `Table` page**.
5. Lays itself out edge to edge by its own rules (a board, a map)?
   **A `Blank` page**.
6. Otherwise **a `Content` page**. A record's detail screen is one; with a
   companion panel it holds a `SplitView(mode = Detail)`.

Every screen mounts its own `Page`, a tab of a tabbed record included,
so a navigator's push carries the header with the screen.

## A Content page

The page scrolls as one column under a pinned header. The body is a
`Stack` of `Section`s, `lg` apart.

```rust
use idea_ui_layout::{Action, FieldGrid, FieldGridRow, LabelValue, Page, PageHeader, Section, SidebarToggle, StatGrid};

#[component]
fn WorkerScreen(#[prop(static)] sidebar: SidebarState, worker: Signal<Worker>) -> Element {
    let editing = signal(false);
    ui! {
        Page(
            header = ui! {
                PageHeader(
                    title = rx!(worker.get().name),
                    leading = Some(ui! { SidebarToggle(sidebar = sidebar) }),
                    actions = vec![Action::new("Edit worker", open_edit.clone()).icon(PENCIL).primary()],
                    menu = vec![Action::new("Archive", archive.clone()).icon(ARCHIVE)],
                )
            },
            body = ui! {
                Stack(gap = StackGap::Lg) {
                    StatGrid {
                        // Each child is one tile: your own component, or a Card.
                        Card { Typography(content = "Hours".to_string(), muted = true) Typography(content = hours, kind = typography_kind::H2) }
                        Card { Typography(content = "Shifts".to_string(), muted = true) Typography(content = shifts, kind = typography_kind::H2) }
                    }
                    Section(title = "Employment".to_string()) {
                        Stack(gap = StackGap::Sm) {
                            LabelValue(label = "Hired".to_string(), value = rx!(worker.get().hired_on))
                            LabelValue(label = "Position".to_string(), value = rx!(worker.get().position))
                        }
                    }
                    Section(title = "Contact".to_string()) {
                        FieldGrid(columns = 2_usize) {
                            Field(label = Some("First name".to_string()), value = first)
                            Field(label = Some("Last name".to_string()), value = last)
                            FieldGridRow { Field(label = Some("Notes".to_string()), value = notes) }
                        }
                    }
                }
            },
        ) {
            // The page's children are its dialogs and sheets, mounted beside
            // the frame so nothing the body rebuilds can tear one down.
            EditWorkerDialog(open = editing)
        }
    }
}
```

What the blocks do for you:

| Block | Wide | Phone |
|---|---|---|
| `FieldGrid(columns = 1..=3)` | that many columns | one column |
| `LabelValue` | label beside value | label above value |
| `StatGrid` | four across | two across |
| `ActionBar` | right-aligned row | full-width, stacked, primary first |
| `Section` | a heading and its body, spaced | the same |

Add no side padding of your own: `Page` applies the gutter. A body that
must run edge to edge passes `bleed = true`.

## A Table page

The page mounts no scroller. The body is bounded to what the header and
toolbar leave, and the table inside owns the one scroller, so the toolbar
and the column labels hold still.

```rust
use idea_ui_layout::{Blank, BottomInset, ListPhase, ListStates, Page, PageVariant, Toolbar, ToolbarButton, ToolbarSearch};

ui! {
    Page(
        variant = PageVariant::Table,
        header = ui! { PageHeader(title = "Headings".to_string(), actions = vec![add_heading.clone()]) },
        toolbar = Some(ui! {
            Toolbar(
                start = Some(ui! { ToolbarSearch(value = search, inline = true, placeholder = "Search headings".to_string()) }),
                end = Some(ui! {
                    ToolbarButton(
                        label = "Filter".to_string(),
                        icon = Some(LIST_FILTER),
                        count = rx!(filters.get().len()),
                        on_press = open_filters.clone(),
                        bind_to = Some(filter_button),
                    )
                }),
            )
        }),
        body = ui! {
            ListStates(
                phase = rx!(load.get()),
                is_empty = rx!(rows.get().is_empty()),
                narrowed = rx!(!search.get().is_empty() || !filters.get().is_empty()),
                blank = Blank::new("No headings yet").action(add_heading.clone()),
                no_match = "No headings match.".to_string(),
                failed_title = "Couldn't load the headings".to_string(),
                on_retry = Some(reload.clone()),
                rows = move || ui! {
                    scroll_view(style = Rows()) {
                        // your table, or rows
                        BottomInset()
                    }
                },
            )
        },
    )
}
```

- **`ListStates`** draws the four things a list shows when it has no rows:
  loading, failed (with Retry), blank (no records exist; offers the action
  that adds the first) and empty (the filters removed everything; no
  action, since the toolbar is the way back). You supply the words.
- **`narrowed`** is what tells blank from empty. Pass whether any search or
  filter is on.
- **`rows`** is a builder. It runs when the list goes from having no rows
  to having some, not on every change to the rows.
- **Your scroller** must be bounded by the page, not by its content:
  `flex_grow: 1`, `flex_shrink: 1`, `min_height: 0`.
- **`BottomInset()`** goes last inside it. See "Safe areas".

The table itself is still `idea_ui::Table`; the kit gives it a seat and its
states.

## A Blank page

Bounded to the window, and nothing else decided: no gutter, no scroller.
The body owns its whole area.

```rust
ui! {
    Page(
        variant = PageVariant::Blank,
        header = ui! { PageHeader(title = "Crew board".to_string()) },
        body = ui! { CrewBoard() },
    )
}
```

A Blank body that cannot work at a phone's width says so with a
`StateMessage` rather than drawing a broken canvas.

## SplitView

Two regions side by side from one width per mode, with one narrow form
each. Crossing the width restyles the regions and remounts neither, so a
half-typed field survives a rotation.

| Mode | For | Beside from | Below that |
|---|---|---|---|
| `Nav` | a list of destinations and the open one | 768 | the list is the screen; opening a destination replaces it |
| `Detail` | a record and a companion panel | 768 | the panel stacks under the record |
| `Inspector` | a canvas and settings for the selection | 1024 | tablet: the pane overlays with ✕. Phone: it docks under the canvas |

Settings, in a scrolling page:

```rust
let frame = PageFrame::new();
ui! {
    Page(
        frame = Some(frame),
        header = header,
        body = ui! {
            SplitView(
                mode = SplitMode::Nav,
                first = sections,                       // the list
                second = section_body,                  // the open destination
                open = rx!(open_section.get().is_some()),
                frame = Some(frame),                    // pins the list under the header
                grow = true,
            )
        },
    )
}
```

Pass the same `PageFrame` to the `Page` and the `SplitView` when the split
sits in a `Content` page: the side region then pins under the header while
the page scrolls, instead of starting a second page scroller. Below 768,
give the reader a way back to the list by setting `open` false (a header
action, or your navigator's back).

A workbench, in a bounded page:

```rust
ui! {
    Page(
        variant = PageVariant::Blank,
        header = header,
        body = ui! {
            SplitView(
                mode = SplitMode::Inspector,
                first = canvas,
                second = inspector,
                open = rx!(selected.get().is_some()),   // tablet: shows the pane
                on_close = Some(clear_selection),       // tablet: the overlay's ✕
            )
        },
    )
}
```

## Flow

One step at a time, with its actions pinned at the foot. `Flow` mounts its
own `Page`, so it is the whole screen.

```rust
use idea_ui_layout::{Action, ActionBar, Flow, FlowStep};

ui! {
    Flow(
        header = ui! { PageHeader(title = "New project".to_string(), actions = vec![cancel.clone()]) },
        steps = rx!(vec![
            FlowStep::new("Basics").subtitle("What the project is called").status(basics_answer()),
            FlowStep::new("Shifts").subtitle("When the crews work"),
            FlowStep::new("Crew"),
        ]),
        current = rx!(step.get()),
        on_step = Some(jump_back),          // done steps only
        body = ui! { match step.get() { i => { StepBody(step = *i) } } },
        actions = ui! {
            ActionBar(
                primary = Some(Action::new("Next", next.clone())),
                secondary = vec![Action::new("Back", back.clone())],
                pinned = true,
            )
        },
    )
}
```

From 768 the step list sits beside the step. Below it the list collapses
to "Step 2 of 3 · Shifts" over a progress bar, and pressing that opens the
list as a sheet. Steps ahead never respond to a press.

## The header

`PageHeader` is slots in a fixed order. You fill them; the header arranges
them for the width.

| Slot | Holds | On a phone |
|---|---|---|
| `leading` | the app's way around: `SidebarToggle` | drawn |
| `nav` | the way back | drawn; what it draws is its own |
| `title`, or `title_content` | the page's name, or your own element for it | drawn; truncates first |
| `menu` | what applies to the whole record | folded into the one ⋮ |
| `status` | the record's status | not drawn |
| `trailing` | something that is not a verb | not drawn |
| `actions` | the verbs | the primary's glyph; the rest folded |

**Actions.** At most three, at most one `primary()`. From 640 they are
labelled buttons. On a phone the primary is its glyph alone, and every
other verb folds into one ⋮ with the record's `menu`, which opens as an
action sheet. Give the primary an `icon` or it keeps its label on a phone.

**The way back.** The `nav` slot takes any element. The kit's own is
`HeaderWayBack`: hand it both the ancestors and a pop, and it draws a
linked trail on the web and the desktop and a back arrow on iOS and
Android.

```rust
PageHeader(
    title = "Settings".to_string(),
    nav = Some(ui! {
        HeaderWayBack(
            crumbs = vec![Crumb::linked("Projects", to_projects), Crumb::linked("North Ramp", to_project)],
            on_back = Some(pop),
        )
    }),
)
```

`HeaderTrail` and `HeaderBackButton` are the two forms on their own, for
one form on every platform. `HeaderTrail` folds its ancestors into a "…"
menu on a phone; `fold_on_phone = false` turns that off. For anything else
(first and last crumb, the page named by the last crumb), pass your own
element to `nav` and `title_content`.

## The toolbar

`Toolbar` is the row under the header, passed to `Page(toolbar = …)`. It
holds only what scopes the body; a verb that writes belongs in the
header's actions.

| Slot | Holds |
|---|---|
| `start` | navigation: `ToolbarTabs`, or an inline `ToolbarSearch` where there are no tabs |
| `end` | controls: `ToolbarSearch`, `ToolbarButton`s, a date control, a view switch |
| `below` | a line of its own, for what fits no row at any width |

When the controls would leave `start` under 200 wide, they wrap to a line
of their own, still at the right. The row works this out from the actual
widths, so you write one toolbar for every size.

```rust
Toolbar(
    start = Some(ui! { ToolbarTabs(tabs = tabs, active = rx!(tab.get()), on_change = pick) }),
    end = Some(ui! {
        ToolbarSearch(value = search, placeholder = "Search this project".to_string())
        ToolbarButton(label = "Filter".to_string(), icon = Some(LIST_FILTER), count = rx!(filters_on()), on_press = open_filters)
    }),
)
```

- **`ToolbarTabs`** takes idea-ui `Tabs`' three props. It draws as many
  tabs as fit and puts the rest under "N more"; nothing scrolls sideways.
- **`ToolbarSearch`** is a magnifier that opens its field, with a dot while
  a term is set. `inline = true` is the field itself, for the `start` slot.
  It writes your `value` signal. Ctrl-F (⌘F on a Mac) goes to it while it is
  mounted, in place of the browser's find; pass `find_shortcut = false` on
  all but one where two are mounted at once. Pass `open = Some(signal)` to
  open the glyph's field from your own code.
- **`ToolbarButton`** shows its label and count from 640 ("Filter · 2") and
  its glyph with a dot on a phone. `compact = true` is the glyph at every
  width with the count as a number on its corner. `bind_to` hands you the
  handle to anchor what it opens.

## A record with tabs

A record with several screens (a project, a worker) is not a special
layout. Each tab is a screen: it picks its own page variant and mounts
its own `Page`, header and toolbar. There is no shell holding one header
over the tabs, and a tab cannot publish content into a header above it.

The tabs read as one record because they build their chrome from the
same component. Write that component once:

```rust
/// One tab of a project, as a whole screen.
#[component]
fn ProjectTabScreen(tab: Signal<ProjectTab>, search: Signal<String>, #[prop(static)] open: ProjectTab) -> Element {
    ui! {
        Page(
            variant = open.variant(),                 // Content, Table or Blank, per tab
            header = ui! { PageHeader(title = rx!(project.get().name), actions = open.actions()) },
            toolbar = Some(ui! {
                Toolbar(
                    start = Some(ui! { ToolbarTabs(tabs = tabs, active = open.id(), on_change = pick) }),
                    end = Some(ui! { ToolbarSearch(value = search) }),
                )
            }),
            body = open.body(),
        )
    }
}
```

and switch on the open tab above it (or let a navigator do it, one route
per tab):

```rust
let tab = signal(ProjectTab::Overview);
let search = signal(String::new());   // held ABOVE the tabs
ui! {
    match tab.get() {
        open => { ProjectTabScreen(tab = tab, search = search, open = *open) }
    }
}
```

Two things follow from each tab owning its header:

- A tab can differ where it needs to: its own verbs in `actions`, its
  own status, a toolbar with different controls.
- Switching tabs rebuilds the header and toolbar. Anything that must
  survive the switch (the open tab, a search term, a filter) is a signal
  held above the tabs and passed in, as `search` is here. State created
  inside the header resets.

## Overlays

Each of these is anchored or centred with a pointer and a bottom sheet on
a phone. Same content and handlers; only the surface changes.

| With a pointer | On a phone | Component |
|---|---|---|
| anchored menu | action sheet | `ActionMenu`, `ActionMenuSurface` |
| popover | titled bottom sheet | `AdaptivePopover` |
| top-anchored dialog card | bottom sheet | `Dialog` |

Mount each one once, ungated, and drive its `open` signal. Do not wrap
them in `if open.get() { … }`: the sheet form has to stay mounted to play
its exit. A `Page`'s children block is the place for them.

```rust
// A row's verbs behind a ⋮. Self-contained: it owns its open state.
ActionMenu(actions = vec![
    Action::new("Rename", rename).icon(PENCIL),
    Action::new("Delete", delete).icon(TRASH),
])

// A filter panel opened by a ToolbarButton.
AdaptivePopover(
    open = filters_open,
    target = Some(AnchorTarget::from(filter_button)),
    title = Some("Filter".to_string()),          // the sheet's heading
    content = move || ui! {
        Stack(gap = StackGap::Xs) {
            SheetCheckRow(label = "Day".to_string(), checked = rx!(day.get()), on_press = toggle_day.clone())
            SheetCheckRow(label = "Night".to_string(), checked = rx!(night.get()), on_press = toggle_night.clone())
        }
    },
)

// A form.
Dialog(
    open = editing,
    title = "Edit worker".to_string(),
    on_close = close.clone(),
    body = move || ui! { FieldGrid(columns = 2_usize) { /* fields */ } },
    footer = move || ui! {
        DialogFooter {
            Button(label = "Cancel", variant = variant::Ghost, block = dialog_block(), on_click = close.clone())
            Button(label = "Save", block = dialog_block(), on_click = save.clone())
        }
    },
)
```

- `content`, `body` and `footer` are builders (`move || ui! { … }`): they
  run on each open.
- Each button in a `DialogFooter` passes `block = dialog_block()`, which
  makes it full-width in the sheet and label-wide in the card.
- `ActionMenuSurface` is the menu without its trigger, for a right-click
  or a long press.
- `Sheet` with `SheetActionRow` (does something and closes),
  `SheetCheckRow` (toggles and stays), `SheetPickRow` (one of a set: a
  check at the end of the chosen row) and `SheetPushRow` (pushes a pane)
  is for building your own. `DrillSheet` is a sheet with levels.
- A slot (`Sheet`'s `content`, `ListStates`' rows) takes a closure or
  a builder you already hold as `Rc<dyn Fn() -> Element>`.

## Safe areas

Content runs under the bottom safe area (the home indicator on an iPhone)
and comes to rest clear of it.

- `Page` takes the top and side safe areas. Take none yourself.
- A `Content` page pads the end of its own scroller. Nothing to do.
- In a `Table` or `Blank` page, your scroller reaches the screen's bottom
  edge. Put `BottomInset()` last inside it.
- A pinned `ActionBar` and a `SplitView`'s side pane pad themselves.

## Changing the gutter

The page gutter is three theme tokens from the kit's own `LayoutTheme`:
16 on a phone, 24 from 640, 32 from 1024. The header, toolbar, page body
and a pinned action bar all read them. To change them for the whole app,
install a changed theme after the idea theme:

```rust
use idea_ui_layout::LayoutTheme;
use runtime_core::Tokens;

LayoutTheme::install(LayoutTheme::standard().with(|t| {
    t.gutter.tablet = 16.0.into();
    t.gutter.desktop = 24.0.into();
}));
```

## The one width question a screen may ask

`opens_on_phone()` reads the width once, untracked, for a value a screen
picks as it opens and the reader changes from there (a canvas's starting
zoom). It never chooses what to draw. If a screen needs to branch on the
width to draw something, that is a gap in a layout: extend the layout.

## Before you call a screen done

1. Does it mount exactly one `Page`?
2. Is the body's top level a handful of named `Section`s, not a flat column
   of controls?
3. Is every gap a spacing step you can name (`xs` within one thing, `sm`
   between siblings, `md` between fields, `lg` between sections)?
4. Is there one scroller, and is it the one the page variant says?
5. Does a `Table` or `Blank` body's scroller end with `BottomInset()`?
6. Are the dialogs and sheets mounted ungated in the page's children?
7. Did you look at it at 390, 820 and 1440 wide? Clipping produces no
   error and no failed test.
