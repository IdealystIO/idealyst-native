+++
title = "Layout, spacing and containers"
summary = "The spacing rhythm (which spacing token for which gap), choosing Stack vs Grid vs Card vs Surface vs List, alignment, density, and the layout traps specific to idea-ui."
when = "Arranging content on a screen or inside a component; deciding gaps, padding, containers or alignment."
order = 35
tags = ["layout", "spacing", "stack", "grid", "card", "surface", "list", "density"]
+++
# Layout, spacing and containers

## The spacing rhythm

Spacing says what belongs together. Use the theme's scale (`xs 4 · sm 8 ·
md 12 · lg 16 · xl 24 · xxl 32` by default — the brand may change the
values, never the roles):

| Between… | Token | idea-ui knob |
| --- | --- | --- |
| an icon and its label; a label and its help line | `xs` | `StackGap::Xs` |
| items in a tight cluster (buttons in a group, chips, a title and its badge) | `sm` | `StackGap::Sm` |
| fields in a form; rows of related content; elements inside a card | `md` | `StackGap::Md` (Stack's default) |
| a section's title and its content group; cards in a grid | `md`–`lg` | `Section` / `Grid(gap = …)` |
| sections of a page | `xl` | built into `Page` |
| page edge (gutter) | `md` phone, `xl` from `Md` | built into `Page` |

Rules:

- **Space with gaps, not margins.** Put children in a `Stack(gap = …)`;
  never add padding to one child to push its neighbor.
- **Bigger gap = weaker relationship.** If two things are related, the gap
  between them must be smaller than the gap around them.
- **Don't nest equal gaps to fake a bigger one.** Stack → Stack with
  `Md`+`Md` means the outer one should have been `Lg`.

## Choosing a container

| Need | Use | Not |
| --- | --- | --- |
| Children in a column/row with uniform gaps | `Stack(axis, gap, align, justify, wrap)` | a raw `view` with a hand-made sheet |
| Equal tiles that reflow by width | `Grid(columns = rx!(…by breakpoint…), gap)` | a wrapped row of fixed-width boxes |
| A bounded, self-contained group (one object, one form section) | `Card(padding = …)` | a `Surface` with a border restyle |
| A region with a different background but no "object" boundary (a toolbar band, a pane) | `Surface(background = SurfaceAlt)` | a `Card` without a border |
| Rows of homogeneous items | `List { ListItem(…) }` | a Stack of Cards |
| Push siblings to opposite ends of a row | `Spacer()` inside a row Stack, or `justify = StackJustify::Between` | margins |
| Center an empty state or spinner | `Center` | manual flex math |
| A hairline between groups in one container | `Divider()` | a 1px view |

- **Card nesting is one level.** A card inside a card is a sign the inner
  thing should be a List row or a Section.
- **Cards group, Lists enumerate.** Five settings toggles = one Card with a
  Stack. Fifty customers = a List.
- **`CardPadding` runs one step larger than the spacing scale**
  (`Md` = `spacing.lg`). Use `CardPadding::Md` by default, `Sm` for dense
  tiles, `Lg` for an empty state.

## Alignment

- **Left-align text and forms.** Center only empty states, auth cards and
  single-number tiles.
- **Row content aligns to center** (`StackAlign::Center`) when it mixes
  icons, text and controls; **to baseline** when it's text of different
  sizes (a number and its unit).
- **Numbers right-align** in tables and in trailing row slots.
- **Actions align trailing** (right in LTR): primary last.

## Density

Default density is comfortable (touch). For data-heavy desktop tools the
user may ask for compact; do it app-wide in `brand.rs` (smaller spacing
tokens, `size::Sm` defaults via `set_default`) — never by shrinking
individual screens.

## Traps

- **`flex_grow` without `min_height: 0`** (or `min_width: 0` in a row)
  won't shrink: long content overflows instead of scrolling. Panes that
  grow set both.
- **Width caps go on the content column, not the scroll container** —
  otherwise the scrollbar floats in the middle of a wide window. `Page`
  does this right.
- **Text in a row doesn't wrap unless it can shrink**: give the text
  column `flex_grow: 1` and `min_width: 0`.
- **No fixed heights on content.** Let content size itself; fix heights
  only for media and bars.
