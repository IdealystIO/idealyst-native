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
  then two code blocks, each with its own Copy button: the imports (the `use` lines the snippet
  needs, by the shortest public path the component crate offers — `use
  idea_ui::{Alert, tone};`) and the usage (the `ui!` call the current
  controls make, props still at their default left out). Each row whose
  default isn't simply empty says what it is ("Default: Soft"), so a
  prop missing from Usage is never a mystery. The canvas is a near-white
  neutral outside the theme's palette under a dot grid: borders read as
  they do on a page, and any filled box hides the dots, so a component's
  surface shows even where its color matches the canvas.
- **Props** — the full props table, then composition, methods and
  animations.
- **Recipes** — each recipe's source as written, rendered live when it
  takes no arguments.

"View source" opens the component's own code, as written.

The sidebar shows one library at a time: the crate the docs were built
for, each crate it depends on (a component library like idea-ui), and the
framework's own primitives, utilities and guides. It opens on the project;
the card at its top switches libraries, with a search for crates by name.
Opening an entry from another library (from search, or a link on a page)
moves the sidebar to that library.

A component library lists only the components it exports: a `#[component]`
a caller can't name (a private helper, say a table's row toggle) has no
page. A crate that exports no component at all (an app) lists all of its
own.

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
| a unit-only enum with `#[derive(PropControl)]` | a picker over its variants |
| `TextAlign`, `TextTransform`, `FontWeight`, `ElementSide`, `ElementAlign` | a picker over the variants |
| `ToneRef` | a picker over the built-in tones |
| `ImageSource` | a URL field; switching an optional image on starts at a sample image |
| `IconData` | a picker over the icons the docs site registers (a set of common Lucide glyphs) |
| `children: Vec<Element>` | a text field, rendered as one `text` child |
| `Element` (a slot like `action: Option<Element>`) | a text field, rendered as a `text` element |
| `Option<Rc<dyn Fn(..)>>` (zero to two arguments) | an "Attach handler" checkbox; attaching passes a no-op, so a component that renders differently with a handler (a close button) shows it |
| `Reactive<T>`, `Signal<T>`, `ReadSignal<T>` | `T`'s control |
| a type that implements `PropControl` | the control it describes |

A required callback (`Rc<dyn Fn()>`) has no row: it is always attached.
A prop whose type has none of these (a `Ref`, your own struct, a
`Vec` of rows) has no control and keeps the value the props default to,
or its `#[prop(example = …)]` (below). That is never an error. A component whose props struct is hand-written (no
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

### Data and children

An example also works on a prop with no control, and that is where it
matters most: a component built from data (`items`, `options`, `tabs`)
previews as nothing without some. The preview shows the example, and the
Usage snippet spells it exactly as written, comments and layout included:

```rust
#[props]
pub struct BreadcrumbsProps {
    #[prop(example = vec![
        Crumb::linked("Home", Rc::new(|| {})),
        Crumb::new("Settings"),
    ])]
    pub items: Vec<Crumb>,
    // …
}
```

On a children slot, an example replaces the editable text with real
children, given as a `ui!` block (or a `Vec<Element>`); Usage shows it as
the call's child block. Give containers a few children, so their layout
props (`gap`, `axis`, `align`) visibly do something:

```rust
#[component]
fn Toolbar(
    #[prop(example = ui! {
        Button(label = "Save")
        Button(label = "Cancel")
    })]
    children: Vec<Element>,
) -> Element { /* … */ }
```

Write examples against your crate's public names (`Crumb`, not
`crate::breadcrumbs::Crumb`): the Imports block finds the names an example
mentions among the crate's public items, plus `signal` and `Rc`. It never
lists `ui!`: the snippet is written inside one.
Names an example uses only in docs builds need importing in the component's
module; mark that `use` `#[allow(unused_imports)]`, since other builds
don't compile the example.

## Controls for your own types: `PropControl`

For a unit-only enum, derive it:

```rust
#[derive(Clone, Copy, Default, PartialEq, runtime_core::PropControl)]
pub enum Expand {
    #[default]
    Single,
    Multi,
}
```

