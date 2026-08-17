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
    "storage",
    SdkCategory::Data,
    SdkKind::Api,
    "Cross-platform INSECURE key-value storage for non-sensitive app data. PREFERRED for signal-backed state: `storage::persisted_signal(namespace, key, initial) -> Signal<T>` (crate feature `reactive`) — hydrates on creation, persists on change, race-correct (a user write before the async load resolves WINS; hand-rolled load/persist wiring almost always clobbers it). Raw API: `storage::platform_storage(namespace) -> Arc<dyn Storage>` with ASYNC `get(key) -> Result<Option<String>, StorageError>` / `set` / `remove` — call via `runtime_core::driver::spawn_async` (needs runtime-core's `async-driver` feature; generated wrappers enable it). Backends: localStorage (web), NSUserDefaults (iOS/macOS), SharedPreferences (Android), JSON file (desktop). The `persisted_signal` recipe registers once storage is a dependency of the build. No security claims — use `credentials` for secrets."
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
    "table",
    SdkCategory::Ui,
    SdkKind::External,
    "Cross-platform table — a real `<table>` on web, shared-track CSS-grid on native. REGISTRATION REQUIRED on every target: call `table::register(&mut registry)` from your `register_scene_extensions`, or the payload panics at realize (the scene registry has no fallback handler — a missed registration fails loud, it does not render a placeholder). To ship the web handlers in a lazy chunk instead, call `table::defer(&mut registry)` at boot and `table::register_from_chunk::<MyBackend>()` from inside a `#[component(lazy)]` body. Before hand-rolling a grid from `view`/`text`, reach for this SDK. See the `sdks` and `sdk-components` guides."
);
sdk!(
    "form",
    SdkCategory::Ui,
    SdkKind::External,
    "Third-party `Form` container SDK (a scene-registry payload). Author it as a first-class tag: `ui! { Form(on_submit = Some(cb)) { text_input(value = name) button(label = \"Save\", on_click = cb) } }` — the `on_submit` closure reads your field signals (NOT DOM FormData); share the same `Rc` with your submit button so one action covers every backend. WEB: on web `Form` renders a real `<form>` (free Enter-to-submit, autofill); elsewhere it is a passthrough container (submission is fired by your submit button). Call `form::register(registry)` from your `register_scene_extensions` on EVERY target — an unregistered payload PANICS at realize. Need imperative `.submit()`? Use the fn-call form `form(props).bind(ref)` — the `ui!` tag form drops the handle."
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
    "audio",
    SdkCategory::Media,
    SdkKind::Api,
    "Sound playback — `load(AudioSource)` → a `Sound` you `play()`, with a controllable `Playback` (pause/stop/volume/loop). The playback peer of the capture SDKs. AVAudioPlayer / MediaPlayer / HTMLAudioElement."
);
