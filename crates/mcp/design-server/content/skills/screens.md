+++
title = "Designing a screen"
summary = "The screen archetypes (overview, collection, detail, form/settings, auth, empty/onboarding), the anatomy every screen shares, and the step-by-step way to design one with idea-ui."
when = "Designing or building any individual screen."
order = 30
tags = ["screens", "page", "archetypes", "dashboard", "detail", "list", "settings", "auth"]
+++
# Designing a screen

## Workflow

1. **Name the job** in one sentence ("see which projects need attention")
   and the **primary action** ("create a project"). One of each.
2. **Pick the archetype** below and open its recipe.
3. **List the hierarchy** before writing code: title, subtitle, actions,
   then each section with what it shows. Cut anything that doesn't serve
   the job.
4. **Decide the phone and wide presentations** (read_design_skill("adaptive")).
5. **Build** inside `Page` (the `page_layout` recipe) with idea-ui
   components and tokens only.
6. **Design every state** (read_design_skill("states")).
7. **Review** with read_design_skill("review").

## Anatomy every screen shares

```
Page(title, subtitle?, actions?, width)        ← one scroll container, capped width
├─ PageHeader: H1 · muted subtitle · actions (primary last)
├─ Section(title, description?) …              ← spacing.xl between sections
│  └─ Card / List / Grid / Field …             ← spacing.md inside
└─ …
```

- **The page scrolls, not its parts.** One `scroll_view` per screen
  (`Page` owns it). Inner scrolling only for a pane in a split view or a
  bounded widget (a table, a code block).
- **Width**: `Narrow` (720) for forms, settings, auth and reading;
  `Regular` (1120) for dashboards, lists, details; `Wide` for tables and
  canvases only.
- **Actions**: the page's actions sit in the header. A form's submit sits
  at the end of the form section it submits. A list row's actions sit on
  the row (or its overflow menu).

## Archetypes

### Overview / dashboard
The answer to "what needs my attention?". Lead with 2–4 key numbers
(stat tiles), then the actionable list (needs review, overdue), then
recent activity. Numbers link to the list they summarize. No chart without
a question it answers. → `dashboard` recipe.

### Collection (list of things)
Header (title, count, primary "New" action), then search/filter toolbar,
then the list or table. Rows show the identifying label, one or two
attributes, and status as a `Badge`. Row press opens the detail. Design
the five states. → `collection_states` recipe; list + detail together →
`list_detail_adaptive`.

### Detail
The thing's name as the title (in the stack bar when pushed), its status
near the title, then sections from most to least used: summary, the
thing's content, related items, metadata/history last. Edit is a header
action or inline per section — not both. Destructive actions at the
bottom in a danger zone.

### Form / editor
One column, `Narrow`. Group fields into titled sections of ≤ 6 fields.
Submit at the end of the form; Cancel beside it (ghost). For long forms,
the submit also lives in the page header. → `settings_form` recipe,
read_design_skill("forms").

### Settings
A form archetype with sections that save independently. Order: profile,
preferences, notifications, integrations, billing, danger zone. →
`settings_form` recipe.

### Auth (sign in, sign up, reset)
Outside the app shell. A centered `Narrow` card on wide screens,
edge-to-edge on phones. Product name/logo, one H1 ("Sign in to Acme"), the
fields, one block primary button, the secondary path as a link ("Forgot
password?", "Create an account"). → `sign_in` recipe.

### Empty / first run
When a whole section has nothing yet, the section *is* the empty state:
what it's for, the one action that fills it, optionally a link to learn
more. Not a blank list with a "+" somewhere.

## Text in a screen

| Role | Component |
| --- | --- |
| Page title (once) | `Typography(kind = H1)` via `Page` |
| Section title | `Typography(kind = H3)` via `Section` |
| Card / row title | `Typography(kind = BodyLg)` or H4 |
| Body | `Typography()` (Body) |
| Secondary line, metadata | `Typography(muted = true, kind = BodySm)` |
| Overline / label above a value | `Typography(kind = Overline, muted = true)` |
| Big number | `Typography(kind = H2)` |
