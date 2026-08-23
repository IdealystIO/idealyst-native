//! Hand-curated registration table for [`SdkEntry`] — the opt-in crates
//! under `crates/sdk/{client,server}/*`, `crates/api/*`, and `crates/ui/*` that ship
//! outside `runtime-core`.
//!
//! Same lock pattern as `primitives.rs` / `macros.rs`: `SdkEntry` carries
//! a private `_seal: ()` so only this crate constructs one. Every entry
//! names a crate that actually exists in the workspace; the prose home
//! for the roster is the `sdks` guide (`guides/sdks.md`), and the
//! `#[server]` flow has its own [[server-functions]] guide. The drift
//! audit (`.claude/audits/mcp-catalog-drift.md`) checks this table
//! against the `crates/{sdk,api,ui}/*` directory listing, so adding or
//! renaming a crate means updating this file in the same change.
//!
//! `dep_line` is a copy-pasteable `Cargo.toml` line. We use the
//! `{ workspace = true }` form (correct inside the workspace and for
//! examples); an external project mirrors its `runtime-core` git/rev/path
//! source — the `sdks` guide spells that out.

use crate::{SdkCategory, SdkEntry, SdkKind};

macro_rules! sdk {
    ($name:literal, $cat:expr, $kind:expr, $summary:literal) => {
        sdk!($name, $cat, $kind, $summary, guide = "sdks");
    };
    // Entries whose prose home is a dedicated guide (e.g. the navigator
    // crates → `navigation`) override the default `sdks` anchor so
    // `describe_sdk` points an agent at the guide that actually documents
    // the crate's API shape.
    ($name:literal, $cat:expr, $kind:expr, $summary:literal, guide = $guide:literal) => {
        sdk!($name, $cat, $kind, $summary, guide = $guide,
             dep = concat!($name, " = { workspace = true }"));
    };
    // Entries needing a non-default dependency line — the server-tier crates
    // are OPTIONAL deps gated behind the app's `server` feature, and the
    // default `{ workspace = true }` line would drag tokio into the wasm
    // client build.
    ($name:literal, $cat:expr, $kind:expr, $summary:literal, guide = $guide:literal, dep = $dep:expr) => {
        inventory::submit! {
            SdkEntry {
                name: $name,
                summary: $summary,
                dep_line: $dep,
                category: $cat,
                kind: $kind,
                guide: $guide,
                _seal: (),
            }
        }
    };
}

/// The dependency line for a server-tier crate: optional, enabled from the
/// app's `server` feature so the wasm/native client never compiles it.
macro_rules! server_dep {
    ($name:literal) => {
        concat!(
            $name,
            " = { workspace = true, optional = true }  # + `server` feature: server = [\"dep:",
            $name,
            "\", ...]"
        )
    };
}

// ---------------------------------------------------------------------
// Data — networking, persistence, server relay, i18n
// ---------------------------------------------------------------------

