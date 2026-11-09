+++
title = "Overlays: modals, sheets, menus, popovers, hover cards, tooltips"
summary = "When a task belongs in a Modal vs a pushed screen, Sheet vs dialog vs Popover by form factor, menus for overflow actions, confirmation rules for destructive actions, and the always-mounted Modal contract."
when = "Adding a dialog, confirmation, menu, popover, hover card, tooltip or bottom sheet."
order = 55
tags = ["overlays", "modal", "sheet", "dialog", "menu", "popover", "hover-card", "tooltip", "confirm", "destructive"]
+++
# Overlays

## Which surface

| The content is… | Phone | Wide | idea-ui |
| --- | --- | --- | --- |
| a blocking decision (confirm, choose before continuing) | bottom sheet | centered dialog | `Modal(presentation = rx!(…Sheet/Centered…))` |
| a short task (≤ 5 fields: rename, invite) | bottom sheet | centered dialog | `Modal` |
| a long task or something linkable (an editor, a full form) | a pushed screen | a pushed screen or a wide dialog | a stack route |
| actions on a thing (row, card, page) | `Menu` (or a sheet of actions) | `Menu` anchored to an overflow button | `Menu` + `IconButton` trigger |
| supplementary info next to its trigger | sheet | `Popover` | `Popover` |
| detail about a thing, on hover (a progress breakdown, a profile preview) | long-press | `HoverCard` | `HoverCard` — never essential info |
| a hint about a control | — (long-press) | `Tooltip` | `Tooltip` — never essential info |
| search / command palette | full-height sheet | top-anchored dialog | `Modal(presentation = Top)` |

## Modal contract

- **Always mounted.** `Modal(open = signal, …)` — never `if open { Modal(…) }`.
  Flipping `open` animates in/out.
- **`content` is a closure** rebuilt on each open; clone the handlers it
  needs inside it.
- **Dismissable dialogs show a visible way out** (Cancel / ×): iOS has no
  Back key, and Escape/Android Back only reach `on_dismiss` when
  `dismissable`.
- **Title first, then the consequence, then the actions** — Cancel (ghost)
  then the primary (filled) last, trailing.
- **A form that can outgrow the screen keeps its title and actions fixed**:
  put them in `header = move || ui! { … }` / `footer = move || ui! { … }`.
  Those render outside the modal's scroller, so only `content` scrolls.
- **One portal at a time**: don't open a Modal from inside a Menu that
  stays open; close the menu first.

## Hover cards

- `HoverCard(content = move || ui! { … }) { trigger }` wraps its trigger
  like `Tooltip`, with any content in the card. It opens after the pointer
  rests (`open_delay_ms`), stays open while the pointer moves into the
  card, and closes after it leaves both (`close_delay_ms`).
- Bind `disabled` to state that would make the card stale (`disabled =
  dragging` while its row is dragged): turning it on closes the card and
  nothing opens it until it turns off.
- Never drive a `Popover` from hover: its click-away layer covers the
  trigger and the popover flickers shut. Clickable content that opens on
  click is a `Popover`.
- Touch opens it on long-press and keyboard focus opens it too, but
  neither is as discoverable as hover, so the card repeats or summarises what is already on screen; it is never
  the only way to reach a fact or an action.

## Destructive actions

- At rest: an outlined `tone::Danger` button, in a danger zone at the
  bottom of the page or in the overflow menu (as the last item, after a
  separator).
- Confirm with a Modal that **names the thing and the consequence**:
  "Delete Atlas? Its 34 tasks will be permanently removed." The confirm
  button repeats the verb ("Delete project"), filled danger.
- For high-stakes deletes (a workspace, an account), require typing the
  name before the button enables.
- Prefer **undo** over confirm for anything reversible (read_design_skill("states")).

## Menus

- Menus hold actions, not navigation. An item is a verb ("Rename",
  "Duplicate", "Delete").
- Order: most common first, destructive last after a `MenuSeparator`.
- The trigger is an `IconButton` (⋯) with `variant = Ghost`; on touch,
  make it `IconButtonSize::Lg` or pad it to 44pt.
- Anything in a hover-revealed row toolbar must also be in the row's menu
  for touch.
