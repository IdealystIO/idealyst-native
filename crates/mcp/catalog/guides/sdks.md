+++
title = "SDKs & opt-in crates"
order = 65
tags = ["sdk", "crates", "net", "storage", "credentials", "discovery", "cache", "pubsub", "jobs", "email", "server-tier"]
+++

# SDKs & opt-in crates

The framework core (`runtime-core`) ships only the lowest UI primitives and the
reactive system. Everything else — networking, persistence, camera, maps, a full
component library — lives in **separate opt-in crates** you add to your project's
`Cargo.toml` as you need them. This keeps binaries small: you only link what you
use.

These crates are **not** in the `list_components` / `list_primitives` catalog
surface, because they expose plain Rust functions and types (e.g.
`net::Client`, `storage::platform_storage()`), not `#[component]`s or `ui!`
primitives. This guide is the index for them.

## Adding an SDK

Add the crate the same way your project references `runtime-core` — by the bare
crate name (no `idealyst-` prefix), pointing at the same source your
`runtime-core` line uses (git `rev`, path, or workspace):

```toml
[dependencies]
net = { git = "https://github.com/.../idealyst-native", rev = "<same rev as runtime-core>" }
storage = { git = "...", rev = "..." }
```

Inside the workspace, examples use `net = { workspace = true }`. After adding the
dep, the SDK's functions are importable (`use net::Client;`).

Writing your own SDK rather than consuming one? See [[sdk-components]] for the
payload/handler shape, the registration seam names, and how to support lazy
loading.

## Networking & data

| Crate | What it gives you |
|---|---|
| **`net`** | Cross-platform async networking — HTTP, WebSocket, and Server-Sent Events. `net::Client` is the HTTP entry point. The transport layer the server-functions layer composes (see [[server-functions]]). |
| **`server`** | Full-stack server functions — `#[server] async fn`, `server::configure`, `server::router`, extractors, auth guards. See the dedicated [[server-functions]] guide. |
| **`storage`** | Cross-platform **insecure** key-value storage for non-sensitive app data. For signal-backed state PREFER `storage::persisted_signal(ns, key, initial)` (feature `reactive`): hydrates on creation, persists on change, and a user write before the async load resolves WINS — don't hand-roll load/persist wiring, it almost always clobbers that write. Raw store: `storage::platform_storage(ns) -> Arc<dyn Storage>`; async `get(key) -> Result<Option<String>, _>` / `set` / `remove` (see "Calling async APIs" below). Backends: localStorage / NSUserDefaults / SharedPreferences / JSON file. Once storage is a dependency, the `persisted_signal` recipe registers in `list_recipes`. Use `credentials` for secrets. |
| **`credentials`** | Cross-platform **secure** storage for secrets (auth tokens, API keys) — Keychain / Keystore on device. Web errors rather than faking security. |
| **`files`** | Cross-platform blob/file storage for **binary data** (recordings, downloads). |
| **`file-export`** | Save a file to a user-chosen location through the platform's native "save" UI (no permission prompt). |
| **`i18n`** | Localization / translation / multi-language — the internationalization SDK. Declare translations inline with the `i18n!` macro in a `mod t` (`locales: { En = "en" (default), Es = "es" } greeting(name) { En: "Hello, {name}", Es: "Hola, {name}" }`); a missing translation or bad `{placeholder}` is a COMPILE error. Each message is a fn returning `Reactive<String>` you pass to any reactive-text prop (`Typography(content = t::greeting("Ada"))`). Switch language live with `t::set_locale(t::Locale::Es)` / `i18n::set_locale_code("es")` — every visible translated string re-renders in place. Bundled locales compile in; `(lazy)` locales fetch a JSON pack (feature `lazy-fetch`). Full walkthrough in the [[i18n]] guide. |

## Server tier (cache, pubsub, jobs, email, config)