sdk!(
    "net",
    SdkCategory::Data,
    SdkKind::Api,
    "Cross-platform async networking — HTTP, WebSocket, and Server-Sent Events. `net::Client` is the HTTP entry point; the transport the server-functions layer composes."
);
sdk!(
    "server",
    SdkCategory::Data,
    SdkKind::Api,
    "Full-stack server functions: `#[server] async fn`, `server::configure`, `server::router`, request extractors, auth guards. See the [[server-functions]] guide."
);
sdk!(
    "graphql",
    SdkCategory::Data,
    SdkKind::Api,
    "Typed GraphQL client + server-side execution. Client: `#[derive(GraphQLQuery)]` codegen over `.graphql` operation files (`graphql_client` is re-exported, so no second dependency), driven through the `GraphqlClient` + `Transport` seam — `GraphqlClient::http(url)` for any endpoint, or `graphql::graphql_transport!(AppGraphql, my_endpoint)` to ride an app-authored `#[server] async fn(GraphqlRequest) -> Result<GraphqlResponse, _>` and inherit its auth / CSRF / per-platform base URL. The crate deliberately has NO dependency on the `server` SDK (same posture as `sync`). Reactive: `use_query` / `use_mutation` wrap runtime-core's `resource` / `mutation`, so a component re-renders as results arrive. SERVER EXECUTION IS FEATURE-GATED: `execute_request` / `Schema` / `sdl` exist ONLY under the `server` cargo feature (off by default so client/wasm builds never pull async-graphql) — enable it on the server build only. SCOPE (v1): queries + mutations end to end; SUBSCRIPTIONS ARE NOT IMPLEMENTED — they are a documented extension seam (`execute_stream` + a WebSocket transport), not a shipped API."
);
sdk!(
    "storage",
    SdkCategory::Data,
    SdkKind::Api,
    "Cross-platform INSECURE key-value storage for non-sensitive app data. PREFERRED for signal-backed state: `storage::persisted_signal(namespace, key, initial) -> Signal<T>` (crate feature `reactive`) — hydrates on creation, persists on change, race-correct (a user write before the async load resolves WINS; hand-rolled load/persist wiring almost always clobbers it). Raw API: `storage::platform_storage(namespace) -> Arc<dyn Storage>` with ASYNC `get(key) -> Result<Option<String>, StorageError>` / `set` / `remove` — call via `runtime_core::spawn_then(future, |result| { … })`, which keeps every signal write in a callback that runs inside a turn or not at all (needs runtime-core's `async-driver` feature; generated wrappers enable it). Do NOT write signals inside the future: every `.await` is a flush boundary, so an unmount mid-flight leaves the continuation writing a freed slot and the app aborts with `idealyst[stale-signal-handle]`. Backends: localStorage (web), NSUserDefaults (iOS/macOS), SharedPreferences (Android), JSON file (desktop). The `persisted_signal` recipe registers once storage is a dependency of the build. No security claims — use `credentials` for secrets."
);
sdk!(
    "sync",
    SdkCategory::Data,
    SdkKind::Api,
    "Offline-first cache + server synchronization: download a named PARTITION of server data (`\"project:123\"`), read and mutate it offline against a reactive `Signal`, and replay a durable outbox when connectivity returns. YOU WRITE BOTH HALVES OF THE WIRE — the SDK owns persistence (on the `storage` SDK), the outbox, the sync engine, and the `pull`/`push` protocol types; the APP owns its entity type, the server bodies of `pull`/`push`, and the per-entity `Merge` policy, bridged with `sync::sync_transport!(MyTransport, Entity, pull = pull_fn, push = push_fn)`. Like `graphql`, it has NO dependency on the `server` SDK — the protocol types are the only contract. CORRECTNESS SCOPE (v1): no silent data loss, crash-safe across restarts, conflicts surfaced to the app — under a SINGLE-WRITER-PER-DEVICE assumption. It is NOT automatic multi-device convergence, real-time push, or CRDTs; those layer on top. Crash safety rests on `storage`'s single-`set` atomicity, so nothing spans two keys. Web multi-tab is coordinated with Web Locks + BroadcastChannel (`SharedPartition`). The in-memory reference `Authority<T>` server is behind the off-by-default `reference-server` feature — do not expect it in a client build."
);
sdk!(
    "credentials",
    SdkCategory::Data,
    SdkKind::Api,
    "Cross-platform SECURE storage for secrets (auth tokens, API keys) — Keychain / Keystore on device. Web errors rather than faking security."
);
sdk!(
    "files",
    SdkCategory::Data,
    SdkKind::Api,
    "Cross-platform blob/file storage for binary data — recordings, downloads."
);
sdk!(
    "file-export",
    SdkCategory::Data,
    SdkKind::Api,
    "Save a file to a user-chosen location through the platform's native save UI (no permission prompt)."
);
sdk!(
    "file-picker",
    SdkCategory::Data,
    SdkKind::Api,
    "Inverse of file-export: let the user pick local file(s) via the native picker. Yields a lazily-streamed `PickedFile` (path / open-chunk / copy_to) — never reads the whole file into RAM. Documents vs Media (dedicated mobile photo picker)."
);
sdk!(
    "i18n",
    SdkCategory::Data,
    SdkKind::Api,
    "Localization / translation / multi-language — the internationalization SDK (locale, language switcher, i18n). Declare translations inline with the `i18n!` macro inside a `mod t`: `i18n::i18n! { locales: { En = \"en\" (default), Es = \"es\" } greeting(name) { En: \"Hello, {name}\", Es: \"Hola, {name}\" } }` — a missing translation or bad `{placeholder}` is a COMPILE error. Each message becomes a fn returning `Reactive<String>`, so pass it straight to any reactive-text prop: `Typography(content = t::greeting(\"Ada\"))` / `text(t::tagline())`. Switch language live with the generated typed `t::set_locale(t::Locale::Es)` (or untyped `i18n::set_locale_code(\"es\")`) — every visible translated string re-renders in place, no manual refresh, every backend. Bundled locales compile into the binary; `(lazy)` locales fetch a JSON pack on demand (feature `lazy-fetch`). See the [[i18n]] guide.",
    guide = "i18n"
);

// ---------------------------------------------------------------------
// Server tier — crates that run ONLY in the server binary. Apps depend
// on them as OPTIONAL deps enabled by their `server` feature (the same
// feature `#[server]` compiles under), so the wasm client never pulls
// tokio/redis/sqlx. The `sdks` guide's "Server tier" section documents
// the shared-connection + configuration story.
// ---------------------------------------------------------------------

