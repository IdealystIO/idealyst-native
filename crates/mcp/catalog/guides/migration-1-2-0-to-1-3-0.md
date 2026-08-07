+++
title = "Migrating 1.2 → 1.3"
order = 908
tags = ["migration", "1.3.0", "breaking", "theme", "tokens", "stylesheet", "theme_token"]
+++

# Migrating 1.2 → 1.3

1.3.0 makes theme tokens **typed**. A `stylesheet!` names a token through
its block binding — `t.spacing.md()` — instead of a string literal. There
is **one breaking change** (two macros removed), and it's mechanical.

Everything else about styling is unchanged: `Tokenized::token(name,
fallback)` still works, `<()>` sheets still work, and resolution, premint,
class hashing, and theme swap are byte-for-byte what they were. Each
accessor returns the same `Tokenized::Token { name, fallback }` the string
form built.

## 1. BREAKING: `theme_token!` / `theme_length!` removed

Both macros are gone. They were the compile-checked **string** form; the
token vocabulary supersedes them by making the name a path.

```rust
// before
background: theme_token!("color-surface"),
padding:    theme_length!("spacing-lg"),

// after — declare the theme in the sheet's slot, name tokens off the binding
stylesheet! {
    Sidebar<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),
            padding:    t.spacing.lg(),
        }
    }
}
```

**Do:** grep for `theme_token!` / `theme_length!`. For each, change the
sheet's `<…>` slot to your theme type (`IdeaThemeRef` for idea-ui), change
the block's binding from `_t` to `t`, and rewrite the call as the accessor
path. The mapping is mechanical because **the accessor path is the token
name**:

| token | accessor |
| --- | --- |
| `color-surface-alt` | `t.color.surface_alt()` |
| `intent-primary-solid-bg` | `t.intent.primary.solid_bg()` |
| `spacing-md` | `t.spacing.md()` |
| `radius-pill` | `t.radius.pill()` |
| `typography-body-size` | `t.typography.body_size()` |

Outside a `stylesheet!` — component code building `StyleRules` by hand —
`idea_ui::tokens()` hands back the same namespace:
`tokens().color.text_muted()`.

For a token name only known at runtime, `theme_color(name)` /
`theme_length(name)` (the functions, not the macros) are unchanged.

## 2. Not breaking, but worth a grep: dead token names

The typed path makes an unknown token a **compile error**. A string
couldn't do that — `Tokenized::token("typography-size-md", …)` compiled
fine, resolved to nothing, and rendered its fallback forever in every
theme. Converting this repo surfaced eight such names that had been
shipping.

If a sheet of yours stops compiling because no accessor matches, that name
was never resolving. Two honest fixes:

- **A real token has the same value** → use it (`typography-size-md` was
  14px, exactly `typography-body-size`), and the value becomes themeable
  for the first time.
- **Nothing matches** → write the literal (`font_size: Length::Px(11.0)`).
  Rendering is identical; you've just stopped claiming it was themeable.

Same for a **fallback that drifted** from your palette. Fallbacks now come
from the theme's own base palette rather than being restated at the call
site, so a converted sheet may pick up the palette's value pre-install.
Post-install nothing changes — the registry always won.

## 3. `<()>` sheets and app-defined tokens: no change

A sheet that declares no vocabulary keeps working exactly as before, and
`Tokenized::token("my-app-token", fallback)` remains the supported way to
reference a token no vocabulary describes. The typed path is an addition,
not a replacement — the two forms can appear in the same block.

```rust
stylesheet! {
    Panel<IdeaThemeRef> {
        base(t) {
            background: t.color.surface(),                              // vocabulary
            gap: Tokenized::token("app-gutter", Length::Px(20.0)),      // app-defined
        }
    }
}
```

See [[styling]] for the grammar and [[theming]] for the full token table.
