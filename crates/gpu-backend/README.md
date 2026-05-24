# `gpu-backend/` — the composed wgpu Backend

Not every platform has a native UI toolkit to translate into. The
**GPU Backend** draws everything itself: pixels straight to a wgpu
surface, with its own input dispatch, layout, text shaping, and
accessibility bridge. With no native toolkit underneath, the GPU
Backend has to *provide* the substrate internally — so it ships as
three plug-and-play layers behind one `runtime_core::Backend`
impl.

```
                ┌────────────────────────────────┐
                │      runtime_core::Backend   │
                │      (the seam)                │
                └───────────────┬────────────────┘
                                │
                ┌───────────────┴────────────────┐
                │            engine/             │
                │  wgpu surface + pipeline +     │
                │  text shaping + input dispatch │
                │  (delegates "what to draw" to  │
                │   the Painter)                 │
                └───────────────┬────────────────┘
                                │
                ┌───────────────┴────────────────┐
                │            painter/            │
                │  Per-primitive geometry —      │
                │  "what does a Button look      │
                │  like?". Swappable per look.   │
                └────────────────────────────────┘

                ┌────────────────────────────────┐
                │            host/               │
                │  Window, drawing surface, OS   │
                │  events. One per platform.     │
                └────────────────────────────────┘

                ┌────────────────────────────────┐
                │            variant/            │
                │  Form-factor bundles —         │
                │  phone, tablet, tv. Picks the  │
                │  Host + Painter + viewport.    │
                └────────────────────────────────┘
```

| Layer | Path | Role |
| --- | --- | --- |
| Contract | [`api/`](./api) | `EventSink`, `DeviceProfile`, input vocabulary — the small contract crate that lets any Host pair with any Engine without either knowing the other's internals. |
| Engine | [`engine/`](./engine) | The wgpu renderer itself. Implements `runtime_core::Backend` + `render_api::EventSink`. Owns the surface, pipeline, frame management, text shaping. Knows nothing about specific primitives — delegates to the Painter. |
| Host | [`host/`](./host) | Platform integration. Owns window + drawing surface + event source; translates platform events into the api vocabulary. One crate per platform: `winit`, `appkit`, `web`, `terminal`, `wgpu-accesskit` (the accessibility bridge). |
| Painter | [`painter/`](./painter) | Per-primitive look. The Engine receives `create_button` from the Backend trait and asks the Painter what a button looks like. Sub-crates: `ios-sim`, `android-sim`. Each defines the platform's chrome. |
| Variant | [`variant/`](./variant) | Form-factor bundles. `phone`, `tablet`, `tv` — each picks a (host, painter, profile) trio and exposes one `run(...)` entry point. |

## Why three layers instead of one crate

The wgpu Backend is the most complex Backend by far — it
reimplements everything a native toolkit gives you for free. Slicing
it into Engine + Painter + Host means each piece is small enough to
swap independently. Swap the Painter to change the look. Swap the
Host to change the platform. The Engine and api stay the same.

`host-uikit` and `host-android-native` are planned so GPU-rendered
apps can target iOS and Android directly — that's the future of
this slot.

## Renames in flight

The Painter layer was previously called *Skin* (`crates/skin/`).
The trait inside `engine/` is now `Painter`; the crate-level
identifier may still mention the old name in places — those are
follow-up cleanups, not blockers.