sdk!(
    "cache",
    SdkCategory::Server,
    SdkKind::Api,
    "Server-tier KV cache with TTL — centralized memory storage shared across server instances. Object-safe `Cache` trait (`get`/`set`(+TTL)/`delete`, bytes) + blanket `CacheExt::get_json`/`set_json`; backends: `MemoryCache` (always), `RedisCache` (feature `redis`). Configure at boot via `cache::configure(...)` / `configure_from_env()` (`IDEALYST_CACHE_BACKEND`=memory|redis, `IDEALYST_CACHE_URL`) or the `[cache]` section in `idealyst.toml` (idealyst-config), then read anywhere server-side via `cache::configured()` — or install as `#[ctx]` state (`server::install_state(Arc<dyn Cache>)`). `RedisCache::from_installed()` reuses the app-installed `redis::Client`, so ONE client serves cache + sessions + rate-limit + pubsub. Deliberately excludes atomic read-modify-write — rate limits/counters belong in server-kit's `LimitStore`, not get/set races.",
    guide = "sdks",
    dep = server_dep!("cache")
);
sdk!(
    "pubsub",
    SdkCategory::Server,
    SdkKind::Api,
    "Server-tier publish/subscribe — cross-instance broadcast fan-out (at-most-once, ephemeral). Typed topics: `const ROOM: pubsub::Topic<Msg> = Topic::new(\"room\")`, then `ROOM.publish(&msg).await` from any instance and `ROOM.subscribe() -> impl Stream<Item = Msg>` dropped straight into a `#[subscription]` body — the decentralized WebSocket-notification bridge (client on instance A receives events produced on B). Backends: memory (`tokio::broadcast`, default), redis (native Pub/Sub), postgres (LISTEN/NOTIFY). Configure via `pubsub::configure`/`configure_from_env` (`IDEALYST_PUBSUB_*`) or `[pubsub]` in `idealyst.toml`; `RedisBackend::from_installed()` reuses the app-installed `redis::Client`. Sibling of `jobs`: jobs delivers each message to ONE consumer, pubsub to EVERY subscriber.",
    guide = "sdks",
    dep = server_dep!("pubsub")
);
sdk!(
    "jobs",
    SdkCategory::Server,
    SdkKind::Api,
    "Server-tier background job queue — durable one-consumer work delivery with retries, backoff, and dead-lettering. `#[job]` mirrors `#[server]`: it generates `name::enqueue(args)` (builder: `.delay`/`.queue`/`.max_attempts`/`.backoff`, awaitable) everywhere, and the handler body only under your `server` feature. Handlers run in a worker: `jobs::worker().run()` (dedicated bin / `idealyst worker`) or `.spawn()` (in-process). Backends: memory (default), redis, postgres, sqs. Configure via `jobs::configure`/`configure_from_env` (`IDEALYST_JOBS_*`) or `[jobs]` in `idealyst.toml`. Sibling of `pubsub`: jobs = one consumer per message, pubsub = fan-out.",
    guide = "sdks",
    dep = server_dep!("jobs")
);
sdk!(
    "email",
    SdkCategory::Server,
    SdkKind::Api,
    "Server-tier transactional email. Fluent builder — `Email::to(..).subject(..).text(..)` — or render an idealyst `#[component]` to email-safe HTML with `.template(|| Welcome(props))` (styles inlined, tokens resolved, no wasm). `email::configure(provider)` at boot, `email::send(...).await` anywhere server-side. Providers: `MemoryProvider` (in-process capture for dev/tests, default), `SesProvider` (feature `ses`). `[email]` section in `idealyst.toml` wires it via idealyst-config.",
    guide = "sdks",
    dep = server_dep!("email")
);
sdk!(
    "idealyst-config",
    SdkCategory::Server,
    SdkKind::Api,
    "Unified boot configuration for the server-tier SDKs (jobs, pubsub, email, cache). Named connection profiles — `[connections.<name>]` (kind = aws|redis|postgres) defined ONCE in `idealyst.toml`, referenced per tool (`connection = \"main\"`) so two SDKs share one AWS account or redis endpoint by name (flat env can't express that). Files compose: base + per-tool `jobs.toml`/`pubsub.toml`/`email.toml`/`cache.toml` + `extends`, env vars as the override layer. ONE call at startup — `idealyst_config::configure_all().await?` — wires every SDK whose cargo feature is enabled (`jobs`/`pubsub`/`email`/`cache`, backends via `redis`/`postgres`/`sqs`/`ses`).",
    guide = "sdks",
    dep = server_dep!("idealyst-config")
);
sdk!(
    "server-kit",
    SdkCategory::Server,
    SdkKind::Api,
    "The conventional policy layer over the `server` crate (which itself holds no policy): ordered middleware chain (`install_middleware`/`from_fn`), `Auth<T>` principal extractor (real 401 on missing), `csrf_guard` origin allow-list, path-prefix `require` guards, `Sessions` (ticket store built ON the `cache` crate's `Cache` trait), and rate limiting (`rate_limit` + `LimitStore`: `MemoryLimitStore`/`RedisLimitStore` — the atomic counters a get/set cache can't express). Occupies `server`'s single `DispatchHook` seam. See the [[server-functions]] guide.",
    guide = "server-functions",
    dep = server_dep!("server-kit")
);
sdk!(
    "server-aws",
    SdkCategory::Server,
    SdkKind::Api,
    "Run every linked `#[server]` fn as ONE AWS Lambda. `#[tokio::main] async fn main() -> Result<(), server_aws::Error> { server::install_state(...); server_aws::run().await }` — or `server_aws::run_router(app)` to compose extra axum routes (health check, static fallback) onto `server::router()` first. It is a thin `lambda_http::run(server::router())` wrapper over the SAME dispatch core, so extractors, `#[ctx]` state, middleware, cookies, the `_batch` route, and schema-drift headers all work unchanged, and the client's existing transport posts to `<base>/_srv/<fn>` against a Function URL or API Gateway route. The router is built once at cold start and reused across warm invocations, so install pools/state/middleware BEFORE calling `run()`. STREAMING DOES NOT PORT: `#[channel]` / `#[subscription]` (WebSockets) and `#[sse]` do NOT work over plain Lambda request/response — they need an API Gateway WebSocket API and a response-streaming Function URL (`InvokeMode: RESPONSE_STREAM`) respectively, which this adapter does not provide. Always implies `server/server`; there is no client build of it, which is why it is an optional server-feature dep.",
    guide = "sdks",
    dep = server_dep!("server-aws")
);

// ---------------------------------------------------------------------
// Media — capture, playback, drawing
// ---------------------------------------------------------------------

