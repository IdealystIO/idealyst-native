# `examples/`

Curated demo apps. Each is an ordinary app crate: it depends on
`runtime-core`, builds its tree with `ui!`, and exposes
`app()`, `register_scene_extensions(&mut runtime_scene::Registry<H>)`, and
`scene_app()` for the Android wrapper. Build any of them with
`idealyst build --<target> examples/<name>` or run with
`idealyst dev --<target> examples/<name>`.

## Every app owns its entry point

An app crate carries a `src/main.rs` holding one line:

```rust
idealyst::entry!(welcome);
```

plus `idealyst = { workspace = true }` in `[dependencies]`. That macro
reads `[package.metadata.idealyst.app]`, lifts the app's
`register_scene_extensions` into the `SceneExtensions` impl the boot
seam takes, and emits a `main` calling `idealyst::boot::run`. Which
shell that resolves to is **config, not code** — the target triple
settles web/iOS/Android, and a feature on the `idealyst` dep picks
between native shells that share a triple.

There is no CLI-generated *web* wrapper any more: `idealyst build --web`
builds this crate's own binary, and a project without `src/main.rs`
fails with a message naming the fix. iOS and Android still get a
generated wrapper that depends on the app as an `rlib`.

**`register_scene_extensions` must stay registry-GENERIC** —
`fn register_scene_extensions<H>(&mut Registry<H>) where H: …`. The boot
seam is generic over the backend (`SceneExtensions::register<H:
SceneHost>`), so a `cfg`-split seam whose web half takes a concrete
`Registry<WebBackend>` cannot cross it. SDK `register` fns type-dispatch
on the registry internally, so one generic seam serves every target;
`whiteboard-demo` is the worked example.

| Example | What it demonstrates | Targets |
| --- | --- | --- |
| `welcome` | The animation system (springs + tweens + a raf-driven pulse). **Load-bearing**: `idealyst new` copies this project verbatim, so its Cargo.toml + `src/lib.rs` shape is the scaffold's shape (`crates/tools/cli/src/cmd/scaffold_template.rs` mirrors it). | web, ios, android |
| `nav-showcase` | Nested navigators (swap + stack) with `idea-ui-nav` chrome. | web, ios, android |
| `login-demo` | Full-stack auth: `#[server]` login, httpOnly session cookie (BFF), bearer token in the OS keystore on native. | web, ios, android |
| `whiteboard-demo` | Drawable canvas + camera + recording. See its module docs for the platform coverage: the canvas handler is real on web and the missing-primitive placeholder on native. | web, macos, ios, android |
| `inspector` | The Inspector's front end: a live debugging dashboard. Talks only to the Inspector server (`idealyst inspect`, which embeds and serves its web build); see [docs/inspector.md](../docs/inspector.md). | web, macos |
| `baseline` | One text node. The framework's web bundle-size floor. | web |
| `keyboard-avoid` | `keyboard_avoiding_view` at the root: a bottom composer rides the keyboard in step with the platform's own keyboard animation, plus a `keyboard_inset()` readout. See `docs/keyboard.md`. | web, ios, android |
| `sdk-gallery` | One panel per client SDK (SVG, clipboard, connectivity, notifications, web view, video, audio, location, share sheet, camera, screen recording) for checking each on a platform by hand. | web, macos, windows |
| `user-activity` | A kiosk-style idle clock driven by `user_activity()`: every interaction anywhere in the app restarts it, with no handlers of the app's own. See `docs/user-activity.md`. | web, ios, android, macos |
| `fiddle` | The online playground's compile server (not an idealyst app itself; `template/` is the project it compiles user snippets into). | — |
