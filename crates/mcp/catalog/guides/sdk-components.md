+++
title = "Implementing an SDK component"
order = 67
tags = ["sdk", "authoring", "registry", "scene", "payload", "handler", "lazy", "chunk", "extension", "third-party"]
+++

# Implementing an SDK component

This is the authoring-side guide: how to build a component that ships in its own
crate and mounts a real platform node. [[sdks]] is the consumer-facing index of
what exists; [[lazy-loading]] covers the app author's side of splitting.

## First: do you actually need one?

Most components don't. If yours composes existing primitives — a card, a form
row, a chart built from `view` and `text` — write a plain `#[component]` and
stop reading. It needs no registration, works on every backend for free, and
can't fail the way the rest of this guide describes.

You need an SDK component when you're wrapping something the framework has no
primitive for: a native map view, a video surface, a PDF canvas, a real HTML
`<table>`. That means a **payload type** and a **mount handler**, and those have
to be installed on the scene `Registry` before anything can draw them.

The framework's builtin primitive set stays deliberately small. Peripheral
features belong out here, in your crate, on the registry.

## The four pieces

```rust
use runtime_scene::{item, MountCx, Registry};
use runtime_core::{ui, Element};

// 1. The payload. Its TypeId is the registry's dispatch key, so two
//    crates can both ship a "MapView" without colliding. Keep it private
//    (see "Seal the payload" below).
struct MapPrim {
    center: (f64, f64),
    zoom: u8,
}

// 2. The mount handler. Receives the payload and a MountCx carrying the
//    real backend — fully typed, never `dyn Any`.
fn mount_map<H>(cx: &mut MountCx<'_, H>, prim: &Rc<MapPrim>, children: Vec<Element>) -> H::Node
where
    H: ExternalOps + StyleServices + 'static,
{
    // ...build the platform node...
}

// 3. The registration seam. See the naming rules below.
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    registry.register::<MapPrim, _>(mount_map::<H>);
}

// 4. `ui!` dispatch — a props struct, a tag alias, a BuildElement impl.
pub type MapView = MapViewProps;

impl BuildElement for MapViewProps {
    fn build(self) -> Element {
        item(MapPrim { center: self.center, zoom: self.zoom }, Vec::new())
    }
}
```

An unregistered payload panics at realize, by design. That's the loud failure —
not a reason to move the feature into the framework core.

## Name your seams exactly this

There are three, and they mean different things. Apps and tooling key off the
names, so don't invent variants.

| Function | Called from | What it does |
|---|---|---|
| `register(registry)` | the app's `register_scene_extensions`, at boot | Installs the handler now |
| `defer(registry)` | the app's `register_scene_extensions`, at boot | Declares the kind late-bound; installs nothing |
| `register_from_chunk()` | inside a `#[component(lazy)]` body | Installs the handler when the chunk lands |

Backend-concrete variants keep the **same name** under `cfg` — most SDKs have a
generic `register<H>` for native and a `register(&mut Registry<WebBackend>)` for
wasm. That's the convention, not an exception.

Two things not to do:

- **Don't invent a synonym.** `register_handlers`, `register_scene`,
  `register_generic` — all of these existed in the tree before 1.1.0 and all of
  them were renamed to `register`. Don't reintroduce one.
- **Never ship a no-op `register<B>(_backend: &mut B) {}`.** An unconstrained
  `B` accepts a `&mut Registry<H>` happily, so a caller who guesses the
  conventional name gets a function that compiles, does nothing, and leaves them
  debugging a panic they already "fixed". If your SDK has nothing to register on
  a target, omit the function on that target.

### Platform dispatch inside one seam

When the same payload needs different handlers per backend, a `cfg` alone won't
do it — `target_os = "ios"` is also true for a host-side SSR render inside an
iOS build graph. Dispatch on the registry's concrete type instead:

```rust
#[cfg(target_os = "ios")]
{
    let any: &mut dyn Any = registry;
    if let Some(reg) = any.downcast_mut::<Registry<backend_ios::IosBackend>>() {
        reg.register::<MapPrim, _>(mount_map_ios);
        return;
    }
}
registry.register::<MapPrim, _>(mount_placeholder::<H>);
```

`svg` is the reference implementation.

## Supporting lazy loading

Worth doing when your handler drags in real weight — a rasterizer, a font stack,
a parser, an embedded table. Not worth doing for a thin wrapper around a
platform view.

The mechanics are in [[lazy-loading]]. What matters here is that lazy always
needs **two** calls in two places, and that isn't a rough edge you can smooth
over:

- **At boot**, something must declare the kind, or realize panics the first time
  it meets your payload.
- **Inside the chunk**, something must install the handler. It has to be inside,
  because the split tool decides what goes in the chunk by asking what the
  startup code *doesn't* reach. If boot could install it, boot would reach it,
  and it'd be back in the main bundle.

### Ship a `defer`, because your payload type is private

The app-side recipe in [[lazy-loading]] shows `registry.defer::<HeavyProps>()`.
That only works if your payload type is public — and for most SDKs it isn't and
shouldn't be. `svg`'s `SvgPrim` is private today, which means **no app can defer
it**, no matter how much they want to.

