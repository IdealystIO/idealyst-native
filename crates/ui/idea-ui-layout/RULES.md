# idea-ui-layout: the rules

These are the rules the kit is built to. Each has a name; the crate's
docs cite them as "rule `name`", and a review can too. Where a rule
names a component, that component is what enforces it.

The first rule governs the rest:

### layout
**A screen never reads the breakpoint. It picks a layout, hands it
content, and the layout decides what each width draws.**

- `current_breakpoint()` is read inside this kit. A screen calls it
  nowhere.
- A screen that needs something no layout gives is a reason to extend
  the layout, never to branch on the width inside the screen.
- A screen-local `breakpoint` overlay in a stylesheet is allowed for
  what is particular to that screen. Gutters, the gap between fields, a
  row that stacks, a column count and a button's width are not
  particular: they belong to the blocks below.
- `opens_on_phone()` is the one width question a screen may ask, for a
  value it picks ONCE as it opens (a canvas's starting zoom). It never
  chooses what to draw.

## The ladder

### ladder
**Two layouts of the content, three seats for the sidebar.**

| | Phone (`Xs`, under 640) | Tablet (`Sm`, `Md`, 640 to 1023) | Desktop (`Lg` up, 1024 and wider) |
|---|---|---|---|
| Content | phone layout | desktop layout | desktop layout |
| App sidebar | drawer | drawer | pinned |
| A pane beside the content | never | from the layout's own width | yes |
| Menus and popovers | bottom sheet | anchored | anchored |
| Form dialogs | bottom sheet | card | card |

- A screen changes at two widths, 640 and 1024. A tablet is the desktop
  layout with the sidebar off screen, so a screen has two layouts, never
  three.
- `Md` (768) is the width at which a second pane fits. It belongs to
  `SplitView` and `Flow`. Nothing else steps there.
- The sidebar's three seats are `idea_ui_nav::AppShell`'s, not this
  kit's.

### gutter
**The page gutter is the theme's `lg` on a phone, `xl` from 640 and
`xxl` from 1024, and only `Page` applies it.**

The header, the toolbar row, the body and a pinned action bar all take
the same steps, so a body's first text stands under the title. A screen
adds no side padding of its own. A body that must run edge to edge
passes `bleed = true` and lines its own first text up with the title.

### one-scroller
**A screen has one scroller, and its page variant says which.**

On a `Content` page the page scrolls and nothing inside it scrolls
vertically. On a `Table` page the table scrolls and the page does not.
On a `Blank` page the body brings its own. A pane beside scrolling
content pins under the header and scrolls itself only when it is taller
than the room left; it does not start a second page scroller.

### safe-areas
**`Page` owns the safe areas. Nothing inside it takes one.**

The header block takes the top and sides; the body takes the bottom and
sides. A pinned `ActionBar` therefore pads only itself. Sheets and
dialogs are outside the page and carry their own.

### spacing-ladder
**A gap comes from the relationship between the things it separates. It
is never picked by eye.**

| Rung | Separates | Examples |
|---|---|---|
| `xs` | parts of ONE thing | a label and its value; a heading and its subtitle |
| `sm` | siblings in a group | buttons in a row; rows of a list; `LabelValue` rows |
| `md` | distinct things in one section | fields of a form; a section's heading and its body |
| `lg` | sections of a page | the children of a page's body |
| `xl` | page-level regions | only where a page has separate halves |

For every gap you type, name the rung. A raw pixel gap is a bug.

### groups
**A screen is groups of controls, and a group is a `Section`.**

A group is the set of fields that answer one question: roughly two to
six controls, with a name. A group that cannot be named is not one;
merge it or split it differently. The body of a `Content` page is a
`Stack(gap = StackGap::Lg)` of `Section`s. Cards do not nest, and a
table is already a frame.

### theme-owns-look
**Sizes, shapes and colours are the theme's.**

A screen passes no size to make a control fit and sets no colour or
radius. Every measure in this kit that is not a style token is a named
constant with the reason beside it.

## Choosing a layout

Ask in order and stop at the first yes.

1. Is the screen a canvas being edited, with settings for what is
   selected? **Workbench**: a `Blank` page holding a
   `SplitView(mode = Inspector)`.
2. Is it a sequence of steps a person finishes? **`Flow`**.
3. Is it a set of destinations with one open (settings sections)?
   **`SplitView(mode = Nav)`**.
4. Is its main content one table of records? **A `Table` page**.
5. Does it lay itself out edge to edge by its own rules (a custom grid,
   a map, a board)? **A `Blank` page**.
6. Otherwise it is **a `Content` page**. A record's detail screen is
   one; if it carries a companion panel, the page holds a
   `SplitView(mode = Detail)`.

A shell with tabs (a project, a worker) is one `Page` whose
`live_variant` follows the tab on show; each tab takes a layout of its
own from the same list and mounts no `Page`.

## Pages

A `Page` is the frame every screen stands in. Its variant decides three
things and nothing else.

| Variant | Scrolls | Body height | Gutter |
|---|---|---|---|
| `Content` | the page | its content's | all round |
| `Table` | the table inside | what the header leaves | sides only |
| `Blank` | whatever the body brings | what the header leaves | none |

### page-content
**A Content page scrolls as one column under a header that holds
still.**

Sections stack `lg` apart in the page gutter. On a phone it is the same
column; its blocks (`FieldGrid`, `LabelValue`, `StatGrid`) do their own
stacking. A table inside it is a section: it grows to its rows and has
no scroller of its own.

### page-table
**A Table page is a table that is the page: its toolbar is pinned and
only its rows scroll.**

The page mounts no scroller. The body is bounded to what the header and
toolbar leave, and the table in it owns the one scroller, so the search,
the filters and the column labels hold still. The body stands in the
side gutter with no inset above it (the toolbar row's padding is that
space) or below it (the table's own foot ends the page).

### page-blank
**A Blank page bounds its body to the window and decides nothing
else.**

No gutter and no scroller. For a body that owns its whole area: a
workbench, a `Flow`, a custom grid. A Blank body that cannot work at a
phone's width says so with a `StateMessage` rather than drawing a broken
canvas.

### header-holds-still
**The header is pinned, travels with its screen, and is charged to
every screenful.**

Each screen mounts its own header inside its own `Page`, so a
navigator's push carries the header with the screen. Only the body
scrolls under it. Anything pinned at the top must earn its height.

### header-actions
**Primary actions go in the header: at most three, at most one
filled.**

The verb that adds to or acts on the page stands at the far end of the
title row. A fourth button means the screen has not decided what its
primary act is.

- Tablet, desktop: labelled buttons, the primary last.
- Phone: the primary as its glyph alone. Every other verb folds into
  ONE ⋮. The primary never folds.
- A verb collapses to its glyph or moves into the menu whole. Its label
  is never shortened: a short label is a second piece of copy and reads
  as a different verb.

Built by `PageHeader(actions = …)`.

### record-menu
**What applies to the whole record is a ⋮ menu right after the title.**

Archive, Delete, "Save as template": never a button on one of the
record's tabs. The record's status follows the menu.

- Tablet, desktop: title, ⋮, status.
- Phone: the ⋮ moves to the row's far end and holds the folded verbs
  followed by the record's menu. Status is not drawn.

Built by `PageHeader(menu = …, status = …)`.

### toolbar-row
**One row under the header: navigation at the left, the controls that
scope the body at the right.**

Tabs at the left; search, filters, a date control, a view switch at the
right. The row is pinned with the header at every variant and holds
only what scopes the body. A verb that writes belongs in the header.

Built by `Page(toolbar = …)`.

## Layouts inside a page

### layout-split
**Two regions sit side by side from one width per mode, and each mode
has one narrow form.**

| Mode | For | Beside from | Below that |
|---|---|---|---|
| `Nav` | a list of destinations and the one that is open | `Md` | the list is the screen; opening a destination replaces it |
| `Detail` | a record and its companion panel | `Md` | the panel stacks under the record |
| `Inspector` | a canvas and the settings of what is selected | `Lg` | tablet: the pane overlays from the right while something is selected, with ✕. Phone: the pane docks under the canvas |

- Crossing a threshold restyles the two regions. It never remounts
  either, so a half-typed field survives a rotation.
- Nav's list is 240 wide; the second region of Detail and Inspector is
  320 unless the screen passes a width.
- No screen states a threshold of its own. `SplitMode::beside()` answers
  it for a decision a screen makes alongside its split, never for what
  to draw.

### layout-flow
**A Flow is one step at a time with its actions pinned at the bottom.**

- The step's content scrolls; the `ActionBar` (Back, Next or the final
  verb) does not.
- From `Md` the step list sits beside the step, done steps leading
  back. Below it the list collapses to "Step 2 of 5 · Shifts" over a
  progress bar, which opens the list as a sheet.
- Steps ahead never respond to a press: a flow that saves on each Next
  would skip the write the step owes.
- A companion panel is a `Detail` split inside the step, and the step
  list then keeps its narrow form at every width.

## Blocks

Blocks go inside any layout. Each replaces a stylesheet overlay a
screen would otherwise write by hand.

### field-grid
**Fields sit in one, two or three columns on a wide screen and in one
on a phone.**

`FieldGrid(columns = 2_usize) { … }`, `md` apart both ways. A field
that must span the row is wrapped in `FieldGridRow`. Reading order is
source order at every width.

### label-value
**A read-only fact is its label and its value: beside each other when
wide, the label above the value on a phone.**

`LabelValue(label = …, value = …)`. The label is muted, in a fixed
column when beside. A list of them is a `Stack` with the `sm` gap. An
absent value reads `—`: not stated is not zero.

### stat-grid
**Figures sit four across when wide and two across on a phone.**

Fewer than a row's worth fill the row evenly. A fifth wraps to a second
row; more than eight is a table.

### action-bar
**A screen's closing actions sit right-aligned when wide; on a phone
they are full width and stacked, the primary first.**

At most one primary and two others; a fourth goes in the header's ⋮.
`pinned = true` at the foot of a `Flow` or a decision screen.

## What a list shows when it has no rows

`ListStates` draws all four. A screen supplies the words and never
draws a state. The toolbar stays in every one: a screen never hides its
search or filters because the list has nothing in it.

### blank-state
**Blank: no records exist.**

A glyph over a message, centred where the rows would be. If the
screen's own verb creates the first record, that verb stands under the
message as a filled button.

### empty-state
**Empty: the filters removed every record.**

The same shape with the search-✕ glyph and no action. The toolbar above
is the way back.

### loading-state
**Loading: placeholder rows in the table's own shape.**

The same columns, row height and phone form as the rows that will
replace them, supplied by the screen through `loading`.

### failed-state
**Failed: the screen's title for the failure, the reason, and Retry.**

## Overlays

### overlays
**An overlay changes kind with the form factor. Its content and
handlers do not.**

| With a pointer | On a phone | Built by |
|---|---|---|
| anchored menu | action sheet | `ActionMenu`, `ActionMenuSurface` |
| anchored popover | bottom sheet with a title | `AdaptivePopover` |
| menu with submenus | sheet whose rows push panes | `DrillSheet` |
| top-anchored dialog card | bottom sheet | `Dialog` |

A panel anchored to a small trigger is not reachable with a thumb, and
it lands wherever the trigger happens to be. A sheet always arrives in
the same place, with room for full labels. A tablet keeps the anchored
form: it has the room, and its triggers are not under a thumb at the
bottom edge.

- A screen mounts the component once, ungated, and drives its `open`.
  The sheet form has to stay mounted to play its exit.
- A sheet has three kinds of row and only three: one that does
  something and closes (`SheetActionRow`), one that toggles something
  and stays (`SheetCheckRow`), one that pushes a pane (`SheetPushRow`).
- Sheet rows are at least 48 tall and run edge to edge.

### menus
**A menu row that leads somewhere opens beside the menu with a pointer
and pushes on a phone.**

With a pointer a submenu flies out beside its list, which stays on
screen. On a phone the pane replaces the rows under a titled back
chevron, inside the sheet. A row of chips across a sheet is not the
phone form; a list of push rows with a summary is.

### dialogs
**A form dialog pins its title and its verbs; only the fields between
them scroll.**

- From `Sm` the card is anchored at the top, not centred: a form that
  grows as it is filled would otherwise re-centre and move the field
  being typed in.
- On a phone it is a bottom sheet, its verbs full width and stacked
  with the verb the dialog is for nearest the thumb.
- The ✕, the backdrop and Escape are the same close.

Built by `Dialog` and `DialogFooter`; each footer button passes
`block = dialog_block()`.

## Phone

### phone
**A phone screen collapses rather than shrinks and stacks rather than
wraps.**

- Collapse, do not shrink: a header verb becomes its glyph or moves
  into the menu; it never gets a shorter label.
- Stack, do not wrap: a wrapping row lays out by label length, so it
  differs per record. The blocks stack by stylesheet, which restyles
  without rebuilding and so never closes an open menu.
- Titles give ground: anything sharing a row with a title lets the
  title truncate.
- A narrow form must be reachable. If an ancestor refuses the phone,
  a phone arm below it is dead code.
- Look at the picture. Clipping produces no error and no failed
  assertion; a phone layout is not finished until someone has looked at
  it at 390 wide.

## What the kit leaves to the app

- **The sidebar and its drawer**: `idea_ui_nav::AppShell`.
- **What goes before the title**: a back arrow, the drawer's ☰, a
  breadcrumb trail. `PageHeader(leading = …)` takes it; which one is
  right depends on the platform and the app's navigation.
- **The table itself and its paging**: `idea_ui::Table`. The kit gives
  it a seat (`Table` page) and its states (`ListStates`).
- **Search and filter controls**: the app's, mounted in the toolbar
  row. `AdaptivePopover` and `DrillSheet` are what they open.

## What a layout must hold to

For whoever adds to the kit:

- Every public component is declared with `#[component]`, its
  props documented and a `///` example on the function.
- Width changes are stylesheet overlays wherever both widths hold the
  same elements. A `ui!` `match` on the width is for the cases that
  hold different elements (a menu and a sheet), and it lives inside the
  kit.
- A layout mounted as a tab's body takes no header and adds no scroller
  beyond what its seat says.
- A mount test per form, asserting the form (sheet or menu, scroller or
  none, restyled or remounted), in `tests/mount.rs`.
