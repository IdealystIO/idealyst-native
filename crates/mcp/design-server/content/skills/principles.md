+++
title = "House rules for idea-ui apps"
summary = "The non-negotiables every screen follows: brand from the user, tokens only, adaptive by default, idea-ui first, every state designed."
when = "Before designing or reviewing anything in an idea-ui app — every prompt starts here."
order = 10
tags = ["principles", "rules", "brand", "tokens", "responsive", "adaptive"]
+++
# House rules for idea-ui apps

These rules are deliberately opinionated. They exist so an idea-ui app looks
designed on the first try, on every platform, without anyone inventing a
look. Follow them unless the user overrides one explicitly.

## 1. The look belongs to the user

The brand — colors, typeface, corner style, tone of voice — comes from the
user. Never invent one. If they gave nothing, use idea-ui's stock light and
dark themes unchanged; if they gave part of a brand, apply exactly that part
and list what you assumed. The brand lives in ONE module, `src/brand.rs`
(the `brand_module` recipe), installed first thing in `app()`.

What these rules decide is structure and usage — hierarchy, layout,
navigation, states, feedback — which is the same for every brand.

## 2. Tokens only

No hex colors, no pixel spacing, no font sizes in screens or components.
Color comes from the theme (`t.color.*`, `t.intent.*` in a `stylesheet!`;
`tone = …` on components), spacing from `t.spacing.*` or a component's
`gap`/`padding` scale, type from `Typography(kind = …)`. A literal in a
screen is a bug: it won't follow the brand, and it breaks dark mode.

The only fixed numbers allowed are layout constraints that aren't part of
the brand — a readable width cap, a pane width, an icon size.

## 3. Adaptive, not just responsive

Every screen works from a 360-wide phone to a wide desktop. That means two
different things, and both are required:

- **Responsive** — the same layout reflows: columns collapse, rows wrap,
  gutters shrink. Done with `breakpoint` blocks and breakpoint-driven
  props.
- **Adaptive** — the *pattern* changes kind across form factors. A phone
  pushes a detail over its list and pops back; a wide screen shows them
  side by side. A drawer becomes a pinned sidebar; a bottom sheet becomes a
  centered dialog; table rows become cards.

Same routes, same URLs, same data at every width — only the presentation
changes. Never fork a screen per platform. read_design_skill("adaptive") is
the full playbook.

## 4. idea-ui first

If idea-ui has a component for it, use it — `Button`, `Field`, `ListItem`,
`Card`, `Modal`, `Alert`, `Tabs`, `Badge`, … Their states, focus rings,
hover feedback, accessibility and theming are already right on every
backend. Drop to primitives (`view`, `text`, `pressable`) only for
genuinely new UI, and then style them with a token `stylesheet!`.

A component that looks wrong for the brand gets **restyled once, app-wide**
(`XStyles::restyle` / `set_default` in `brand.rs`) — never wrapped, forked,
or overridden per call site.

## 5. One job per screen

Every screen has one primary job and at most one primary (filled) action.
Everything else is secondary (`Outlined`/`Soft`), tertiary (`Ghost`) or in
a menu. If you can't name the job in a sentence, the screen is two screens.

## 6. Hierarchy you can see

Exactly one H1 per screen. Sections are H3 with an optional muted line
under them. Muted text (`muted = true`) for anything secondary. Spacing
encodes grouping: tight inside a group, loose between groups
(read_design_skill("layout")).

## 7. Every state is designed

A screen that shows data designs loading, error, empty, no-results and
data — and a form designs pristine, invalid, submitting, failed and saved.
"It renders nothing while loading" is a bug (read_design_skill("states")).

## 8. Platforms differ; the app doesn't hack around them

Touch has no hover, phones have notches and keyboards, iOS has swipe-back,
Android has a system Back. Design for those conventions through the
framework's seams — breakpoints, `primary_pointer()`, `safe_area`,
`keyboard_avoiding_view`, `platform()` for copy and shortcuts — never with
"subtract 4px on Android" (read_design_skill("platforms")).

## 9. Copy is part of the design

Buttons are verbs ("Save profile", "Delete workspace"), not "OK"/"Submit".
Errors say what happened and what to do. Empty states say what belongs
there and how to add it. Sentence case everywhere.