It gets worse for multi-payload SDKs. `table` registers three, keyed on a
framework wrapper type. Without a seam, an app would have to write:

```rust
registry.defer::<PrimCell<TablePrim>>();
registry.defer::<PrimCell<TableRowPrim>>();
registry.defer::<PrimCell<TableCellPrim>>();
```

— importing framework internals to do it. Nobody will get that right.

So expose the declaration as a function and keep your types to yourself:

```rust
/// Declare this SDK's payload kinds late-bound. Pair with
/// `register_from_chunk()` from inside a `#[component(lazy)]` body.
pub fn defer<H: runtime_scene::Host>(registry: &mut Registry<H>) {
    registry.defer::<MapPrim>();
}
```

Now the app writes one line either way — `map::register(registry)` or
`map::defer(registry)` — and the verb is the whole decision.

### Prefer a feature flag over two spellings

Better still, make it a build-time choice so consumers can't get the two halves
out of sync. This is ordinary `cfg`; nothing framework-level is missing.

```toml
[features]
lazy = []
```

```rust
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    #[cfg(feature = "lazy")]
    registry.defer::<MapPrim>();

    #[cfg(not(feature = "lazy"))]
    registry.register::<MapPrim, _>(mount_map::<H>);
}
```

Then ship the chunk boundary yourself, so the consumer never writes the chunk
call at all:

```rust
#[cfg(feature = "lazy")]
#[component(lazy)]
pub fn MapView(center: (f64, f64), zoom: u8) -> Element {
    register_from_chunk();
    ui! { MapViewInner(center = center, zoom = zoom) }
}
```

The app's side collapses to one unchanging line plus a manifest flag:

```toml
map = { version = "1", features = ["lazy"] }
```

> **Unverified.** No SDK in this repo ships its own `#[component(lazy)]`
> boundary yet. The split tool analyses the finished binary, so a boundary
> declared in a dependency *should* behave the same as one in the app — but that
> has not been measured. Prove it with a bundle-size check before relying on it;
> `tests/lazy-payload-split` is the pattern to copy.

### Keep `loading` and `error` in both modes

`#[component(lazy)]` adds `loading` and `error` props. A component that only has
them under a feature flag is a component whose public API changes when the flag
flips — call sites that customise the loading state stop compiling when the flag
goes off, and have no way to set it when it goes on.

Declare both props in both builds and ignore them when eager. The flag should be
a bundling switch, not a breaking change.

### Seal the payload

When lazy is on, your payload must be **unreachable except through the lazy
component**. If any other public path can produce one — a lower-level builder, a
re-exported sub-piece, a cell type usable on its own — that piece lands with no
handler and no chunk coming:

- as an ordinary child, it's **invisible forever**, with no error at all;
- as a screen, a `when` branch, or a list row, it **panics immediately**,
  because a parked item needs a parent and an index and a subtree root has
  neither.

Rust's own visibility enforces this. Keep the payload private and let the lazy
component be the only public door, and the failure becomes impossible to write
rather than something consumers have to remember.

## What you own vs what the consumer owns

**The consumer picks the loading and error UI.** They pass closures at the call
site; you don't get a say, and shouldn't want one.

**You decide whether retry exists.** Re-attempting a load means re-sending the
props, which needs them cloneable, so retry only works if you declared
`#[component(lazy, retryable)]`. If you didn't, `LazyError::retry()` still
exists and does nothing — a consumer can wire a dead button. Document which one
you shipped.

Warn consumers about the defaults while you're at it: an unset `loading` renders
**nothing**, and an unset `error` leaves the loading UI up permanently, so a
failed load is indistinguishable from a slow one.

## Failure modes to design against

| Situation | What the consumer sees |
|---|---|
| Seam never called | Panic at realize, naming only a raw `TypeId` |
| Deferred, chunk never loads | Nothing. No panic, no log, no placeholder |
| Payload escapes the lazy wrapper, as a child | Invisible, silently |
| Payload escapes the wrapper, as a subtree root | Immediate panic |
| Server render | Chunks never load — your `loading` UI is the final HTML |

That last one deserves a line in your README. If your component holds content
that matters for a server-rendered page, lazy is the wrong default for it.

## Before you publish

- [ ] Seam is named `register`, not a synonym.
- [ ] No no-op `register<B>(_backend: &mut B)` anywhere.
- [ ] Backend-concrete arms share the seam name under `cfg`.
- [ ] Payload types are private; anything an app must name has a function.
- [ ] If you support lazy: `defer` exists, `register_from_chunk` exists, and the
      payload is unreachable outside the lazy component.
- [ ] `loading` / `error` props present in both eager and lazy builds.
- [ ] Retry support stated in the docs either way.
- [ ] Bundle-size delta actually measured, not assumed.

## See also

- [[sdks]] — the consumer-facing index of available SDKs
- [[lazy-loading]] — the app author's side: splitting, chunks, `--data-prune`
- [[idiomatic-components]] — the house style every component follows
- [[component-hygiene]] — props, callbacks, and reactive-scope rules