These crates run **only in the server binary** — they use tokio, redis, sqlx.
Never make them unconditional dependencies of an app crate that also compiles to
wasm: depend on them as **optional** deps enabled by the same `server` feature
your `#[server]` bodies compile under:

```toml
[dependencies]
cache  = { workspace = true, optional = true }
pubsub = { workspace = true, optional = true }

[features]
server = ["dep:cache", "dep:pubsub", "server/server"]
```

| Crate | What it gives you |
|---|---|
| **`cache`** | Centralized KV cache with TTL, shared across server instances. `Cache` trait (`get`/`set`(+TTL)/`delete` over bytes) + `CacheExt::get_json`/`set_json`; `MemoryCache` always, `RedisCache` behind feature `redis`. NOT for atomic read-modify-write — rate limits / counters go through server-kit's `LimitStore`. |
| **`pubsub`** | Cross-instance broadcast fan-out (at-most-once, ephemeral). `const ROOM: pubsub::Topic<Msg> = Topic::new("room")`; `ROOM.publish(&msg).await`; `ROOM.subscribe() -> impl Stream<Item = Msg>`. Backends: memory / redis (native Pub/Sub) / postgres (LISTEN/NOTIFY). |
| **`jobs`** | Durable background job queue — each job delivered to ONE worker, with retries/backoff/dead-letter. `#[job]` mirrors `#[server]`; run handlers via `jobs::worker().run()`, `.spawn()`, or `idealyst worker`. Backends: memory / redis / postgres / sqs. |
| **`email`** | Transactional email — fluent `Email` builder, or render an idealyst component to email-safe HTML via `.template(...)`. Providers: memory (dev/test capture), SES. |
| **`idealyst-config`** | Unified boot config for all of the above: named `[connections.<name>]` profiles in `idealyst.toml`, one `configure_all().await` call. See below. |
| **`server-kit`** | Policy layer over `server`: middleware, `Auth<T>`, CSRF, sessions (built on `cache`), rate limiting. See [[server-functions]]. |

### One connection, many consumers

The Redis connection is **app-provided context, like a database**: open one
`redis::Client` at boot, and every consumer attaches to it — no per-SDK URLs.

```rust
// boot (server main), before serving:
let client = redis::Client::open(cfg.redis_url)?;
server::install_state(client.clone());

cache::configure(cache::RedisCache::from_installed());          // KV cache
pubsub::configure(pubsub::RedisBackend::from_installed().await?); // fan-out
// server-kit sessions / RedisLimitStore read the same installed client.
```

`cache::configured()` hands the cache back anywhere server-side; to receive it
as injected `#[ctx]` state instead, `server::install_state::<Arc<dyn Cache>>(...)`.

### Configuring — `idealyst.toml` or env

The default authoring surface is `idealyst.toml` + `idealyst_config::configure_all()`:

```toml
[connections.main]
kind = "redis"
url = "redis://127.0.0.1:6379"

[cache]
backend = "redis"
connection = "main"     # same endpoint…

[pubsub]
backend = "redis"
connection = "main"     # …shared by name
```

The flat env spelling (`IDEALYST_CACHE_BACKEND`/`_URL`, `IDEALYST_PUBSUB_*`,
`IDEALYST_JOBS_*`) is the override layer; each SDK also has a
`configure_from_env()`. `idealyst dev` forwards a project's `[cache]` /
`[pubsub]` / `[jobs]` config to the spawned server + worker as those vars. For
the redis backends, env config with **no URL set falls back to the installed
`redis::Client`** — one URL configured at boot serves every consumer.

### Decentralized WebSocket notifications (pubsub × `#[subscription]`)

The headline pubsub use — no changes to the server crate needed, a
`#[subscription]` body just returns the topic's stream:

```rust
const ROOM: pubsub::Topic<Msg> = pubsub::Topic::new("room");

#[subscription]                    // client holds a socket on instance A
async fn feed() -> impl Stream<Item = Msg> { ROOM.subscribe() }

#[server]                          // any instance publishes
async fn say(text: String) -> Result<(), ServerError> {
    ROOM.publish(&Msg { text }).await.map_err(ServerError::failed)?;
    Ok(())
}
```

