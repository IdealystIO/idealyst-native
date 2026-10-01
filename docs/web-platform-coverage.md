# Web platform coverage

Idealyst owns the boundary between an app and the browser. An app gets
every browser capability it needs from a framework backend or SDK, and
does not call `web-sys`, `js-sys` or `wasm-bindgen` itself. There are
two reasons:

- **Portability.** A `web_sys::window()` call only works on web, so the
  app has to wrap it in `#[cfg(target_arch = "wasm32")]` and write a
  second path for every other target. The SDK call works on every
  target, and each target gives its own answer, even when that answer
  is "there is no address bar here".
- **The binding layer is the framework's to change.** The framework's
  own crates are off `wasm-bindgen` (they bind the browser through
  `web-glue`), so an app that goes through the SDKs builds without the
  wasm-bindgen CLI at all (own mode). An app that imports `web-sys` /
  `wasm-bindgen` directly is tied to them: its web build runs in hybrid
  mode and needs the `wasm-bindgen` CLI at the version in its
  `Cargo.lock`.

This page lists every browser capability a real app has needed, which
framework API covers it, and how each backend answers. The list started
from an audit of CrewForge, the main consumer app, where 15 files in 6
crates called `web-sys` directly. **When an app needs a browser
capability that isn't listed here, the fix is a row on this page and an
SDK method, not a `web-sys` call in the app.**

Status values:

- **covered**: the API already existed.
- **added**: the API was added for this audit. See the SDK's README for
  details.
- **gap**: no framework API covers it yet. Each gap row says what the
  API should be.

Platform shorthand: *native* means iOS, macOS, Android, Windows and
Linux desktop. The SSR/SSG backend reports `Platform::Web`, but it has
no `window`, so every web-only answer below is `None` or a no-op there
too.

## Navigation and the URL

On native targets the navigators keep the app's position in memory, so
there is no URL to read or rewrite. `None` and no-op are the real
answers there, not unfinished stubs.

| Capability | Needed by (CrewForge) | Framework API | Web | Native | Status |
| --- | --- | --- | --- | --- | --- |
| Read the path the app is at right now (`location.pathname`), before any navigator has mounted: `/share/<token>` skips the auth gate, restore the section from a cold deep link | `app-main/src/app.rs`, `app-main/src/shell/mod.rs`, `screens/organizations/src/lib.rs`, `app-checkin/src/app.rs` | `deep_link::current_url() -> Option<DeepLink>` (`.path`) | `location.href`, parsed; reflects navigator writes and Back/Forward | `None` | added |
| Read query-string state (`location.search`) | `app-kit/src/url_query.rs` | `deep_link::current_url()` (`.query`, `.query_pairs()`). A navigator screen's own state has a separate channel: `ScreenState` / `screen_query()` (runtime-core) | `location.href` | `None` | added |
| Change the address bar without navigating (`history.replaceState`): drop `/login` after sign-in, send the landing page to `/`, keep filters in `?k=v` | `app-main/src/app.rs`, `app-kit/src/url_query.rs` | `deep_link::replace_url(url)` | `history.replaceState(<current state>, "", url)`. Keeps the entry's `history.state`, adds no history entry, fires no `on_link` | no-op | added |
| The page origin (`location.origin`), used for the API base URL and for absolute share links | `app-main/src/app.rs`, `app-checkin/src/app.rs`, `screens/projects/src/projects/contacts/model.rs`, `screens/projects/src/projects/reports/share.rs`, `app-kit/src/new_tab.rs` | `deep_link::origin() -> Option<String>` | `location.origin` (`None` for an opaque `"null"` origin) | `None` | added |
| The URL that launched the app, and links that arrive while it runs | none | `deep_link::initial_link()`, `deep_link::on_link()` | `location.href` at boot | host calls `feed_link` from the OS open-URL callback | covered |
| Move between screens in the app | none (the navigators already do it) | `NavHandle::{push,select,replace,reset}[_with_state]`, the `link` primitive | History API through the navigator substrate | in-memory stack | covered |

## Windows and tabs

| Capability | Needed by | Framework API | Web | Native | Status |
| --- | --- | --- | --- | --- | --- |
| Open a URL in a new tab, where the new page gets no `window.opener` handle back into the app | `app-kit/src/new_tab.rs`, `app-checkin/src/admin.rs` | `runtime_core::open_url(url)`. To open another copy of the app, pass `origin() + path` | `window.open(url, "_blank", "noopener")`. Before this change the web backend omitted `noopener`, which is why the app called `window.open` itself | iOS `UIApplication.open`, Android `ACTION_VIEW`, macOS `NSWorkspace.open`. No-op on terminal, CPU and runtime-server | covered (web `noopener` added) |
| Full screen | none | `runtime_core::set_fullscreen(bool)` | Fullscreen API | per backend | covered |

