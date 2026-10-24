+++
title = "Adaptive and responsive layouts"
summary = "How one screen serves phone, tablet and desktop: the breakpoint set, which patterns reflow and which change kind (push/pop ⇄ split view, drawer ⇄ sidebar, sheet ⇄ dialog, cards ⇄ table), and how to wire each."
when = "Designing any screen or shell — every design must work at phone and desktop widths — or deciding how a mobile pattern maps to a wide web layout."
order = 18
tags = ["responsive", "adaptive", "breakpoints", "mobile", "split-view", "layout"]
+++
# Adaptive and responsive layouts

Every screen must work from a 360-wide phone to a 1440+ desktop. Two
mechanisms, used together:

- **Reflow (responsive)**: the same components rearrange — grids lose
  columns, rows wrap to columns, gutters tighten.
- **Change kind (adaptive)**: the interaction pattern itself differs,
  because what's natural with a thumb on a 6" screen is wrong on a 27"
  monitor with a mouse, and vice versa.

The rule that ties them together: **one route table, one URL scheme, one
set of screens.** The breakpoint chooses a presentation; it never chooses a
different app.

## The breakpoint set

idea-ui uses the framework's min-width breakpoints (`runtime_core::Breakpoint`):

| Name | From (dp) | Think of it as |
| --- | --- | --- |
| `Xs` | 0 | phone, portrait |
| `Sm` | 640 | phone landscape, small tablet |
| `Md` | 768 | tablet portrait |
| `Lg` | 1024 | tablet landscape, laptop — **the "wide" line** |
| `Xl` | 1280 | desktop |

House decisions:

- **`Lg` is where layouts go multi-pane** (sidebar pinned, list beside
  detail). Below it, one pane at a time.
- **`Md` is where dialogs stop being sheets** and two-column forms become
  allowed.
- Keep the default table. If the brand's shell needs a different "wide"
  line, change it once at boot with `install_breakpoints(Breakpoints { lg_min: …, ..Default::default() })`
  before anything reads a breakpoint — never with ad-hoc pixel checks.
- Every design is checked at **phone (390) and desktop (1280)** at
  minimum; check tablet (820) when a layout changes between `Md` and `Lg`.

## How to branch on the breakpoint

Prefer the most static form that works:

1. **Style only → a `breakpoint` block in the stylesheet.** On web it
   compiles to a real `@media` rule (correct on first paint, no Rust work
   on resize); on native it merges reactively.
   ```rust
   stylesheet! {
       Toolbar<IdeaThemeRef> {
           base(t) { flex_direction: FlexDirection::Column, gap: t.spacing.sm() }
           breakpoint md(t) { flex_direction: FlexDirection::Row, gap: t.spacing.md() }
       }
   }
   ```
2. **A component prop → `rx!` over `current_breakpoint()`.**
   `Grid(columns = rx!(match current_breakpoint().get() { Breakpoint::Xs => 1u32, Breakpoint::Sm | Breakpoint::Md => 2, _ => 4 }))`,
   `Modal(presentation = rx!(…))`.
3. **Structure → `if` inside `ui!`** on `current_breakpoint().get().is_at_least(…)`:
   a pane that exists only when wide. Put the condition in one helper
   (`fn is_wide() -> bool`) so every branch agrees on the line.

Never read `viewport_size()` to pick a layout — it bypasses the shared
breakpoint table, so two parts of the screen flip at different widths.

## Patterns that change kind

| Pattern | Phone (< `Lg` unless noted) | Wide | Recipe |
| --- | --- | --- | --- |
| Sections of the app (≤ 5, used daily) | bottom tab bar | pinned sidebar | `app_shell_tabs` |
| Sections of the app (tool/admin, any count) | drawer behind a menu button | pinned sidebar | `app_shell_sidebar` |
| List → detail | push the detail; Back pops | list pane beside detail pane; selecting replaces | `list_detail_adaptive` |
| Confirm / short task | bottom sheet (`ModalPresentation::Sheet`, < `Md`) | centered dialog | `settings_form` |
| Filters, row actions | bottom sheet | anchored `Popover` / `Menu` | — |
| Data grid | `TableCards` (a card per row) | `Table` | — |
| Page actions | wrap under the title; overflow into a menu | inline, right-aligned | `page_layout` |
| Multi-column form | one column | two columns from `Md` only for short paired fields | `settings_form` |
| Dashboard tiles | one column | 2 → 4 columns | `dashboard` |

### List → detail: push/pop vs split view

This is the one people get wrong, because the mobile model ("detail
replaces list, Back returns") and the desktop model ("both visible,
clicking switches") look incompatible. They aren't, if both are one stack
navigator:

- Routes: `/` (the list) and `/items/:id` (the detail). Deep links work at
  any width.
- Narrow: the stack shows one screen. Row press → `push`. The bar's back
  (and the iOS swipe / Android system Back) → `pop`.
- Wide: the layout draws the list in a pane *outside* the outlet, and the
  outlet becomes the detail pane. The root screen renders "Select an item"
  there. Row press → `push` from the root, **`replace` from an open
  detail** (clicking through ten items must not leave ten entries to pop).
  No back button — the list is the way back.
- Crossing the breakpoint (window resize, tablet rotation) keeps the open
  detail mounted: the outlet never moves; only the list pane appears or
  disappears beside it.

`get_design_recipe("list_detail_adaptive")` is the complete wiring.

### Sheets vs dialogs vs popovers

On a phone, anything the user drives with a thumb rises from the bottom:
confirmations, filters, pickers, row actions → `Modal(presentation = Sheet)`.
On wide screens the same content is a centered dialog (blocking decisions)
or an anchored `Popover`/`Menu` (non-blocking choices next to their
trigger). Drive `presentation` with `rx!` so it's one component.

## Reflow rules

- **Cap reading width** (`Page(width = Narrow | Regular)`); don't let text
  lines run 1400px wide on desktop.
- **Rows wrap before they shrink**: action clusters and toolbars use
  `flex_wrap: Wrap` so a phone gets two lines, not squeezed buttons.
- **Gutters**: `spacing.md` on phones, `spacing.xl` from `Md` (built into
  `Page`).
- **Fixed widths only for panes** (sidebar 240–280, list pane ~360), never
  for content.
- **Touch first, then pointer**: size for fingers by default (44pt
  targets); a `Fine` `primary_pointer()` may densify, never the reverse.
- **No horizontal scroll** of the page at any width. Wide content (tables,
  code, boards) scrolls inside its own container.

## Checklist

- [ ] Renders correctly at 390 and 1280 wide (and 820 if anything changes at `Md`).
- [ ] Every breakpoint branch reads `current_breakpoint()` / a `breakpoint` block — no pixel checks.
- [ ] Adaptive patterns share routes and URLs; deep links work at any width.
- [ ] Resizing across a breakpoint keeps state (open detail, form input, scroll).
- [ ] Nothing reachable only by hover; touch targets ≥ 44pt on touch.