With a shared backend (redis/postgres), a client connected to one instance
receives events produced on another.

## Media & capture

| Crate | What it gives you |
|---|---|
| **`media-stream`** | A platform-agnostic handle to a live video source — the common abstraction camera / screen-recorder yield. |
| **`camera`** | Cross-platform camera capture → a `MediaStream`. |
| **`microphone`** | Cross-platform microphone capture → an audio stream. |
| **`screen-recorder`** | Cross-platform screen / window recording → a `MediaStream`. |
| **`media-writer`** | Record live media streams to a file (mp4). |
| **`video`** | Third-party `Video` playback primitive (a scene-registry payload). |
| **`canvas`** | The author-facing facade for the 2D-drawing SDK (GPU canvas + self-capture compositor). |

## UI primitives & extensions (scene-registry payloads)

These are third-party UI primitives. The framework has one dispatch model for
every primitive: a **payload handler on the scene `Registry`**. First-party
primitives (`view`, `text`, …) register through
`runtime_vocabulary::register_builtins`; a third-party SDK registers exactly
the same way. Adding the crate and calling the primitive in `ui!` is **not
sufficient**: an unregistered payload **panics at realize**. See "Registering
extension SDKs" just below.

| Crate | What it gives you |
|---|---|
| **`idea-ui`** | The cross-platform **component library** — `Button`, `Card`, `Field`, `Select`, etc. Most apps depend on this. Its components ARE catalogued (`list_components`) once linked. |
| **`idea-theme`** | Theming abstraction + extensibility for the idealyst design system. |
| **`icons-lucide`** | Lucide icon pack — only icons you reference end up in the binary. |
| **`webview`** | Third-party `WebView` primitive. The canonical single-crate cfg-gated External pattern. |
| **`maps`** | Third-party `MapView` primitive. |
| **`svg`** | Third-party SVG renderer. |
| **`markdown`** | CommonMark/GFM document primitive. |
| **`codeblock`** | Read-only colored-text (code) panel primitive. |
| **`table`** | Cross-platform table — a real `<table>` on web. |
| **`form`** | Third-party `Form` SDK. |
| **`toolbar`** | Third-party `Toolbar` SDK. |
| **`menu`** | OS-level menu-bar SDK (desktop). |

### Registering extension SDKs

Every extension SDK (`webview`, `maps`, `svg`, `markdown`, `codeblock`,
`table`, `toolbar`, `video`) exposes a `register(&mut registry)` that installs
its payload handler. Registration is **mandatory on every target**: the scene
registry has no fallback handler, so a payload with no handler is a panic at
realize, not a placeholder. (This is deliberate — a missed `register` fails
loud instead of rendering a silent grey box.)

Call it from your crate's `register_scene_extensions` — the seam the
CLI-generated wrapper invokes after `runtime_vocabulary::register_builtins`:

```rust
pub fn register_scene_extensions<H: runtime_scene::Host>(
    registry: &mut runtime_scene::Registry<H>,
) {
    table::register(registry);      // real <table> on web, CSS-grid on native
    markdown::register(registry);   // CommonMark handler
    // …one line per extension SDK you render.
}
```

Every SDK spells that seam `register` — as of 1.1.0 there are no
`register_handlers` / `register_scene` / `register_generic` variants left, and
no no-op `register(&mut backend)` shims (see [[migration-1-0-0-to-1-1-0]]).

To ship an SDK's handler in a lazy chunk rather than the main bundle, swap the
verb: `table::defer(registry)` at boot, plus
`table::register_from_chunk::<MyBackend>()` from inside a `#[component(lazy)]`
body. Both halves are required — see [[lazy-loading]]. Off-web `defer` simply
registers eagerly, since only web code-splits, so it is always safe to call.