## Theme and colour scheme

| Capability | Needed by | Framework API | Web | Native | Status |
| --- | --- | --- | --- | --- | --- |
| The OS light/dark preference (`prefers-color-scheme`) | none: CrewForge only stores an explicit user choice (see Storage below) | `runtime_core::color_scheme()`. Theme palettes follow the preference automatically | `matchMedia` at boot, plus emitted `prefers-color-scheme` rules | the platform's appearance setting | covered |

## Storage

| Capability | Needed by | Framework API | Web | Native | Status |
| --- | --- | --- | --- | --- | --- |
| Read a saved preference before the first frame, and save it synchronously (`localStorage.getItem` / `setItem`): the saved light/dark mode, the collapsed sidebar, the kiosk board's mode | `app-kit/src/theme.rs`, `app-main/src/sidebar_collapse.rs`, `app-checkin/src/theme.rs` | `storage::platform_storage(ns)` with `.get_now(key)` / `.set_now(key, v)` / `.remove_now(key)` | `localStorage`, keys `ns:key` | iOS/macOS `NSUserDefaults`, Android `SharedPreferences`, Windows/Linux JSON file | added (sync accessors; the store existed) |
| Async key-value storage, and a signal that loads from and saves to it | none | `Storage::get` / `set` / `remove`, `storage::persisted_signal` | same | same | covered |
| Secrets | none | `credentials` SDK | web returns an error instead of pretending to be secure | Keychain / Keystore | covered |

Moving a key onto `platform_storage` changes where it is stored: the SDK
prefixes keys with the namespace, so `cf-theme` becomes
`crewforge:cf-theme`. Each user's saved preference resets once. A
migration shim would have to read the old key with `web-sys`, which is
the call this change removes.

## Network status

| Capability | Needed by | Framework API | Web | Native | Status |
| --- | --- | --- | --- | --- | --- |
| Is the device online right now (`navigator.onLine`) | `app-checkin/src/clock.rs` | `connectivity::current().online`. Use `connectivity::watch()` for change events | `navigator.onLine`, plus `online` / `offline` events | NWPathMonitor (Apple), ConnectivityManager (Android), NetworkManager (Linux); Windows reports "assume online" | covered |

## Timers and clock

| Capability | Needed by | Framework API | Web | Native | Status |
| --- | --- | --- | --- | --- | --- |
| One-shot and repeating timers | `app-checkin/src/clock.rs` (already on the framework) | `after_ms_scoped` / `after_ms_detached` / `raf_loop_scoped` | `setTimeout` / rAF | platform run loop | covered |
| Current time for animations and instrumentation | none | `runtime_core::time::now_micros()` | `performance.now()` (needs `install_time_source`) | monotonic clock | covered |
| The current date and time (`chrono::Utc::now()`): stamp a check-in, default a report or roster to today, measure elapsed minutes | `app-checkin/src/clock.rs`, `app-checkin/src/overrides/state.rs`, `app-checkin/src/roster/mod.rs`, `api/src/domains/attendance.rs`, `api/src/domains/safety_tips.rs`, `core/src/domains/attendance.rs`, `core/src/domains/completeness.rs` | `datetime::now_utc()` (a `Timestamp`); with the `chrono` feature, `datetime::now_chrono_utc()` returns a `DateTime<Utc>` | `Date.now()` | `SystemTime`. Works without a mounted backend, so server code and tests get the real time too | added |
| The local time zone (`chrono::Local::now()`): the device's UTC offset at an instant, correct across DST changes, and the zone's IANA name | `app-checkin/src/clock.rs` (the device offset, as `Local::now().offset()`) | `datetime::local_offset_at(t)`, `datetime::local_offset()`, `datetime::local_timezone()`; with `chrono`, `datetime::now_chrono_local()` returns a `DateTime<FixedOffset>` | `new Date(ms).getTimezoneOffset()`, `Intl.DateTimeFormat().resolvedOptions().timeZone` | `localtime_r` (Apple, Android, Linux), `SystemTimeToTzSpecificLocalTimeEx` (Windows), through the `zone-offset` crate, which every native backend's wall clock also reads, so `runtime_core::time::local_offset_minutes()` (the framework's date UI) gives the same offset; zone name from `$TZ`, else the system setting | added |

chrono's own clock is the other way an app ends up in hybrid mode
without calling `web-sys` itself. `Utc::now()` and `Local::now()` reach
the browser only through chrono's `wasmbind` feature, which links
`wasm-bindgen` and `js-sys`. Without `wasmbind` they call
`std::time::SystemTime::now()`, which panics on
`wasm32-unknown-unknown`. The fix is to turn off chrono's `clock` and
`wasmbind` features in the web-compiled crates and read the clock
through `datetime`. The datetime README has the replacement for each
call.

## Text input