sdk!(
    "media-stream",
    SdkCategory::Media,
    SdkKind::Api,
    "A platform-agnostic handle to a live video source — the common abstraction camera / screen-recorder yield."
);
sdk!(
    "camera",
    SdkCategory::Media,
    SdkKind::Api,
    "Cross-platform camera capture → a `MediaStream`."
);
sdk!(
    "microphone",
    SdkCategory::Media,
    SdkKind::Api,
    "Cross-platform microphone capture → an audio stream."
);
sdk!(
    "denoise",
    SdkCategory::Media,
    SdkKind::Api,
    "Neural noise suppression as an `AudioStream` -> `AudioStream` transformer (DeepFilterNet 3): `Denoiser::new().process(&noisy).await?` drops between any producer and consumer (`microphone` -> `denoise` -> `media-writer`, camera audio, a decoded file). NO EMBEDDED MODEL ON WEB: `Denoiser::new()` does not even EXIST on wasm32 — it is `#[cfg]`-ed out so the multi-MB weights never enter the bundle. Fetch `DeepFilterNet3_onnx.tar.gz` yourself and call `Denoiser::with_weights(bytes)`, whose slice must be `'static` (park it in a `OnceLock<Vec<u8>>` or `Box::leak` a one-time buffer). OUTPUT IS ALWAYS 48 kHz MONO on its OWN monotonic clock, not the input's timeline — fine for standalone denoised capture, but note it when muxing against the original audio/video for lip-sync. Latency is ~30 ms (one ~10 ms hop + DeepFilterNet's ~20 ms lookahead) — fine for recording and one-way streaming, marginal for full-duplex monitoring. It is noise SUPPRESSION, not acoustic echo cancellation (no far-end reference signal). Pure-Rust inference (`tract`), so one implementation covers macOS / iOS / Android / desktop / web; only the execution context differs — a worker thread on native, inline on the main thread on wasm (no threads there)."
);
sdk!(
    "screen-recorder",
    SdkCategory::Media,
    SdkKind::Api,
    "Cross-platform screen / window recording → a `MediaStream`."
);
sdk!(
    "media-writer",
    SdkCategory::Media,
    SdkKind::Api,
    "Record live media streams to a file (mp4)."
);
sdk!(
    "video-compose",
    SdkCategory::Media,
    SdkKind::Api,
    "Real-time video compositing that emits a NEW output `MediaStream` — \"the product.\" The input stream is never modified; the ops live only on the emitted stream, which any consumer (a preview `video`, the `media-writer` recorder, WebRTC) treats like any other. `VideoPipeline::new(input).crop(|| rect).watermark(img, Corner::BottomRight, 16.0, || opacity).overlay_stream(pip, || rect).draw(|scene| ...).build()`. Every parameter is a `Fn` closure re-read EACH composited frame, so a moving watermark / live crop / dragged picture-in-picture updates without rebuilding the pipeline; call order is z-order. IT FAILS SILENTLY BY DESIGN: `build()` never errors — with no GPU adapter, or on a target whose compositor is not implemented yet, it returns a LIVE-BUT-EMPTY output stream so callers compile and run everywhere. A blank product therefore means \"no backend,\" not \"bad parameters.\" PLATFORM REALITY: macOS is the implemented, hardware-verified backend (zero-copy IOSurface in and out); web composites through a hidden Canvas2D `<canvas>` + `captureStream()` but does NOT render `.draw()` graphics; other native targets run the same path with no GPU layer compositor yet. `watermark_svg` is behind the off-by-default `svg` feature (resvg is a heavy tree); text watermarks need a caller-supplied font (no system fonts, so it works on wasm)."
);
sdk!(
    "canvas",
    SdkCategory::Media,
    SdkKind::Api,
    "Author-facing facade for the 2D-drawing SDK (GPU canvas + self-capture compositor)."
);
sdk!(
    "video",
    SdkCategory::Media,
    SdkKind::External,
    "Third-party `Video` playback primitive (a scene-registry payload). REGISTRATION REQUIRED ON EVERY TARGET: call `video::register(registry)` from your `register_scene_extensions` — the scene registry has no fallback handler, so an unregistered payload PANICS at realize. See the `sdks` guide's \"Registering extension SDKs\" section."
);
sdk!(
    "video-decode",
    SdkCategory::Media,
    SdkKind::Api,
    "Decode a video file into frames — the file-decoder peer of `camera` / `screen-recorder`."
);

// ---------------------------------------------------------------------
// UI — component library + scene-registry extension primitives
// ---------------------------------------------------------------------

