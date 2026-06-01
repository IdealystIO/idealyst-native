# Handoff — Drawer header consolidation + mobile icon/safe-area fixes

Snapshot of an experiment that ran in **idealyst-native** (framework) and
**../quill-emr** (the QuillEMR demo app). Goal: make the drawer navigator's
menu/header chrome look and behave identically on web, iOS, and Android by
moving it to the **page level** (each screen renders its own header with a
menu button) and removing per-platform native chrome. Several framework bugs
were found and fixed along the way (mobile icons, safe-area, a reactive
viewport crash).

> Repo note: the working tree contains other unrelated in-flight work
> (`crates/api/server/*`, `examples/login-demo/`, `examples/idea-ui-docs/*`,
> `crates/backend/ios/core/border.rs`, `crates/sdk/credentials/*`). None of
> that is part of this experiment. The user manages commits; some of this
> work may already be committed.

---

## What was built (the feature)

A drawer screen renders its **own header with a hamburger** instead of relying
on a backend-native nav bar (iOS `UINavigationController`) / Toolbar (Android).
The button is driven by an ambient hook so screens don't have to wire the
drawer by hand.

- **`runtime_core::primitives::navigator::ambient_drawer() -> Option<DrawerChrome>`**
  (`crates/runtime/core/src/primitives/navigator/chrome.rs`). `DrawerChrome {
  open: Rc<dyn Fn()>, collapse_below: f32 }`. A drawer navigator publishes it
  at `init`; any screen/component reads it to render a menu button. Mirrors
  `ambient_scroll_context`. Published by web/iOS/Android handlers
  (`_set_ambient_drawer`). **macOS deliberately does not publish** (web-style
  persistent sidebar; no hamburger needed).
- **`collapse_below`** is the viewport width below which the drawer is modal.
  Web = `navigator_pin_width()` (default 1024, the `@media` pin breakpoint);
  iOS/Android = `f32::INFINITY` (always modal → button always shows).
