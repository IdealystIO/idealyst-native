+++
title = "Carrying the user's brand into idea-ui"
summary = "Brand intake (what to ask, what never to invent), mapping a brand onto theme tokens, light + dark, then shape language via set_default and restyle — all in one brand.rs, never per instance."
when = "The user gives brand colors, fonts, a logo, a style reference — or asks to change how the whole app looks."
order = 60
tags = ["brand", "theme", "tokens", "restyle", "dark-mode", "fonts", "component-overrides"]
+++
# Carrying the user's brand into idea-ui

## Intake: use what they gave, ask for the rest

The brand is the user's. Collect, in this order:

1. **Primary color** — and ideally its scale (tint, base, darker text
   shade). One hex is enough to start; ask for their palette before
   inventing tints.
2. **Neutrals** — do they have brand grays? If not, keep idea-ui's.
3. **Typeface(s)** — body and (optionally) headings, with the weights they
   use. It must ship on every target (or be a system font).
4. **Shape** — sharp, soft or round corners; pill buttons or not.
5. **Density** — comfortable (default) or compact.
6. **Dark mode** — they always get one; ask whether they have dark brand
   colors.

**Never invent**: a second accent color, gradients, illustration styles,
a different font. Where the brand is silent, idea-ui's default stands —
and you list each assumption for the user at the end.

**Contrast is not optional.** Text on `solid_bg` and the `fg` color on the
page must reach 4.5:1. If the brand color fails as text (a light yellow,
a mint), it stays the fill, and a darker shade of *their* scale becomes
`fg` — ask for it rather than eyeballing one.

## The three levers, strongest first

All of them live in `src/brand.rs` and run in `brand::install()`, the first
call in `app()` (the `brand_module` recipe is the template).

### 1. Theme tokens (`IdeaThemeDefaults`)

Start from `light_theme()` / `dark_theme()` and set only what the brand
specifies:

| Brand input | Token(s) |
| --- | --- |
| primary color | `intents.primary` — all six slots: `solid_bg`, `solid_text`, `soft_bg`, `soft_text`, `fg`, `border` |
| focus color | `colors.focus_ring` (usually the primary) |
| brand grays | `colors.background`, `surface`, `surface_alt`, `text`, `text_muted`, `border`, `border_hover`, `border_strong` |
| corner style | `radius.sm / md / lg` (sharp ≈ 2/4/6, default 4/8/12, round ≈ 6/10/16) |
| typeface | `font` (`FontFamily::System("Brand, system-ui, sans-serif")` or a registered `Typeface`) |
| type scale | `typography.*_size` — only if the brand defines one |
| density | `spacing.*` — scale all six together, never one |

- Install **both schemes**: `install_idea_theme_schemes(light(), dark())`.
  Build both from one shared function so shape and type can't drift.
- Use `Tokenized::Literal(Color(..))` for values — the field's canonical
  token name is kept, so dark-mode swaps still work.
- Semantic intents (success/warning/danger/info) stay idea-ui's unless the
  brand defines them: users read red as danger regardless of brand.

### 2. Axis defaults (`XStyles::set_default`)

The brand's shape language, app-wide:

- pill buttons → `ButtonStyles::set_default(ButtonShape::Pill)`
- filled fields → `FieldStyles::set_default(FieldAppearance::Contained)`
- elevated cards → `CardStyles::set_default(CardVariant::Elevated)`

### 3. Slot restyles (`XStyles::restyle`)

Details a token can't say — a heavier button label, a borderless card, a
table header band:

```rust
ButtonStyles::restyle(|s| {
    s.label().base(|_| StyleRules { font_weight: Some(FontWeight::SemiBold), ..Default::default() });
});
```

- A restyle **merges** into the component's sheet (still premints) — set
  only the fields you change.
- Write restyles against **tokens** (`t.color.surface()`), never literals,
  so they follow light/dark.
- Every component's slots are documented on its `*Styles` type
  (`ButtonStyles`, `FieldStyles`, `TableStyles`, …); the base MCP's
  `design-systems` guide is the full mechanics.
- A brand-specific *look* the component lacks (a "glass" card) is a new
  axis value: `axis_values! { pub trait BrandCards for CardVariant { Glass } }`
  plus one `.arm(CardVariant::Glass, …)` restyle.

## What not to do

- **No per-instance branding.** `styles = CardSlotStyles::default()…` at a
  call site is for a genuinely one-off layout need (a card flush against
  an image), not for brand.
- **No wrappers to restyle.** `fn BrandButton(..) { Button(..) }` that only
  changes looks forks the design system; restyle `ButtonStyles` instead.
  A wrapper is fine when it adds *behavior* (an analytics hook, a
  confirm step).
- **No brand colors in screens.** Screens say `tone::Primary`; the brand
  says what Primary is.
- **No restyles after boot.** Restyles register in `install()`; later ones
  only reach new elements and miss the premint.

## Done when

- [ ] `brand.rs` is the only file with a color literal or font name.
- [ ] Light and dark both installed; text passes 4.5:1 on its surfaces in both.
- [ ] Every brand value traces to something the user gave; assumptions listed.
- [ ] Screens changed by zero lines.