sdk!(
    "idea-ui",
    SdkCategory::Ui,
    SdkKind::External,
    "The cross-platform component library — `Button`, `Card`, `Field`, `Select`, etc. Its `#[component]`s surface in `list_components` once linked."
);
sdk!(
    "idea-ui-mail",
    SdkCategory::Ui,
    SdkKind::External,
    "Email-safe component set — the opinion layer over the deliberately un-opinionated `backend-email` renderer: `EmailBody` (full-bleed canvas), `EmailContainer` (centered fixed-width content column), `Section`, `Heading`, `Text`, `Button` (an `external_link` call-to-action), `Divider`, `Spacer`. FOR EMAIL RENDERS, NOT SCREENS: every component builds its `StyleRules` DIRECTLY and reads no theme, so `idea-theme` tokens do NOT apply and colors are plain CSS-color strings passed as props. That is deliberate — a headless email render has no theme lifecycle and mail clients need inline styles — but it also means these components will not follow your app's theme if you drop them into a normal screen; use `idea-ui` there. Compose a template as an ordinary `#[component]` and hand it to the `email` SDK's `.template(|| Welcome(props))`, which renders it to email-safe HTML with styles inlined."
);
sdk!(
    "idea-theme",
    SdkCategory::Ui,
    SdkKind::Api,
    "Theming abstraction + extensibility for the idealyst design system."
);
sdk!(
    "icons-lucide",
    SdkCategory::Ui,
    SdkKind::Api,
    "Lucide icon pack — only icons you reference end up in the binary."
);
sdk!(
    "webview",
    SdkCategory::Ui,
    SdkKind::External,
    "Third-party `WebView` primitive. The canonical single-crate extension pattern. REGISTRATION REQUIRED ON EVERY TARGET: call `webview::register(registry)` from your `register_scene_extensions` — an unregistered payload PANICS at realize. See the `sdks` guide's \"Registering extension SDKs\" section."
);
sdk!(
    "maps",
    SdkCategory::Ui,
    SdkKind::External,
    "Third-party `MapView` primitive. WEB REGISTRATION REQUIRED: call `maps::register(&mut backend)` from your wasm32 `register_extensions` or it renders an unsupported-`External` placeholder (runtime, not compile-time); native self-registers. See the `sdks` guide's \"Registering External UI SDKs\" section."
);
sdk!(
    "svg",
    SdkCategory::Ui,
    SdkKind::External,
    "Third-party SVG renderer. WEB REGISTRATION REQUIRED: call `svg::register(&mut backend)` from your wasm32 `register_extensions` or it renders an unsupported-`External` placeholder (runtime, not compile-time); native self-registers. See the `sdks` guide's \"Registering External UI SDKs\" section."
);
sdk!(
    "markdown",
    SdkCategory::Ui,
    SdkKind::External,
    "CommonMark/GFM document primitive. WEB REGISTRATION REQUIRED: call `markdown::register(&mut backend)` from your wasm32 `register_extensions` or it renders an unsupported-`External` placeholder (runtime, not compile-time); native self-registers. See the `sdks` guide's \"Registering External UI SDKs\" section."
);
sdk!(
    "codeblock",
    SdkCategory::Ui,
    SdkKind::External,
    "Two code surfaces. `code_block(spans)` is the read-only colored-text panel. `code_editor(value_signal, on_change)` is the EDITABLE one: an editor whose syntax highlighting and underlines come from author-supplied BYTE RANGES — `.decorate(|text| Vec<Decoration>)` for a synchronous tokenizer, `.decorations(read_signal)` for async diagnostics — so the primitive never parses anything and works for any language. Decorations overlap and layer field-by-field, which is how a red `Underline` sits on a syntax-colored token without clearing its color; stale/mid-character ranges clamp rather than panic. Font/size/line-height/padding are `.font()`/`.line_height()`/`.padding()` METRICS, not `.with_style()` — styling one of its two layers and not the other is the drift it exists to prevent. It does not soft-wrap and does not scroll internally: put it in a `scroll_view`. WEB REGISTRATION REQUIRED: call `codeblock::register(&mut backend)` from your wasm32 `register_extensions` (and the SSR bootstrap, so first paint matches) or it renders an unsupported-`External` placeholder (runtime, not compile-time); native self-registers. See the `sdks` guide's \"Registering External UI SDKs\" section."
);
sdk!(
    "pdf",
    SdkCategory::Ui,
    SdkKind::External,
    "Render PDF pages as an element — `hayro` interprets the document and this crate bridges every drawing instruction into a renderer-agnostic `canvas_core::Scene` (text becomes GPU glyph runs, vectors Fill/Stroke, images Image). CANVAS RENDERER REQUIRED: a PDF *is* a canvas scene, so there is NOTHING to register on the scene registry, but nothing paints unless the app installs a canvas renderer at boot — `canvas_vello::register` (GPU) or `canvas_native::register` (CPU fallback) — exactly the contract `charts` has. NOT A `ui!` TAG: it is the fn-call form only, spliced as a child expression — `{ pdf::Pdf(pdf::PdfView { bytes, page, width }) }` (height follows the page aspect ratio; the page is interpreted ONCE at build) or `{ pdf::PdfReactive(move || bytes_signal.get(), page, w, h) }` for a document loaded at runtime, which fits the page into a FIXED w x h box and re-interprets only when the `Rc<Vec<u8>>` identity changes. DEGRADES SILENTLY: a corrupt or ENCRYPTED document renders an empty element and logs at `warn` rather than panicking — hayro does not support encrypted PDFs. Known approximations, reported per render in `Warnings` rather than dropped: pattern/shading fills (gradients, tiling) draw as nothing, soft masks are ignored, blend modes outside {Normal, Multiply, Screen} downgrade to Normal, dashed strokes render solid. Standard-14 fallback fonts and the predefined CJK cmaps are bundled, so non-embedded standard fonts still render."
);
sdk!(
    "table",
    SdkCategory::Ui,
    SdkKind::External,
    "Cross-platform table — a real `<table>` on web, shared-track CSS-grid on native. REGISTRATION REQUIRED on every target: call `table::register(&mut registry)` from your `register_scene_extensions`, or the payload panics at realize (the scene registry has no fallback handler — a missed registration fails loud, it does not render a placeholder). To ship the web handlers in a lazy chunk instead, call `table::defer(&mut registry)` at boot and `table::register_from_chunk::<MyBackend>()` from inside a `#[component(lazy)]` body. Before hand-rolling a grid from `view`/`text`, reach for this SDK. See the `sdks` and `sdk-components` guides."
);
sdk!(
    "charts",
    SdkCategory::Ui,
    SdkKind::External,
    "Reactive charting: `Chart` (line / smooth / stepped / area / bar / stacked / scatter / heatmap), `PieChart` (pie / donut), `RadialChart` (radial bars / gauges). Ordinary `#[component]`s — no scene-registry payload, so nothing to register for the components themselves. CANVAS RENDERER REQUIRED: a chart is a canvas AUTHOR and installs none, so call `canvas_native::register` (Canvas2D on web, CoreGraphics on macOS) or `canvas_vello::register` at boot, exactly as any other canvas consumer does — without one the marks never paint and the chart looks blank. SIZING: the chart fills its container, so that container must have a resolvable height (a fixed height, or `flex_grow` inside a parent that has one) — a chart in a purely auto-height column has no height to take. NO BUILT-IN TOOLTIP by design: `on_hover` is the whole hover mechanism and the app renders its own surface outside the chart\'s tree; `ChartHover` carries a `PointerFrame` (pointer in plot-local AND window space, plus the plot\'s viewport rect) and each `HitResult` carries its anchor `position` and its drawn `MarkBounds`, which is everything a cursor-following, snap-to-mark, or pinned-axis placement needs. Companion crate `charts-core` is renderer-agnostic (spec + rect -> marks, label placements, hit index) and depends on no runtime crate. See the [[charts]] guide.",
    guide = "charts"
);
sdk!(
    "form",
    SdkCategory::Ui,
    SdkKind::External,
    "Third-party `Form` container SDK (a scene-registry payload). Author it as a first-class tag: `ui! { Form(on_submit = Some(cb)) { text_input(value = name) button(label = \"Save\", on_click = cb) } }` — the `on_submit` closure reads your field signals (NOT DOM FormData); share the same `Rc` with your submit button so one action covers every backend. WEB: on web `Form` renders a real `<form>` (free Enter-to-submit, autofill); elsewhere it is a passthrough container (submission is fired by your submit button). Call `form::register(registry)` from your `register_scene_extensions` on EVERY target — an unregistered payload PANICS at realize. Need imperative `.submit()`? Use the fn-call form `form(props).bind(ref)` — the `ui!` tag form drops the handle."
);
sdk!(
    "virtualized",
    SdkCategory::Ui,
    SdkKind::Api,
    "Virtualized-collection constructors over the framework's builtin `flat_list` / virtualizer windowing engine: `list(data, key, size, render)`, `grid(.., columns)`, and `responsive_grid(.., min_item_cross)` — the latter derives its lane count from the measured container width like CSS `repeat(auto-fill, minmax(min, 1fr))`, so a resize or rotation re-lanes it. NOTHING TO REGISTER: the recycling engine is a framework PRIMITIVE (it needs UICollectionView / RecyclerView / NSCollectionView, which cannot be composed from `view`/`scroll_view`), and this crate is only the opinion layer above it — no scene-registry payload, no boot call. Each constructor returns the SAME `GlueFlatList` builder the primitive does, so `.axis(Axis::Horizontal)`, `.gap(n)`, `.spacing(main, cross)`, `.overscan(f)`, and `.bind(ref)` (for `scroll_to_index`) still chain. A list is just a one-lane grid — the constructors differ only in the preset lane count, not the engine, and `grid(.., 1)` degrades to a list. Deliberately data- and style-agnostic: you supply `Signal<Vec<T>>` + key + item-size + render closures, and it has NO styling, headers, or selection model — that is higher-level policy (the `table` SDK can sit on top). Reach for this before hand-rolling windowing over `scroll_view`."
);
sdk!(
    "toolbar",
    SdkCategory::Ui,
    SdkKind::External,
    "Third-party `Toolbar` SDK. WEB REGISTRATION REQUIRED: call `toolbar::register(&mut backend)` from your wasm32 `register_extensions` or it renders an unsupported-`External` placeholder (runtime, not compile-time); native self-registers. See the `sdks` guide's \"Registering External UI SDKs\" section."
);
sdk!(
    "menu",
    SdkCategory::Ui,
    SdkKind::Api,
    "OS-level menu-bar SDK (desktop)."
);

