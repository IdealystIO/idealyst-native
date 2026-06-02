# `svg`

An `Svg` primitive for the idealyst framework — render SVG markup
inside your native UI tree. Built on the framework's `Element::External`
extension mechanism, so it's not part of runtime-core: an app opts in by
depending on this crate and calling `svg::register(&mut backend)` once
at bootstrap.

Single-crate, `cfg`-gated — one crate ships every backend, selected at
compile time. Renders the **same SVG spec** everywhere; only the
mechanism diverges (the browser's own SVG engine on web, a `usvg`-based
native vector renderer on iOS/Android).

```rust,ignore
use svg::prelude::*;
use runtime_core::{signal, Ref};
use std::rc::Rc;

// App bootstrap — one line per third-party SDK:
let mut backend = WebBackend::new("#app");
svg::register(&mut backend);

// Inside a `ui!` block. `Svg` is an external primitive, so it's
// interpolated as an expression:
let markup = signal(LOGO_SVG.to_string());
let r: Ref<SvgHandle> = Ref::new();
ui! {
    View {
        { svg::Svg(SvgProps {
            markup: svg::markup(move || markup.get()),
            on_load: Some(Rc::new(|| log::info!("svg parsed"))),
            ..Default::default()
        }).bind(r.clone()) }
    }
}

// Read the parsed SVG's natural dimensions (None until first render):
let size = r.with(|h| h.intrinsic_size());
```

## What you get

Every backend renders the same SVG document and converges on the same
output. The *mechanism* differs per platform:

| Target | Mechanism |
| --- | --- |
| Web (wasm32) | `innerHTML` into a wrapper `<div>` — the browser is the SVG renderer |
| iOS | `usvg` parse → replay into a `UIView` subclass's `drawRect:` `CGContext` (resolution-independent, no raster step) |
| Android | `usvg` parse → walk into a `Picture` → `PictureDrawable` on an `ImageView` (scales with bounds, no raster step) |
| Other (wgpu desktop, terminal, …) | the framework's `External` "not supported" placeholder |

The native backends re-draw the parsed vector tree at the view's
current bounds every frame, so output stays crisp through resize,
scroll, transform, and retina/non-retina scale changes. A shared
`tree_walker` translates the parsed `usvg::Tree` into per-backend
vector primitive calls (iOS + Android only); web hands markup straight
to the browser.

## Reactive markup

`markup` is reactive: pass a closure reading a `Signal`/`Source` and the
per-backend handler re-renders on change (it subscribes via
`Effect::new(...)` when it builds the native view). Use [`markup`] to
coerce a `&str`, `String`, or `Fn() -> String` into the stored closure
shape — static logo constants pass straight through.

`on_load` fires once per successful render; `on_error` fires with a
human-readable description on a parse failure (`resvg`'s parse error on
native; a stub on web, where the failure path isn't observable).

## Imperative ops

Through the bound `Ref<SvgHandle>`: `intrinsic_size()` returns the SVG's
natural pixel dimensions (from its `viewBox`, or `width`/`height`
attributes), or `None` until the first successful render — call it from
`on_load` if you need the value synchronously after mount. Bind a handle
with `.bind(my_ref.clone())` on the value `Svg(..)` returns.

[`markup`]: src/lib.rs
