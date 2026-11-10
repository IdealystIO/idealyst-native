+++
title = "Generated docs and live component previews"
order = 63
tags = ["docs", "idealyst docs", "preview", "previews", "prop controls", "storybook", "prop_control", "PropControl", "example", "component library"]
+++

# Generated docs and live component previews

`idealyst docs [PROJECT]` builds and serves a documentation site for a
project, generated from its code: every `#[component]` in the project and
in its component-library dependencies, plus the primitives, utilities,
types, recipes and guides it can see. Each component page stacks three
sections:

- **Playground** — the live component on a canvas (full width, or framed at
  a phone's width), a panel with a control for each prop, "Reset props",
  and a **Usage** snippet: the `ui!` call the current controls make, with
  props still at their default left out, ready to copy.
- **Props** — the full props table, then composition, methods and
  animations.
- **Recipes** — each recipe's source, rendered live when it takes no
  arguments.

"View source" opens the component's own code, as written.

```sh
idealyst docs                 # the project in the current directory
idealyst docs path/to/app --open --port 8300
```

Nothing has to be registered. A component gets a preview because it is a
`#[component]`, and a prop gets a control because of its type.

## Which props get a control

| Prop type | Control |
| --- | --- |
| `String` | text field |
| `bool` | switch |
| integers, `f32`, `f64` | number field (8- and 16-bit integers are bounded by their type) |
| `Option<T>` | a "Some" checkbox plus `T`'s control |
| an axis enum (`ButtonVariant`, a `stylesheet!` variant, any `VariantEnum`) | a picker over its built-in values |
| `TextAlign`, `TextTransform`, `FontWeight` | a picker over the variants |
| `ToneRef` | a picker over the built-in tones |
| `ImageSource` | a URL field; switching an optional image on starts at a sample image |
| `IconData` | a picker over the icons the docs site registers (a set of common Lucide glyphs) |
| `children: Vec<Element>` | a text field, rendered as one `text` child |
| `Element` (a slot like `action: Option<Element>`) | a text field, rendered as a `text` element |
| `Option<Rc<dyn Fn(..)>>` (zero to two arguments) | an "Attach handler" checkbox; attaching passes a no-op, so a component that renders differently with a handler (a close button) shows it |
| `Reactive<T>`, `Signal<T>`, `ReadSignal<T>` | `T`'s control |
| a type that implements `PropControl` | the control it describes |

A required callback (`Rc<dyn Fn()>`) has no row: it is always attached.
A prop whose type has none of these (a `Ref`, your own struct) has no control and keeps the value the props default to. That is
never an error. A component whose props struct is hand-written (no
`#[props]`, not inline props) previews with its defaults and no controls.

## Starting values: `#[prop(example = …)]`

A preview starts from the props' defaults (`#[prop(default = …)]`,
`#[component(default(…))]`, or the struct's `Default`). An empty label
makes a poor preview, so give a prop a starting value for docs only:

```rust
#[component]
fn Badge(
    #[prop(example = "New")] label: String,
    #[prop(default = 3, example = 12)] count: i32,
) -> Element { /* … */ }

#[runtime_core::props]
pub struct CardProps {
    #[prop(example = "Settings")]
    pub title: String,
    // …
}
```

`example` only affects the preview. Outside a docs build the attribute is
accepted and has no effect; `ui! { Badge() }` still gets the default. The
expression only compiles in docs builds, so it may use the preview API — an
image prop can start at `runtime_core::previews::SAMPLE_IMAGE_URL`.

This is the component's say in what a realistic value looks like. A type
can also offer a representative value of its own (see `example()` below);
that one is used when a preview needs a value out of nothing, such as an
optional prop switched on.

## Controls for your own types: `PropControl`

Implement `runtime_core::previews::PropControl` for a type to give it a
control. Write the impl inside `#[prop_control]`, which removes it from
every build except `idealyst docs` — dev and release builds never compile
it, and the `previews` module it names doesn't exist there.

```rust
use runtime_core::prop_control;

#[prop_control]
impl runtime_core::previews::PropControl for Rating {
    fn control() -> runtime_core::previews::Control {
        runtime_core::previews::Control::Number { min: Some(0.0), max: Some(5.0), step: 1.0, integer: true }
    }
    fn to_value(&self) -> runtime_core::previews::ControlValue {
        runtime_core::previews::ControlValue::Number(self.0 as f64)
    }
    fn from_value(value: &runtime_core::previews::ControlValue) -> Option<Self> {
        match value {
            runtime_core::previews::ControlValue::Number(n) => Some(Rating(*n as u8)),
            _ => None,
        }
    }
}
```

A control is plain data, and the docs site decides how to draw it:

- `Control::Text`, `Control::Bool`
- `Control::Number { min, max, step, integer }`
- `Control::Choice(keys)` — one of a set of string keys. Use it for any
  named set of values, not only enums (icons, presets): `to_value` gives
  the key, `from_value` maps a key back.
- `Control::Optional { inner, example }` — `example` is the inner value
  switching it on starts from (`Option<T>` fills it from `T::example()`).
- `Control::Group(parts)` — a struct edited part by part, each part a
  labelled control; its value is `ControlValue::Group`, one value per part.
- `Control::Handler` — a callback; inside `Optional` it is the "Attach
  handler" checkbox. Use it for your own callback-carrying type (idea-ui's
  `AlertClose` maps "attached" to its close button).