// ---------------------------------------------------------------------
// Navigation — render navigators (`Element::Navigator`)
// ---------------------------------------------------------------------

// The two navigator SDK crates that exist today: `stack-navigator` (push/pop
// depth) and `swap-navigator` (flat Select). Tab and drawer experiences are
// author layouts over the swap model with idea-ui-nav chrome (AppShell /
// TabBar / Drawer) — there is no separate tab-/drawer-navigator crate, so no
// entry (the drift audit checks this table against the crate listing).
sdk!(
    "stack-navigator",
    SdkCategory::Ui,
    SdkKind::Api,
    "Push/pop stack navigator with native screen transitions and a back stack. Fluent BUILDER, not a `ui!` tag: `StackNavigator::new(&ROOT).screen(ROUTE, |params| Screen::new(...).title(...)).layout(|nav| ...).bind(nav_ref)` with `const ROOT: Route<()> = Route::<()>::new(\"home\", \"/\")` route consts (typed params via `Route<P>` + `RouteParams`). Import BOTH extension traits — `use stack_navigator::{StackBuilder, StackScreenExt, ...}` — or `.screen(...)`/`.title(...)` won't resolve. Compile-checked `stack_two_screens` recipe registers once this crate is a dependency. See the [[navigation]] guide.",
    guide = "navigation"
);
sdk!(
    "swap-navigator",
    SdkCategory::Ui,
    SdkKind::Api,
    "Tabs and drawers: this is the tab-bar / drawer / bottom-nav / screen-switcher SDK — there is NO separate tab- or drawer-navigator crate. A flat screen-swap navigator (the Select model) — one visible screen, no back stack — is the substrate; the tab bar / drawer is author layout wrapping the navigator's `{ nav.outlet }` with idea-ui-nav's `TabBar` / `Drawer` / `AppShell`, wired to `nav.active_route` + `nav.on_select`. Same builder shape as the stack: `SwapNavigator::new(&HOME).screen(HOME, |_| Screen::new(...)).screen(SEARCH, ...).layout(|nav| ui!{ view { { nav.outlet } TabBar(items = vec![TabItem::new(\"home\", \"Home\")], active_route = nav.active_route, on_select = nav.on_select) } }).bind(nav_ref)`; import the `SwapBuilder` extension trait (`use swap_navigator::{SwapBuilder, SwapHandle, SwapNavigator};`) or `.screen(...)` won't resolve. `swap_three_screens_tab_bar` recipe registers once this crate is a dependency. See the [[navigation]] guide.",
    guide = "navigation"
);
sdk!(
    "idea-ui-nav",
    SdkCategory::Ui,
    SdkKind::External,
    "Themed navigation CHROME for the two navigators: `AppShell` (responsive pinned-sidebar <-> drawer, plus `sidebar_pinned`), `TabBar` (+ `TabItem`, and `TabIndicator` re-exported from idea-ui), `Drawer` (+ `DrawerSide`), and `StackHeader`. IT DOES NOT NAVIGATE: under the outlet model a navigator owns only its single `{ nav.outlet }` and everything wrapping it is ordinary author layout — these components ARE that layout, and they only do anything once you wire them to the context the navigator's `.layout(|nav| ...)` closure hands you (`active_route = nav.active_route`, `on_select = nav.on_select`). This is why there is NO tab- or drawer-navigator crate: a tab bar is `swap-navigator` + this crate's `TabBar`; a drawer is `swap-navigator` + `Drawer`/`AppShell`. Built on the same `idea-theme` as `idea-ui` and piggybacks on its `Tabs` strip rather than reimplementing chrome. Features mirror idea-ui: `docs` (reflective doc-control panels) and `robot` (forward a `test_id` to the root interactive primitive). See the [[navigation]] guide.",
    guide = "navigation"
);