Most SDK `register` fns are **caps-generic** (`register<H>(&mut Registry<H>)`)
and serve every backend from one handler. An SDK with a real platform leg
(`svg`'s iOS/Android vector walk, `video`'s AVPlayer, `toolbar`'s `NSToolbar`)
selects it by **registry type**, not by `cfg`: its `register` downcasts the
registry to the concrete backend's `Registry<MacosBackend>` / `<IosBackend>` /
`<AndroidBackend>` and falls through to the portable handler otherwise. A
`cfg(target_os = "macos")` split alone cannot express that — the cfg is
equally true for an SSG render running on a macOS host, which needs the
portable handler. If your `register_scene_extensions` needs to name a concrete
backend, specialize `H` to that backend's registry type in your app crate and
add the backend dep.

SSR needs the same handlers so first paint matches the client (see the
website's `examples/serve.rs`, which registers alongside the web build).

### Code-splitting a heavy extension (web)

If an extension SDK is large but used in only one corner of the app, wrapping
the *usage* in `#[component(lazy)]` splits that corner's **rendering** code
into a chunk. What it does **not** split on its own is the SDK's handler: a
handler named in `register_scene_extensions` at boot — and everything it
statically reaches — is reachable from `main.wasm`.

Registration is the anchor, and the registry has a **late-registration seam**
for moving it out of main: declare the payload kind at boot with
`registry.defer::<T>()` (which lets `realize` park an item of that kind instead
of panicking), then install the real handler from inside the chunk with
`runtime_scene::defer_registration` → `Registry::register_deferred`. This is the
runtime-v2 successor to the pre-v2 core's `defer_external_registration`. Full
recipe and the size measurement in [[lazy-loading]].

Practical consequences:

- Split the **body**, and defer the **handler** when the handler is the heavy
  part. If you do register at boot instead, keep the handler thin: the heavy
  work (a rasterizer, a font stack, an embedded payload) should live behind a
  function the *chunk* calls, not behind one the handler calls at
  registration time.
- Static **data** never leaves `main.wasm` by default regardless of chunking;
  dropping it needs the experimental opt-in `idealyst build --web --release
  --data-prune` (see [[lazy-loading]]).
- Register only the SDKs you actually render. An unused `register` line costs
  its handler's whole reachable graph in the main bundle — and now that
  registration is explicit everywhere, that list is entirely under your
  control.

### There are no `prim-*` features

Earlier releases let you trim whole primitive families out of the build with
twelve `prim-*` cargo features on `runtime-core` (mirrored by each backend
crate), six matching ones on `idea-ui` that `#[cfg]`-deleted the components
rendering each family, and `idealyst build --web --primitives=…` to select the
set. **All three layers are gone**, along with the SDK-side forwards that fed
them (`virtualized` → `prim-virtualizer`, the navigators → `prim-navigator`).
They gated three things in the pre-v2 core — walker dispatch arms, authoring
builder fns, and `Backend` trait methods — and none of them exist now.

Nothing in your source changes: no type, function, macro, or component was
lost, and the component set is unconditional. Manifests and build commands do:
drop any `features = ["prim-…"]` entry (cargo rejects the manifest otherwise)
and drop `--primitives` (the CLI rejects it with a pointer to the migration
guide). If you want a smaller bundle, the lever is now **what you register** —
see "Register only the SDKs you actually render" above.

The structural successor for primitives themselves is per-primitive **handler
registration**: `runtime_vocabulary::handlers::register_builtins` holds the
only reference to each primitive's module and its caps calls, so a per-family
gate belongs there (and in each backend's caps impls). Until that lands,
keeping the old flags would have shipped a lever that deletes components from
the public API while saving nothing in the bundle. See
`docs/migrating-to-runtime-v2.md` for the full before/after, and
[[migration-0-4-0-to-0-5-0]] for the historical contract.

## Device & platform integration

The OS-integration capabilities. `permissions` is the shared runtime-grant
substrate: any capability that prompts the user (`notifications`, `location`,
and the media SDKs `camera` / `microphone`) goes through it rather than
re-implementing an OS prompt. Each capability SDK declares its own build-time
permission requirement (`[package.metadata.idealyst] capabilities = [...]`); the
app supplies the reason string.

| Crate | What it gives you |
|---|---|
| **`permissions`** | Cross-platform **runtime permission** requests — the shared grant substrate. `permissions::request(Permission)` / `status(Permission)` → a uniform `PermissionStatus`. Other SDKs depend on this instead of re-implementing a grant flow. |
| **`biometrics`** | Cross-platform biometric authentication ("prove the device owner is present"). |
| **`notifications`** | Local + scheduled notifications and the raw device push token. Authorization goes through `permissions`; server-side push delivery is the app's job. |
| **`location`** | Device geolocation — one-shot `current()` + continuous `watch()` yielding a `Position`. Permission grant goes through `permissions`. |
| **`clipboard`** | System copy/paste of plain text — `clipboard::set_text` / `text`. |
| **`share`** | The system share sheet (outbound) — hand text/url/files to another app. The inverse of `file-picker`. |
| **`deep-link`** | Inbound URL handling — `initial_link()` + `on_link()` deliver the parsed launch/resume URL (custom scheme / universal / app link). |
| **`connectivity`** | Network reachability — `current()` snapshot + `watch()` of online/offline and coarse transport (wifi/cellular/ethernet). |
| **`haptics`** | Tactile feedback — `impact` / `notify` / `selection`. Fire-and-forget, best-effort. |
| **`audio`** | Sound playback — `load(AudioSource)` → a `Sound` you `play()`, with a controllable `Playback`. The playback peer of the capture SDKs. |

## How they relate to the catalog

- An SDK that ships `#[component]`s (like `idea-ui`) surfaces those components
  through `list_components` / `describe_component` **once it's a dependency of
  the build the catalog is extracted from**.
- An SDK that exposes free functions / scene-registry primitives (like
  `net`, `storage`, `webview`) is documented here and in its own crate docs —
  read the crate's `lib.rs` module docs for the full API.

When you're unsure which crate provides a capability, this list is the map:
networking → `net`, persistence → `storage` / `credentials` / `files`,
server relay → `server`, camera/mic/recording → the media crates.

## Calling async APIs from UI code

Several SDK surfaces (`storage`, `net`, …) are `async`. UI handlers and
component bodies are synchronous — bridge with `runtime_core::spawn_then`:
IO in the future, every signal read and write in the callback.

```rust
use runtime_core::spawn_then;

let items = signal(Vec::new());
spawn_then(
    async move {
        let store = storage::platform_storage("my-app");
        store.get("items").await
    },
    move |saved| {
        if let Ok(Some(saved)) = saved {
            items.set(parse(saved));
        }
    },
);
```

**Do not write signals inside the future.** Every `.await` is a flush
boundary, so the component can be torn down between two adjacent lines of
one async block; a signal write in the resumed continuation lands on a
freed slot and aborts with `idealyst[stale-signal-handle]`. `spawn_then`'s
callback runs inside a turn or not at all, so the update is atomic and
reads are safe too. The in-flight IO still completes — only its result is
discarded — so a save is never abandoned mid-write. The
`signal-across-await` lint flags the raw form.

`runtime_core::driver::spawn_async` remains for genuinely detached work
that must outlive the component (a background upload, a storage
write-through). No `Send` bound is required, and the executor is
pre-installed by the CLI-generated app wrappers on every platform — no
setup in app code. Signal writes from the callback notify the UI exactly
like writes from a handler.

`spawn_then` and `spawn_async` exist only when the `runtime-core` dependency enables the
`async-driver` feature — CLI-generated wrapper Cargo.tomls do, but a
hand-written dep line must add it:

```toml
runtime-core = { path = "…", features = ["async-driver"] }
```
