+++
title = "Extensible Design Systems: Component Styles"
order = 46
tags = ["style", "theme", "components", "design-system", "restyle", "variants"]
+++

# Extensible Design Systems: Component Styles

A theme ([[theming]]) changes the *values* every component reads — colors,
spacing, radii. This guide is about changing the *components*: giving every
Button a pill shape, adding a `raised` variant, making fields filled by
default, restyling one segment of one control. idea-ui is built this way, and
so can any component library you write.

The model has three parts:

- A component declares its styled sections once, as named **slots**, with
  `component_styles!`. Each slot is a `stylesheet!` body.
- An app **restyles** any slot app-wide at boot. The restyle is *composed into*
  the slot's sheet — it is not a runtime override — so restyled components
  still premint to static CSS.
- Variant **axes are open**: an app adds its own value (`ButtonVariant::define("raised")`)
  and gives it a look with one restyle arm. Each component's axis is its own
  type, so a Card variant never compiles on a Button.

Per instance, every component also takes a `styles` prop that overrides any
of its slots on that one element.

## Declaring a component's styles

A third-party library declaring a `Banner` component:

```rust
use runtime_core::{component_styles, FlexDirection, FontWeight, StyleRules};
use idea_ui::IdeaThemeRef; // or your own token vocabulary

component_styles! {
    /// The styled sections of [`Banner`].
    pub Banner<IdeaThemeRef> {
        /// How loud the banner is. Declared at component level so every slot
        /// that styles it agrees on its values and default.
        axis emphasis { #[default] quiet, loud }

        /// The bar.
        slot root {
            base(t) {
                flex_direction: FlexDirection::Row,
                gap: t.spacing.sm(),
                padding: t.spacing.md(),
                background: t.color.surface_alt(),
                border_radius: t.radius.md(),
            }
            variant emphasis {
                quiet(_t) {}
                loud(t) { background: t.intent.primary.solid_bg() }
            }
        }

        /// The message text.
        slot message {
            base(t) { color: t.color.text(), font_weight: FontWeight::Medium }
            variant emphasis {
                quiet(_t) {}
                loud(t) { color: t.intent.primary.solid_text() }
            }
        }
    }
}
```

That generates:

| Item | What it is |
| --- | --- |
| `BannerStyles` | The registry key. `BannerStyles::root()` / `::message()` return each slot's style; `::restyle(…)`, `::set_default(…)`, `::values()`. |
| `BannerEmphasis` | The axis key type, with `BannerEmphasis::Quiet` / `::Loud`, `BannerEmphasis::define("…")` for app values, `Default`, and (for an on/off axis) `From<bool>`. |
| `BannerRestyle` | What a restyle closure receives: `s.root()`, `s.message()`. |
| `BannerSlotStyles` | The per-instance overrides: one optional override per slot, with same-named setters. |

A slot body may open with `extend(path)` — a `fn(&mut SlotRestyle<'_, BannerStyles>)`
the library runs at every composition to add computed arms (idea-ui renders
each tone × variant arm this way, so an app-registered tone appears in every
tone-aware component).

## Using the slots in the component

```rust
#[runtime_core::props(styles = BannerStyles)] // adds `styles: BannerSlotStyles`
pub struct BannerProps {
    pub message: Reactive<String>,
    pub emphasis: BannerEmphasis,               // reactive, like any data prop
}

#[component]
pub fn Banner(props: &BannerProps) -> Element {
    let root = BannerStyles::root()
        .set(props.emphasis.clone())           // a static or live axis value
        .overridden(props.styles.root.as_ref());
    let message = BannerStyles::message()
        .set(props.emphasis.clone())
        .overridden(props.styles.message.as_ref());
    ui! {
        view(style = root) {
            text(style = message) { props.message.clone() }
        }
    }
}
```

`SlotStyle` also takes `.values(&shared)` (one `AxisValues` set shared by
several slots), `.raw(axis, key)` for an axis read as a string,
`.inline(rules)` / `.inline_with(|| rules)` for continuous values (a measured
width) that keep the slot premintable, and `.into_style_application()` when
code needs the application itself.

## Restyling app-wide

At boot — inside `app()`, before the tree is built:

```rust
BannerStyles::restyle(|s| {
    s.root().base(|t| StyleRules { border_radius: Some(t.radius.pill()), ..Default::default() });
    s.message().arm(BannerEmphasis::Loud, |t| StyleRules {
        font_weight: Some(FontWeight::Bold),
        ..Default::default()
    });
});
```