// ---------------------------------------------------------------------
// Device — input gestures + device capabilities
// ---------------------------------------------------------------------

sdk!(
    "pan",
    SdkCategory::Device,
    SdkKind::Api,
    "Pan-gesture SDK — a reactive value handle tracking drag offset for author-level pan interactions."
);
sdk!(
    "zoom",
    SdkCategory::Device,
    SdkKind::Api,
    "Zoom-gesture SDK — reactive scale from a pinch recognizer (touch) plus a wheel/magnify channel (web `wheel`+ctrlKey / macOS `magnify:`)."
);
sdk!(
    "gesture",
    SdkCategory::Device,
    SdkKind::Api,
    "Gesture ARBITRATION — drive several recognizers against ONE view's single `on_touch` slot and resolve the conflicts that slot alone cannot: `GestureGroup::add` (add order = priority), `require_to_fail(dependent, prerequisite)` (UIKit's `require(toFail:)` — the tap that waits to see the pan fail so a drag never also selects), and `allow_simultaneous(a, b)` (pan + pinch together). Then `view(...).on_touch(g.handler())`. IT OWNS THE SLOT: `handler()` CONSUMES the group and produces the one `TouchHandler` for that view, so installing any other `on_touch` on the same view replaces the whole arbiter. IT SHIPS NO RECOGNIZERS — the FSMs come from the substrate (`runtime_shared::touch`: `TapRecognizer`, `PanRecognizer`, ... or your own `Recognizer` impl); if you have a single gesture with nothing to arbitrate, use `pan` / `zoom` or the raw recognizer directly and skip this crate. Recognizers are driven in dependency order within each event, so a prerequisite that fails on that same event has already failed by the time its dependent runs — no event replay. A cancelled loser was still `Possible` and has emitted nothing, which is why exclusivity is side-effect-free."
);
sdk!(
    "dnd",
    SdkCategory::Device,
    SdkKind::Api,
    "Cross-platform IN-APP drag and drop — reorderable lists, kanban columns, sortable grids, drag-into-trash. Three handles: a shared `DragContext<T>` (clone it into every participant; `T` is your payload type), `Draggable<T>` (`.on_release(|DropOutcome|)`, installed with `.on_touch(card.handler())`), and `Droppable<T>` (reactive `.is_over()`, `.on_drop(|payload|)`). BOTH SIDES MUST `.bind(Ref<ViewHandle>)`: drop targets are hit-tested in WINDOW space via `absolute_frame()`, so an unbound participant is simply invisible to the drag. MOUNT `dnd::drag_layer(&ctx)` ONCE, as (or near) the LAST child of your app root, or the drag ghost never appears — and do NOT substitute a fullscreen overlay/portal: its viewport-covering root swallows the pointermove/pointerup the drag depends on, leaving a permanently stuck drag that looks like a freeze. IN-APP ONLY: cross-application drag (out to Finder, accepting a drop from another app) and the browser's native HTML5 `DataTransfer` are a documented, NOT-YET-IMPLEMENTED seam (`dnd::native`). Auto-scrolling near a list edge, reorder animations, and multi-select drag are deliberately left as policy — build them on `DragContext::dragging` / `Droppable::is_over`, the way `pan` leaves momentum to the caller. Pure Rust on the same `Recognizer` FSM as `pan`, so no per-platform code. Compile-checked recipes register once this crate is a dependency."
);
sdk!(
    "offload",
    SdkCategory::Device,
    SdkKind::Api,
    "Run a CPU-heavy function OFF the main thread from one platform-agnostic call site: declare `#[offload::job] fn rasterize(req: Req) -> Out { ... }` (a free fn) and call `offload::run(offload::handle!(rasterize), &req).await?`. On web it runs in a real Web Worker via `wasmworker` with NO `SharedArrayBuffer` — so no COOP/COEP cross-origin-isolation headers are needed and embedding keeps working — reusing the same `--target web` wasm (there is no second build artifact); on native it runs on a `std::thread` with a oneshot delivering the result back to the `.await`. THE CRATE THAT DEFINES A JOB NEEDS TWO EXTRA WASM DEPS: on wasm32 `#[offload::job]` IS wasmworker's `#[webworker_fn]`, and its expansion references `wasmworker` and `wasm_bindgen` BY NAME, so that crate must add `wasmworker = { version = \"0.4\", features = [\"macros\"] }` and `wasm-bindgen = \"0.2\"` under `[target.'cfg(target_arch = \"wasm32\")'.dependencies]` or the wasm build fails to compile. Call sites only ever name `offload`. Job argument and return types must be `serde::Serialize + Deserialize` — the web backend ships them across the worker boundary, native clones them."
);
sdk!(
    "biometrics",
    SdkCategory::Device,
    SdkKind::Api,
    "Cross-platform biometric authentication — prove the device owner is present."
);

