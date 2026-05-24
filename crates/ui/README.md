# `ui/` — component library + extras

Optional component library that ships on top of the Runtime. Apps
can use it, replace pieces of it, or skip it entirely — nothing in
the framework depends on `idea-ui`. It's here because most apps
want a Heading, a Card, a Stack, a Button-with-themed-colors out
of the box rather than reimplementing them.

| Crate | Path | Role |
| --- | --- | --- |
| `idea-ui` | [`idea-ui/`](./idea-ui) | The library itself. Heading, Body, Card, Stack, Btn, Caption, theme tokens, breakpoints. Pure composition over Runtime primitives. |
| `idea-ui-docs-derive` | [`idea-ui-docs-derive/`](./idea-ui-docs-derive) | Derive macro that emits documentation metadata for `idea-ui` components. Used by the docs site to auto-generate prop tables. |
| `icons-lucide` | [`icons-lucide/`](./icons-lucide) | Lucide icon set wrapped as an `IconRegistry`. Consumed by the `Icon` primitive. |

## Relationship to the framework

`idea-ui` is to Idealyst what Tailwind UI is to Tailwind: an
opinionated default that pulls from the underlying tokens. It uses
the same primitives, the same stylesheet machinery, the same
reactivity. Apps that want a different design system can build one
the same way `idea-ui` is built.

## What lives here vs. SDK

- `ui/` — composition over existing primitives. Pure Rust, every
  Backend.
- `sdk/` — defines *new* primitives that need per-Backend impls
  (WebView, Maps, etc.) using `Primitive::External`.

If you only need to compose existing primitives differently, that's
a component crate — belongs in `ui/` (or in your own app). If you
need to render something the Backend Interface doesn't know about,
that's an SDK crate.