`base`, `arm`, `state` (`StateBits::FOCUSED`, …), `breakpoint` and `compound`
(a tuple of axis values and states) each add a layer. A restyle MERGES: fields
it leaves unset keep the declaration's values. Later restyles win over earlier
ones; re-running the same `restyle` call site replaces its earlier entry, so
re-running `app()` doesn't stack copies.

Ordering inside a slot is the stylesheet's: base, then axis arms (in axis-name
order), then states, then compounds — and compounds outrank states. A state
overlay that must win over a compound is itself a compound with more legs.

**Install restyles at boot.** A restyle registered after a slot was handed to
an element still applies to new elements, but the built ones keep their style
and a premint build has no CSS for the new composition; debug builds warn once.
Installing or swapping a theme does not disturb restyles.

## Adding a variant value

```rust
pub const URGENT: BannerEmphasis = BannerEmphasis::define("urgent");

BannerStyles::restyle(|s| {
    s.root().arm(URGENT, |t| StyleRules { background: Some(t.intent.danger.solid_bg()), ..Default::default() });
    s.message().arm(URGENT, |t| StyleRules { color: Some(t.intent.danger.solid_text()), ..Default::default() });
});

ui! { Banner(message = "Disk full".into(), emphasis = URGENT) }
```

`define` checks at compile time that the key is a class-safe name
(`[a-z0-9_]`). A value with no arm anywhere renders like the base, and debug
builds warn once naming the component, axis and value.

`BannerStyles::set_default(URGENT)` (or any value) changes the axis default
for every instance that doesn't pass one — `BannerEmphasis::default()` returns
it too.

### Tone-aware values

idea-ui's tone-aware components render a *recipe* — an
`idea_theme::extensible::Variant` impl returning a surface for a tone — over
every registered tone. An app value gets the same treatment from
`tone_variant_arms(&mut s.root(), &semantic_tones(), VALUE.key(), &Recipe)`;
Button bundles the box, its hover/press feedback and the label foreground as
`ButtonStyles::restyle(|s| s.tone_variant(RAISED, &Raised))`. The opt-in
accent gradient is the same call with `variant::Gradient`.

## Per-instance overrides

```rust
Banner(
    message = "Saved".into(),
    styles = BannerSlotStyles::default().root(StyleRules { padding: Some(Length::Px(4.0).into()), ..Default::default() }),
)
```

An override is plain rules, optionally with arms matched against the slot's
current axis selection — so one override can style only the selected segment
of a segmented control:

```rust
let t = idea_ui::tokens();
SegmentedControlSlotStyles::default().segment(
    SlotOverride::from(StyleRules::default())
        .arm(SegmentedControlSelected::On, StyleRules { background: Some(t.intent.primary.solid_bg()), ..Default::default() }),
)
```

It rides the inline layer: it wins over the composed sheet and keeps the
element premintable. It is data, resolved at apply time — which is why it
takes rules rather than a sheet: a `--premint-only` bundle compiles every
sheet's rule closures out.

## Type safety and tooling

Each component's axis values are its own type. `Card(variant = ButtonVariant::Ghost)`
does not compile, and neither does a restyle arm keyed by another component's
value. For MCP completion, every declared axis value is a `list_values` entry
under its axis type (`list_values("ButtonVariant")`). Under the
`idealyst_premint_dump` cfg every linked component composes during the dump —
a component no crawled screen rendered still ships its restyled CSS — and
`runtime_core::component_styles::composed_slots()` names each composed slot.

## Branding idea-ui

`websites/idea-ui-docs-branded/src/brand.rs` is the worked example: the whole
idea-ui docs site under a second design language ("Ink") — filled fields with
soft corners and an ink focus border, an ink segmented track with a
primary-filled selection, pill buttons, borderless paper cards (an app Card
value made the default), ink badges and table headers — installed by one boot
hook and nothing else:

```rust
pub fn app() -> Element {
    idea_ui_docs::app_with(brand::install)
}
```

Run it with `idealyst dev --web --local` from `websites/idea-ui-docs-branded`.
Every idea-ui component's slots are documented on its `*Styles` type
(`ButtonStyles`, `FieldStyles`, `TableStyles`, …).