// ---------------------------------------------------------------------
// Device — Tier 1 platform-integration capabilities. `permissions` is the
// shared runtime-grant substrate the prompting ones (notifications,
// location, camera, microphone) delegate to.
// ---------------------------------------------------------------------

sdk!(
    "permissions",
    SdkCategory::Device,
    SdkKind::Api,
    "Cross-platform runtime permission requests — the shared grant substrate. `permissions::request(Permission)` / `status(Permission)` returning a uniform `PermissionStatus`. Capability SDKs that prompt the user depend on this instead of re-implementing an OS grant flow."
);
sdk!(
    "notifications",
    SdkCategory::Device,
    SdkKind::Api,
    "Local + scheduled notifications and the raw device push token. `notify`/`schedule`/`cancel`; authorization goes through `permissions`. Server-side push delivery is the app's job."
);
sdk!(
    "clipboard",
    SdkCategory::Device,
    SdkKind::Api,
    "System copy/paste of plain text — `clipboard::set_text` / `text`. UIPasteboard / NSPasteboard / ClipboardManager / `navigator.clipboard`."
);
sdk!(
    "location",
    SdkCategory::Device,
    SdkKind::Api,
    "Device geolocation — one-shot `current()` and continuous `watch()` yielding a `Position`. Permission grant goes through `permissions`."
);
sdk!(
    "share",
    SdkCategory::Device,
    SdkKind::Api,
    "The system share sheet (outbound) — hand text/url/files to another app. `share(ShareContent)`. The inverse of `file-picker`. UIActivityViewController / ACTION_SEND / `navigator.share`."
);
sdk!(
    "deep-link",
    SdkCategory::Device,
    SdkKind::Api,
    "Inbound URL handling — `initial_link()` + `on_link()` deliver the parsed launch/resume URL (custom scheme / universal / app link). The host forwards URLs in via `feed_link`."
);
sdk!(
    "connectivity",
    SdkCategory::Device,
    SdkKind::Api,
    "Network reachability — `current()` snapshot + `watch()` of online/offline and coarse transport (wifi/cellular/ethernet). NWPathMonitor / ConnectivityManager / `navigator.onLine`."
);
sdk!(
    "haptics",
    SdkCategory::Device,
    SdkKind::Api,
    "Tactile feedback — `impact`/`notify`/`selection`. Fire-and-forget, best-effort. UIFeedbackGenerator / Vibrator / `navigator.vibrate`."
);
sdk!(
    "auto-update",
    SdkCategory::Device,
    SdkKind::Api,
    "Desktop self-update for DIRECTLY-DISTRIBUTED apps — a Developer ID `.dmg` (Sparkle's atomic bundle swap), a `.msix` (MSIX App Installer), a `.AppImage` (AppImageUpdate zsync delta). NOT FOR STORE OR MOBILE BUILDS: on iOS, web, and store distributions every entry point is a deliberate no-op that resolves to `UpdateState::Unsupported` (Apple forbids self-update; a reload IS the update on web) — read `InstallKind` and hide your UI rather than expecting a check to do anything. ONE reactive handle: `auto_update::install(UpdateConfig::new(manifest_url, channel, PUBLIC_KEY, env!(\"CARGO_PKG_VERSION\"), build))` returns an `Updater` whose `Signal<UpdateState>` (Idle -> Checking -> Available -> Downloading{progress} -> ReadyToRelaunch, or UpToDate / Failed) you bind ONCE and render identically on every backend; drive it with `updater.check().await` then `updater.download_and_install().await`. TLS IS NOT THE TRUST ANCHOR — you must set up signing, not just host a JSON file: every manifest entry is Ed25519-signed over its `(version|build|url|sha256)` tuple against a public key baked into the binary at BUILD time, and the downloaded artifact's SHA-256 must match the signed digest before anything is installed, so a compromised CDN cannot ship an accepted release. Manifest fetch rides the `net` SDK (cross-platform); only the apply step is a per-platform seam."
);
sdk!(
    "audio",
    SdkCategory::Media,
    SdkKind::Api,
    "Sound playback — `load(AudioSource)` → a `Sound` you `play()`, with a controllable `Playback` (pause/stop/volume/loop). The playback peer of the capture SDKs. AVAudioPlayer / MediaPlayer / HTMLAudioElement."
);
