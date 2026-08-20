# idea-ui — how to build with it

idea-ui is a **Rust** design system (the `idea-ui` crate in the Idealyst
framework). What is published here is generated from it: the stylesheet is
idea-ui's own build-time CSS asset, and every component's markup was rendered
by the framework's SSR backend. Nothing here is a hand-written lookalike, so
what you build maps directly onto shippable Rust.

## Setup — no provider, just the stylesheet

There is **no React provider to wrap**. The theme is plain CSS custom
properties, so linking one file is the entire setup:

```html
<link rel="stylesheet" href="styles.css">
```

`styles.css` imports the tokens, idea-ui's component CSS, and the small
live-engine remainder. Link it and components are styled. Dark mode is
already wired: set `data-theme="dark"` on `<html>` (or leave it off and the
OS preference decides).

## Styling idiom: tokens, not utility classes

idea-ui has **no utility-class vocabulary** — no `p-4`, no `bg-surface-1`.
Two mechanisms, and only two:

**1. Component props select a variant axis.** Every axis is a real,
enumerated value list; anything else is ignored and falls back to the default.

Each component owns exactly the axes of its own root sheet — a `Card` does
not restyle the text inside it, and a nested element keeps what idea-ui
rendered. The per-component `.d.ts` is authoritative; this is the shape.

| Axis | Values | Used by |
|---|---|---|
| `appearance` | `<tone>_<variant>` — tone: `primary` `secondary` `neutral` `success` `danger` `warning` `info`; variant: `filled` `soft` `outlined` `ghost` | Button, Badge, Tag, Alert, IconButton |
| `size` | `sm` `md` `lg` | Button, IconButton |
| `shape` | `sm` `md` `lg` `pill` | Button |
| `block` | `off` `on` (full width) | Button |
| `padding` | `none` `xs` `sm` `md` `lg` `xl` | Stack |
| `padding` | `none` `sm` `md` `lg` | Card |
| `variant` | `elevated` `flat` | Card |
| `gap` | `none` `xs` `sm` `md` `lg` `xl` | Stack, Grid |
| `axis` | `row` `column` | Stack |
| `align` | `start` `center` `end` `stretch` `baseline` | Stack |
| `justify` | `start` `center` `end` `between` `around` | Stack |
| `wrap` | `off` `on` | Stack |
| `kind` | `display` `h1` `h2` `h3` `body-xl` `body-lg` `body` `body-sm` `caption` `overline` | Typography |
| `color` | `default` `muted` `primary` `secondary` `neutral` `success` `danger` `warning` `info` | Typography |
| `weight` | `thin` `extra_light` `light` `normal` `medium` `semi_bold` `bold` `extra_bold` `black` `inherit` | Typography |
| `align` | `left` `center` `right` `justify` | Typography |
| `interactive` | `off` `on` | Badge, Tag, Alert |

```jsx
<Button appearance="danger_filled" size="lg" shape="pill">Delete</Button>
<Card variant="elevated" padding="lg">…</Card>
```

**2. Your own layout glue uses the CSS variables directly.** For spacing,
color and type in wrappers you write yourself, reference the tokens — never
hard-coded hex or px:

```jsx
<div style={{
  display: 'flex', flexDirection: 'column',
  gap: 'var(--spacing-lg)',
  padding: 'var(--spacing-xl)',
  background: 'var(--color-surface)',
  color: 'var(--color-text)',
  borderRadius: 'var(--radius-md)',
  border: '1px solid var(--color-border)',
}}>…</div>
```

Token families (full list in `tokens/tokens.css`, 74 tokens × light/dark):

- `--color-*` — `background` `surface` `surface-alt` `text` `text-muted`
  `text-inverse` `border` `border-hover` `border-strong` `focus-ring` `overlay`
- `--intent-<tone>-*` — `solid-bg` `solid-text` `soft-bg` `soft-text` `fg`
  `border`, for each of the seven tones
- `--spacing-*` — `xs`(4) `sm`(8) `md`(12) `lg`(16) `xl`(24) `xxl`(32)
- `--radius-*` — `sm`(4) `md`(8) `lg`(12) `pill`
- `--typography-*-size` — `display`(40) `h1`(32) `h2`(24) `h3`(19)
  `body-xl`(18) `body-lg`(16) `body`(14) `body-sm`(13) `caption`(12)
  `overline`(11)
- `--iy-default-font` — the system font stack

## The layout model is flex-column, not block

idea-ui inherits a React-Native-style layout model: containers are
`display: flex; flex-direction: column; align-items: stretch` by default.
Children therefore **fill their container's width** unless the container says
otherwise. That is why a `<Button>` in a plain container spans it — that is
correct idea-ui behaviour, not a bug to patch. Use `Stack` with `axis="row"`
for horizontal groups, and set `align` when you want intrinsic widths.

## Component API

Every component accepts its style axes (above), plus:

- `children` — replaces the component's text/content slot
- `className` — appended to the root's class list

Components come from the bundle global:

```jsx
const { Button, Card, Stack, Field, Typography } = window.IdeaUI;
```

## Where the truth lives

- `styles.css` and its imports (`tokens/tokens.css`, `_ds_bundle.css`) — the
  real values. Read these before inventing any style.
- `components/<Group>/<Name>/<Name>.prompt.md` — per-component usage, with the
  exact axis values that component supports and the Rust recipes it came from.
- `components/<Group>/<Name>/<Name>.d.ts` — the prop contract.

Component names and props here match the Rust API (`idea_ui::Button`,
`ButtonProps`), so a design translates to `ui! { Button(...) }` one-for-one.

## Worked example

```jsx
const { Card, Stack, Typography, Button, Badge } = window.IdeaUI;

<Card variant="elevated" padding="lg">
  <Stack gap="md">
    <Stack axis="row" justify="between" align="center">
      <Typography kind="h3">Deployment</Typography>
      <Badge appearance="success_soft">Live</Badge>
    </Stack>
    <Typography kind="body-sm">Last shipped 4 minutes ago.</Typography>
    <Stack axis="row" gap="sm" align="start">
      <Button appearance="primary_filled">Promote</Button>
      <Button appearance="neutral_outlined">Roll back</Button>
    </Stack>
  </Stack>
</Card>
```