- The button (`DrawerMenuButton` in `quill-emr/src/components/kit.rs`) compares
  `viewport_size().get().width < collapse_below` **inside a `ui!` region** so
  it appears/disappears reactively on web as the viewport crosses the pin
  breakpoint. **On mobile (`collapse_below` infinite) it renders the button
  UNCONDITIONALLY — no reactive `if`** (see gotcha #6).

### `native_header(bool)` — suppress native chrome navigator-wide
`crates/sdk/drawer-navigator/src/lib.rs`:
- New field `DrawerPresentation::native_header` (default `true`) + builder
  `DrawerBuilder::native_header(bool)`.
- New shared helper `resolve_header_shown(per_screen, native_header)`: a
  per-screen `.header_shown(...)` always wins; otherwise `native_header=false`
  force-hides. Used by **both** iOS and Android so precedence is identical and
  host-testable.
- iOS (`ios.rs`) and Android (`android.rs`) handlers store `native_header` and
  apply `resolve_header_shown` in **both** `mount_screen` AND `attach_initial`
  (the initial screen goes through `attach_initial`, a separate path — missing
  it left the FIRST screen's native bar visible). Android helper
  (`android-navigator-helpers/src/tab_drawer.rs`) `attach_toolbar_to_body` now
  early-returns when `header_shown == Some(false)` (previously it never read
  `header_shown` at all).
- The app opts out via `DrawerNavigator::new(...).native_header(false)` in
  `quill-emr/src/app.rs`.

---

## Mobile icon fixes (framework)

Icons had no Taffy intrinsic size on mobile and were positioned by stale
create-time LayoutParams. Three distinct bugs, all fixed:

1. **No intrinsic size → collapse/stretch.** iOS now installs an icon
   `measure_fn` (`install_icon_measure`, default 24×24) in `create_icon`
   (`backend/ios/mobile/src/imp/mod.rs` + `imp/icon.rs::DEFAULT_SIZE`).
   Android reuses the generic `install_external_measure_fn` on the icon
   ImageView (`backend/android/mobile/src/imp/mod.rs::create_icon`), which reads
   the drawable's intrinsic size; `FIT_CENTER` then scales the glyph.
2. **iOS positioning discarded (gotcha #2).** `create_icon` was setting
   `translatesAutoresizingMaskIntoConstraints = false` + Auto Layout size
   constraints, which handed frame control to Auto Layout (size-only, no
   position) and **threw away Taffy's computed position** — icons rendered
   hard top-left. Removed it; icons now use manual frames like every other
   view.
3. **Android intrinsic size in wrong units (gotcha #3).** The drawable's
   intrinsic size was set to viewBox units (~24 **px** ≈ 7dp), not 24dp.
   Fixed to `(24dp × density)` px so the measure-fn yields 24dp. Also removed
   a create-time `ViewGroup.LayoutParams(24,24)` (margin-less) that pinned the
   icon to the parent origin (`backend/android/mobile/src/imp/primitives/icon.rs`).
4. **iOS glyph centering within its view:** `sync_icon_sublayer`
   (`backend/ios/core/src/style.rs`), called from `apply_frames`, re-centers
   the CAShapeLayer in the view bounds (the gradient-sublayer-sync pattern).

Net: icons are now Taffy-driven (size via measure-fn, position via frame) and
render centered + correctly sized on web/iOS/Android.

---

## Safe-area (interim, non-edge-to-edge)

- The QuillEMR `Sidebar` (`quill-emr/src/components/sidebar.rs`) opts in with
  `.safe_area(SafeAreaSides::TOP | SafeAreaSides::BOTTOM)` on its root view
  (chained after the `ui!` block — `ui!` supports trailing `.method()` chains).
- **iOS**: correct — UIKit per-view `safeAreaInsets` insets once below the
  notch / above the home indicator (the app is full-screen under the notch).
- **Android (interim)**: `platform_safe_area_insets`
  (`backend/android/mobile/src/imp/mod.rs`) now **zeros the top inset**. The
  activity is **not edge-to-edge**, so the system already lays content below
  the status bar; reporting the raw status-bar inset double-inset every
  `.safe_area(TOP)` surface (~2× height). Bottom is kept (the gesture pill
  genuinely overlays). This is clearly commented as interim.
- **DEFERRED → see Open items**: proper fix is to make Android edge-to-edge
  and have the framework's scroll/body container auto-inset (the iOS
  `.automatic` analogue), so `.safe_area()` means the same thing everywhere.

---

## Other framework fix this session

**Android reactive-`viewport_size` re-entrancy crash (SIGABRT on startup).**
`AndroidBackend::viewport_size()` is called inside `run_layout_pass` and used
to mirror into the reactive `set_viewport_size` signal **synchronously** — any
subscriber (the new menu button) re-ran its effect mid-layout → panic during a
panic → abort. Fixed by **deferring the mirror to a microtask** (deduped by
last size) so the notify runs after the layout pass — matching how iOS/web
update the viewport outside layout. (`backend/android/mobile/src/imp/mod.rs`,
`LAST_MIRRORED_VIEWPORT`.)

---

## App (quill-emr) changes

- `app.rs`: `.native_header(false)` on the drawer navigator.
- `components/kit.rs`: `DrawerMenuButton` + `MenuButtonInner` (36×36 card box;
  `flex_shrink: 0` so crowded headers don't squeeze it; renders directly on
  mobile, reactively on web).
- All six screens (`schedule/today/patients/notetaker/messages/settings.rs`)
  render `DrawerMenuButton()` at the start of their header row.
- `components/sidebar.rs`: distinct `palette::SIDEBAR` (`#f1ebe3`) background so
  the rail reads as chrome vs the page; `.safe_area(TOP|BOTTOM)`.
- `palette.rs`: added `SIDEBAR`.
- `icons.rs`: added `menu()` (hamburger).

---

## Tests

- `crates/sdk/drawer-navigator/src/lib.rs` (`#[cfg(test)]`): `native_header`
  default-on, `.native_header(false)` opt-out, and `resolve_header_shown`
  precedence matrix. All pass (`cargo test -p drawer-navigator`).
- `crates/sdk/drawer-navigator/tests/ssr.rs`: realigned the sidebar-CSS
  assertion to the current responsive modal+pinned CSS (it was asserting the
  old `flex:0 0 auto` rule and failing pre-existing).
- Platform-specific fixes (iOS UIKit / Android JNI / layout) verified
  **on-device/sim** via screenshots + `uiautomator dump` bounds + a debug
  trace; documented in code why tighter host unit tests aren't reachable.

---

## Verification status

| Area | Web | iOS | Android |
|---|---|---|---|
| Page-level menu button on all 6 screens | ✅ | ✅ | ✅ |
| Native nav bar / Toolbar removed | n/a | ✅ | ✅ |
| Menu icon centered + correct size | ✅ | ✅ | ✅ |
| Distinct sidebar background | ✅ | ✅ | ✅ |
| Sidebar top safe-area | n/a | ✅ | ✅ (interim) |
| Drawer opens from button | ✅ | ✅ (assumed) | ✅ |

Measured menu button = 36×36, glyph centered both axes on web + iOS + Android.

---

## Open / deferred items

1. **Android edge-to-edge** (the real safe-area fix). Currently zeroing the
   Android top inset as an interim. Proper: edge-to-edge activity + framework
   body container auto-insets (iOS `.automatic` analogue) + handle gesture-bar
   / IME insets. The user explicitly deferred this to a dedicated effort.
2. **Web modal drawer has no scrim/backdrop** — when the narrow drawer opens,
   the content is fully visible beside it (observed, never raised by the user).
3. **Android BACK key** from an open drawer left the screen blank once during
   testing (separate nav concern; not investigated).
4. **`native_header` over the wire** — `register_wire_drawer_factory` defaults
   it `true`; not threaded through the wire config (irrelevant under `--local`).

---

## Build & run notes

- **Always `idealyst dev --<platform> --local`** (web/ios/android). The default
  runtime-server/hot-reload path is mid-rewrite and fails on a missing
  `sidecar` feature. `--local` is a one-shot build → install → launch (it does
  NOT stay running as a watcher).
- `idealyst dev` is the only sanctioned build path for iOS/Android (never Xcode
  / direct cargo for device builds). `cargo check --target {aarch64-apple-ios-sim,
  aarch64-linux-android}` is fine for type-checking framework crates.
- iOS sim: `simctl launch` on an already-running app does NOT reload the new
  binary — `simctl terminate` first, then launch.
- Sim/emulator inspection used: `xcrun simctl io <dev> screenshot`,
  `adb exec-out screencap -p`, `adb shell uiautomator dump` (exact view bounds).

---

## Reusable framework gotchas (the valuable bits)

1. **iOS icon views must use manual frames.** Don't set
   `translatesAutoresizingMaskIntoConstraints = false` + size constraints on a
   framework view — Auto Layout then owns the frame and discards Taffy's
   computed position. The default (`true`) is the manual-frame behavior the
   layout pass needs.
2. **Mobile icons need a Taffy `measure_fn`** for intrinsic size, and on
   Android the drawable's **intrinsic size must be dp-correct** (px = dp ×
   density), not viewBox units.
3. **Android `viewport_size()` must not notify reactive subscribers
   synchronously during `run_layout_pass`** — defer the mirror (microtask),
   or a subscriber's effect re-enters layout and double-panics → SIGABRT.
4. **Drawer screens are mounted as *content*, not slot closures**, so they
   don't get `SlotProps`. The `ambient_drawer()` thread-local is how page-level
   chrome reaches `open()` / `collapse_below`.
5. **`attach_initial` is a separate path from `mount_screen`** on both native
   handlers — any per-screen option resolution (like `header_shown`) must be
   applied in BOTH or the first screen behaves differently.
6. **A reactive `ui!` `if`/region wrapper as a flex child can fail to get a
   Taffy frame on native**, especially inside a `flex_wrap` container (the
   Schedule header) — the wrapped content stretched/mis-positioned. When the
   condition is constant (mobile `collapse_below = INFINITY`), render the child
   directly instead of through the reactive region.
7. **`flex_shrink` defaults to 1** — a fixed-size item (the 36×36 menu button)
   gets squeezed in a crowded non-wrapping row. Pin `flex_shrink: 0` for
   fixed-size chrome.
8. **Android non-edge-to-edge + `.safe_area()` double-insets** — the inset
   query reports the full status-bar inset even when the system already inset
   the content. `.safe_area()` assumes the surface extends under the system
   bars (true on iOS, not on a non-edge-to-edge Android activity).