`from_value` returns `None` for a value it can't use; the preview then
keeps the prop's previous value.

These methods are optional:

- `fn example() -> Option<Self>` (default `None`): a representative value
  of the type. It is what an optional prop of the type starts at when its
  "Some" box is ticked — a sample image instead of an empty URL, a first
  icon instead of none. Override `example_value()` too only when
  `to_value` can't read your example back.
- `fn code(&self) -> Option<String>` (default `None`): the value written as
  `ui!` prop code (`"Save"`, `tone::Danger`, `Rating(3)`), for the Usage
  snippet. Without it the snippet shows `/* … */` for the prop. Spell what
  a prop of the type accepts: an axis value as its const
  (`ButtonVariant::Ghost`), a string inside an `Option` as
  `Some(String::from("…"))`. Override `value_code(&ControlValue)` instead
  when a built value can't say what it was made from (an element slot
  spells the text it was edited to). Your own `PropControl` impl wins over the
built-in picker an axis enum would otherwise get.

## Icons

The framework ships no icon pack, so an `IconData` prop's picker offers
whatever the renderer registered with
`runtime_core::previews::set_icon_choices(vec![("star", STAR), …])`. The
docs site registers a set of common Lucide glyphs at startup; the first is
the icon an optional icon prop starts at. An icon that isn't in the set
shows as no selection.

## What has no live preview

- **Components that open an overlay.** `Modal`, `Menu`, `SubMenu`,
  `Popover`, `Tooltip`, `HoverCard`, `ToastHost`, anything built on the
  `overlay` / `anchored_overlay` primitives, and anything that composes
  one of those — directly or through other components. A portal escapes
  the preview box, and an open one covers the page with a backdrop nothing
  on the page can dismiss. Their pages show props, composition and recipe
  source.
- **Generic components** (`fn List<T>(…)`): there is no single props type
  to build.
- **Recipes with parameters.** A zero-argument `recipe!` renders live next
  to its source; one that takes arguments shows source only.

## How it works

Previews exist only in builds with `runtime-core/previews`, which only
`idealyst docs` turns on (not `idealyst dev`, not release). In those
builds:

- `#[props]` and inline-props `#[component]` implement
  `previews::PreviewProps` for the props struct, picking each field's
  control from its type at compile time.
- `#[component]` exports a function returning the component's `Preview`,
  and the catalog entry records its symbol name. A zero-argument
  `recipe!` exports a render function the same way.
- The docs site links those functions by symbol. This reaches components
  in private modules and survives the web build's dead-code pass.