The derive gives a picker over the variants, spelled `Expand::Multi` in
Usage and imported from the enum's module. Like `#[prop_control]`, it
expands to nothing outside `idealyst docs` builds. An enum whose variants
carry data needs a hand-written impl.

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
  spells the text it was edited to).
- `fn imports(value: &ControlValue) -> Vec<String>` (default empty): the
  paths that spelling needs in scope, at their defining paths
  (`"std::rc::Rc"`, `concat!(module_path!(), "::MyType")`). The docs page
  rewrites each to the component crate's root re-export when it has one. Your own `PropControl` impl wins over the
built-in picker an axis enum would otherwise get.

## Icons

The framework ships no icon pack, so an `IconData` prop's picker offers
whatever the renderer registered with
`runtime_core::previews::set_icon_choices(vec![("star", STAR), …])`. The
docs site registers a set of common Lucide glyphs at startup; the first is
the icon an optional icon prop starts at. An icon that isn't in the set
shows as no selection.

## Context and server data: `preview_setup`

A component that reads context (`inject`) or fetches from a `#[server]`
function has neither in the docs: nothing provided its context and there
is no server. `#[component(preview_setup = …)]` names a `Fn()` the preview
runs before each build of the component, in the preview's own scope:

```rust
#[component(preview_setup = account_card_preview)]
pub fn AccountCard() -> Element {
    let session = inject::<Session>()…;
    let confirmed = resource(session.user, |_user, _cancel| me());
    …
}

fn account_card_preview() {
    provide(Session { user: signal(Some("ada".to_string())) });
    server::mock::on("me", |()| -> Result<String, ServerError> { Ok("ada".to_string()) });
}
```

- `provide` in the setup is what the component's `inject` finds. Signals
  made there belong to the preview and are freed with it.
- `server::mock::on(path, handler)` answers a `#[server]` call in place of
  the network, so the `resource` above it goes through loading to loaded
  as it does in the app. Return `Err(ServerError::…)` from the handler to
  show the error state, or call `server::mock::pending(path)` to hold the
  loading state. See `docs/server-functions.md` §6.
- The setup is a path or a closure (`preview_setup = || provide(Theme::demo())`).

The setup runs only in a preview. Other builds type-check it and emit
nothing for it, so a fixture that stops compiling is caught by an ordinary
build. A zero-argument recipe needs no attribute: it is a function, so it
calls `provide` or `server::mock` itself before rendering. `examples/login-demo`'s
`AccountCard` is the working example.

Fetches that don't go through a `#[server]` function (a raw `net` request,
another SDK's transport) are not mocked; have the component take its data
source from context and provide a stand-in.

## What has no live preview

- **Components that open an overlay.** `Modal`, `Menu`, `SubMenu`,
  `Popover`, `ToastHost`, anything built on the `overlay` /
  `anchored_overlay` primitives, and anything that composes one of those —
  directly or through other components. A portal escapes the preview box,
  and an open one covers the page with a backdrop nothing on the page can
  dismiss. Their pages show props, composition and recipe source.
  `Tooltip` and `HoverCard` are the exception: they open only while the
  pointer rests on the trigger they wrap, so they — and components built
  on them, like `IconButton` — keep a live preview.
- **Generic components** (`fn List<T>(…)`): there is no single props type
  to build.
- **Recipes with parameters.** A zero-argument `recipe!` renders live next
  to its source; one that takes arguments shows source only.

Handlers in the playground are no-ops, so clicking a controlled component
there (a radio, a tab) doesn't move its value; edit it in the props panel.
A recipe owns its signals, so its live render responds like the real
thing — that is the place to show behavior.

## How it works

Previews exist only in builds with `runtime-core/previews`, which only
`idealyst docs` turns on (not `idealyst dev`, not release). In those
builds:

- `#[props]` and inline-props `#[component]` implement
  `previews::PreviewProps` for the props struct, picking each field's
  control from its type at compile time.
- `#[component]` exports a function returning the component's `Preview`,
  and registers its symbol name with the catalog. A zero-argument
  `recipe!` exports a render function the same way.
- The docs site links those functions by symbol. This reaches components
  in private modules and survives the web build's dead-code pass.