| Capability | Needed by | Framework API | Web | Native | Status |
| --- | --- | --- | --- | --- | --- |
| An editable field with syntax highlighting: coloured tokens under a live caret, IME and selection, with long lines wrapping inside a fixed-width column | `app-kit/src/formula_highlight.rs`, which kept a DOM mirror `<div>` in sync with a `<textarea>` through `web-sys` | `codeblock::code_editor(value, on_change)`, `.decorate(|text| Vec<Decoration>)`, `.soft_wrap(true)` | `<pre>` with styled runs under a transparent `<textarea>`; both use the same wrapping rules. Verified in the browser (`tests/web_soft_wrap.rs`) | the same handler on every backend (attributed text under the native text area); compile-checked, not yet tested on devices | added (`soft_wrap`; the editor existed) |
| A textarea's imperative handle: focus, select, insert at the caret | formula pickers | `Ref<TextAreaHandle>` via `.bind(..)` on `text_area` and on `code_editor` | DOM | native | covered |

The `soft_wrap` work also fixed a backend-web bug. When a wrapping
`text_area` was created, the web backend measured it while it was still
detached from the page and set `height: 0px`. The later measurement
skips absolutely positioned textareas, so nothing ever removed that
height. A wrapping editing layer stretched over other content collapsed
to its padding. Detached textareas are no longer measured.

## Networking

| Capability | Needed by | Framework API | Web | Native | Status |
| --- | --- | --- | --- | --- | --- |
| HTTP | many (through `server`) | `net::Client` | `fetch` | NSURLSession / HttpURLConnection / reqwest | covered |
| WebSocket with binary frames (`binaryType = arraybuffer`, `ArrayBuffer` → `Uint8Array`) | `ui-shared/src/voice_stream.rs` (AWS Transcribe streaming) | `net::WebSocket::connect`, `WsMessage::Binary(Vec<u8>)`, `WsSender` | `WebSocket` | `tungstenite` on an I/O thread. `wss://` works on iOS, macOS and desktop; Android supports only `ws://` (see gaps) | covered |
| How a WebSocket closed (`CloseEvent.code` / `reason`): tell a normal end (1000) from a failure | `ui-shared/src/voice_stream.rs` | `WebSocket::close_status() -> Option<WsClose>` | `CloseEvent`. The browser supplies 1005 (close frame without a code) and 1006 (connection dropped with no close frame) | reads the peer's close frame. Native reports the same 1005 / 1006 a browser would | added |
| Server-sent events | none | `net::EventSource` | `EventSource` | worker thread | covered |

## Audio

| Capability | Needed by | Framework API | Web | Native | Status |
| --- | --- | --- | --- | --- | --- |
| Microphone PCM buffers (sample rate, channels, `f32` samples) | `ui-shared/src/voice_stream.rs`, `ui-shared/src/voice.rs` (already on the framework) | `microphone::Microphone::open(config, callback)` | `getUserMedia` + Web Audio (`ScriptProcessorNode`) | `cpal` (CoreAudio / WASAPI / ALSA); Android via `AudioRecord` | covered |
| Record to an encoded file | `ui-shared/src/voice.rs` | `media-writer` | MediaRecorder | platform encoders | covered |
| Play back audio | none | `audio::load(..).play()` | `HTMLAudioElement` | `AVAudioPlayer` on Apple; per-platform arms elsewhere | covered |

## Clipboard, share, files

| Capability | Needed by | Framework API | Status |
| --- | --- | --- | --- |
| Copy text | share-link cards (already on the framework) | `clipboard::set_text` | covered |
| Share sheet | none | `share::share(ShareContent)` | covered |
| Pick / save files | avatar editor, CSV export (already on the framework) | `file-picker`, `file-export`, `files` | covered |

## Gaps

These are known limits of the framework today. None of them requires an
app to call `web-sys`.

| Gap | Effect | Proposed fix |
| --- | --- | --- |
| Android WebSocket supports only `ws://` | `net`'s Android arm uses `tungstenite` without a TLS stack, so a `wss://` URL such as a presigned AWS Transcribe URL cannot connect on Android. It doesn't affect CrewForge today because its dictation runs only on web: `microphone` is a wasm-only dependency there. | Add an OkHttp `WebSocket` arm through JNI. This is already `net`'s documented next step for Android, and it also brings the OS proxy and certificate store. |
| Web `storage` keys always carry the namespace prefix | An app can't adopt a `localStorage` key that was written without the SDK, so a move onto the SDK resets saved preferences once (see Storage above). | None proposed. A one-time reset is acceptable, and an unprefixed escape hatch would bring back the collisions the namespace exists to prevent. |
| `code_editor` on native is compile-checked only | Soft wrap and highlighting have not been tested on iOS, macOS or Android. | Run the device checklist in `crates/sdk/client/codeblock/README.md`. |
