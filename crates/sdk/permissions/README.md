# `permissions`

Cross-platform **runtime-permission** requests — the shared substrate the
capability SDKs (`notifications`, `location`, …) build on instead of each
re-implementing an OS grant flow. A permission is two facts: the *requirement*
(the manifest entry / plist key the build injects, declared by each capability
SDK) and the *runtime grant* (the user tapping "Allow"). This crate owns the
second half: a tiny, unopinionated surface to read and request a permission's
current grant state, uniform across every backend.

```rust
use permissions::{request, status, Permission, PermissionStatus};

# async fn demo() {
// Read the current grant without prompting.
if status(Permission::Notifications).await == PermissionStatus::Undetermined {
    // Prompt the user; resolves to the resulting state.
    let granted = request(Permission::Notifications).await.is_granted();
    let _ = granted;
}
# }
```

## What you get

Two `async` free functions plus two small enums — that's the whole surface:

- `status(Permission) -> PermissionStatus` — the current grant state,
  **without** prompting. Cheap to call repeatedly.
- `request(Permission) -> PermissionStatus` — shows the OS prompt when the
  state is `Undetermined`, resolves to the resulting state. On a platform
  where the permission needs no grant it resolves to `Unsupported` without
  prompting.
- `PermissionStatus` — `Granted` / `Denied` / `Restricted` / `Undetermined` /
  `Unsupported`, with `is_granted()` and `is_usable()` (`Granted` *or*
  `Unsupported`) helpers — the common gate before invoking a capability.
- `Permission` — `Notifications`, `LocationWhenInUse`, `LocationAlways`,
  `Camera`, `Microphone` (`#[non_exhaustive]`).

Every backend delivers the **same shape** — the platforms diverge in
mechanism, not in the two functions you call.

## Per-platform mechanism

| Target | Status | Request |
| --- | --- | --- |
| web (wasm32) | `Notification.permission` (notifications); `navigator.permissions.query({name})` (geolocation / camera / microphone) | `Notification.requestPermission()` (notifications). Geolocation / camera / microphone have **no** explicit web request API — the prompt fires on first use (`getCurrentPosition` / `getUserMedia`); `request` reports current status (`Undetermined` = "will prompt on first use"). |
| iOS / macOS / tvOS | `UNUserNotificationCenter.getNotificationSettings` (notifications); `CLLocationManager.authorizationStatus` (location); `AVCaptureDevice.authorizationStatusForMediaType:` (camera = `"vide"`, microphone = `"soun"`) | `requestAuthorizationWithOptions:completionHandler:` (notifications); `CLLocationManager.requestWhenInUse/AlwaysAuthorization` + delegate (location); `AVCaptureDevice.requestAccessForMediaType:completionHandler:` (camera / microphone). Callback / delegate bridged to the `async fn` via a oneshot. |
| Android | `Context.checkSelfPermission(name)` (JNI) | `Activity.requestPermissions(...)` (JNI) + a host result-forwarding seam (see **Scope**). |
| Windows / Linux / other native | every permission → `Unsupported` (no native runtime-permission model) | same |

**Verification.** The **web** path is genuinely runnable and is the verified
backend. The **Apple** and **Android** native paths are **compile-checked
only** — exercising them needs a real device/simulator with the matching
plist usage strings (Apple) or a host that forwards
`onRequestPermissionsResult` (Android). Their structure mirrors the verified
`biometrics` / `camera` / `storage` SDKs; the block / delegate / JNI lifetime
invariants are documented inline.

The **camera / microphone** grants were *relocated* here from those SDKs (a
faithful move of the proven AVCaptureDevice / `checkSelfPermission` /
`navigator.permissions` code — same constants, same `RcBlock`/oneshot bridge),
so `camera` / `microphone` now call `permissions::request(Permission::Camera |
Microphone)` instead of carrying their own. The Apple camera path was
previously macOS-hardware-verified on its own copy; after the relocation it is
compile-checked through this crate and should be re-confirmed on a host/device
run. The capture paths are unchanged.

## Permissions

**None of its own.** This crate is the *mechanism* — the plist keys
(`NSLocationWhenInUseUsageDescription`, …) and Android manifest permissions
(`POST_NOTIFICATIONS`, `ACCESS_FINE_LOCATION`, …) are declared by the
capability SDKs that use it (`notifications`, `location`). It injects nothing.

## Scope

The raw `status` / `request` capability, nothing more. Deliberately left to
higher-level SDKs and to the host:

- **Reactive bindings / state** — no `Signal<PermissionStatus>` here; a caller
  wires `request` into its own reactive state (the recipe shows the pattern).
- **"Open app settings"** — re-prompting after a hard `Denied` requires
  sending the user to OS settings; that's a separate navigation capability.
- **Android request-result seam** — a runtime permission *request* is
  callback-delivered on Android via the Activity's
  `onRequestPermissionsResult`, which lives in the app's Kotlin/Java, not in
  this crate. `request` parks a oneshot keyed by a request code and exposes
  `permissions::complete_request(code, granted)` (plus a
  `Java_..._nativeOnPermissionsResult` JNI export) for the host to forward the
  result. Until that one line of host glue is wired, an Android `request`
  resolves to the re-read status rather than observing the grant — it never
  hangs, and it never fakes a grant. This is the honest integration seam.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p permissions` — portable logic (status helpers + host `Unsupported` fallback)
- [ ] `cargo build -p permissions --features catalog` — recipes/docs compile
- [ ] `cargo build -p permissions --target wasm32-unknown-unknown` — web target

**Behavior**
- [ ] **Web** — `status(Notifications)` reads `Notification.permission` without prompting; `request(Notifications)` shows the browser prompt and resolves `Granted`/`Denied`. Geolocation/camera/microphone `status` reports `Undetermined` ("prompts on first use") and never blocks.
- [ ] **iOS** — for each `Permission` variant, `request()` prompts once with the matching plist usage string and returns `Granted`/`Denied`; a second `request()` after grant doesn't re-prompt. `status()` reflects the grant without prompting.
- [ ] **macOS** — same per-variant `request()`/`status()` flow against `UNUserNotificationCenter`/`CLLocationManager`/`AVCaptureDevice`; re-confirm the relocated camera path still grants after the move.
- [ ] **Android** — `request()` raises the runtime dialog; the grant is observed only once the host forwards `onRequestPermissionsResult` via `complete_request(code, granted)`. Until that host glue is wired, `request` resolves to the re-read status — verify it never hangs and never fakes a grant.
