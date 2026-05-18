# Docs site features

Meta-decisions about how the docs site itself behaves — features the
content depends on but that aren't part of any single page's content.

## "Coming from..." comparison picker

A tab bar inside the comparison card (not in the chrome) lets the
reader pick which framework's comparison to view: React, Vue, Svelte,
or Solid. The selection drives which comparison callout is visible
inside that card.

The tabs live on the card itself — every page that has comparisons
shows the same set of tabs, with the active selection persisted
across pages.

React and React Native are treated as one option here. The
differences between them don't show up at the levels the docs talk
about (reactivity model, component lifecycle, theming), so a single
"From React" prefix covers both.

### Authoring convention

Comparison blocks in the content-plan markdown use a labeled
blockquote with a bolded prefix and an em-dash:

```markdown
> **From React.** Body text starts here. Multiple sentences are fine;
> the blockquote continues until a blank line ends it.

> **From Solid.** Another framework's comparison goes in its own
> blockquote, immediately after.
```

The supported prefixes (case-sensitive, period after the framework
name):

- `**From React.**`
- `**From Vue 3.**`
- `**From Svelte 5.**`
- `**From Solid.**`

When porting to the real `ui!`-rendered docs site, these blocks become
a `Comparison(from = "react")` component (or similar). The card's
render function reads a `from_framework` signal and shows only the
matching block.

### Where comparisons belong

Add a comparison only where the mental model genuinely differs.
Examples of good places:

- The reactivity model (signals vs `useState`)
- Component lifecycle (runs-once vs runs-every-render)
- Theming (signal-driven resolution vs Context re-render)
- The app/backend split (renderer model vs compiler-emit model)

Examples of bad places — skip the callout if the mapping is obvious:

- Naming differences with no semantic difference
- Anything where the answer is "it's the same idea, different syntax"
  without further nuance

### Default

The default selection is **React**. Most readers will come from there,
and seeing the comparison rendered without having to click anything
demonstrates that the feature exists. The tab bar makes switching to
another framework one click away.

### Persistence

The selection should persist across pages (and across sessions, via
localStorage on web). Switching pages shouldn't reset it.

## (Future) Code-sample language tabs

Some samples will eventually have variants — for example, the same
component written in `ui!` and in `jsx!`. Same UI pattern as the
"Coming from..." picker: a per-page toggle that selects which variant
is visible. Treat this as a separate feature; the comparison picker
shouldn't try to do double duty.
