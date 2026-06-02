# `maps`

A `MapView` primitive for the idealyst framework — drop a native map
centered on a coordinate into your UI tree. Built on the framework's
`Element::External` extension mechanism, so it's not part of
runtime-core: an app opts in by depending on this crate and calling
`maps::register(&mut backend)` once at bootstrap.

This is the **canonical multi-crate split** of the third-party
primitive pattern (contrast `webview`, which is one `cfg`-gated crate):

- **`maps-core`** — the shared `MapViewProps` payload type. Pure data,
  no backend code, no FFI. Both the umbrella and the leaves depend on
  it without forming a cycle.
- **`maps`** (this crate) — the author-facing facade: the `MapView`
  constructor plus a `cfg`-routed `register` that re-exports exactly one
  leaf per target.
- **`maps-web` / `maps-ios`** (future `maps-android`) — per-backend leaf
  crates that do the actual FFI / native-SDK wiring against a concrete
  backend type.

Use this split when backends have independent maintainers or genuinely
heavy disjoint transitive deps. Otherwise prefer the single-crate
`webview` shape.

```rust,ignore
use maps::{MapView, MapViewProps};

// App bootstrap — one line per third-party SDK. Cargo routes
// `register` to the right leaf for the build target:
let mut backend = WebBackend::new("#app");
maps::register(&mut backend);

// Inside a `ui!` block. `MapView` is an external primitive, so it's
// interpolated as an expression:
ui! {
    View {
        { MapView(MapViewProps { lat: 37.7749, lon: -122.4194, zoom: 12.0 }) }
    }
}
```

## What you get

`MapViewProps` is plain `Copy` data — `lat` / `lon` in decimal degrees
and a `zoom` on the OpenStreetMap tile convention (0 = world, 18 ≈
street). Unlike `webview`/`video`/`svg`, there are no reactive closures
or callbacks; a re-render with changed coordinates rebuilds the native
view rather than mutating it in place (acceptable for the current POC
leaves). The *mechanism* differs per platform:

| Target | Mechanism |
| --- | --- |
| Web (wasm32) | OpenStreetMap embed `<iframe>` (POC; a production leaf would bind Leaflet/MapLibre via wasm-bindgen) |
| iOS | native `MKMapView` via raw `msg_send`; `zoom` → camera altitude |
| Other (Android, wgpu desktop, terminal, …) | umbrella `register` is a no-op; the framework's `External` "not supported" placeholder renders at mount |

## Adding a backend leaf

`cargo new` a `maps-<backend>` crate that depends on `maps-core` and the
concrete backend crate, then exposes
`pub fn register(backend: &mut <Backend>)` calling
`backend.register_external::<MapViewProps, _>(...)`. Add a
`[target.'cfg(...)'.dependencies]` line in this crate's `Cargo.toml`
and a matching `#[cfg(...)] pub use maps_<backend>::register;` in
`src/lib.rs`. App code keeps calling `maps::register(&mut backend)`
unchanged.

## Platform notes

The iOS leaf reaches `MKMapView` at the Obj-C runtime layer via
`AnyClass::get` + raw `msg_send` rather than `objc2-map-kit` — same
objc2-major-conflict rationale as the webview SDK. The host project must
link `MapKit.framework` so the class is registered at startup.
