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
| Android | `Context.checkSelfPermission(name)` (JNI) | `Activity.requestPermissions(...)` through this crate's `RustPermissions` Kotlin shim; the answer comes back through the generated `MainActivity.onRequestPermissionsResult` (see **Android request round trip**). |
| Windows | notifications: the app's notification setting (`ToastNotifier.Setting`: on → `Granted`, off by the user → `Denied`, off by policy → `Restricted`; an app not yet listed in Settings is on → `Granted`). Location: the privacy setting via `AppCapability::CheckAccess("location")` (allowed / denied by user / by system / prompt required → `Granted` / `Denied` / `Restricted` / `Undetermined`). Camera / microphone: the privacy settings via `AppCapability::CheckAccess("webcam" / "microphone")`, same mapping | notifications: same as status (no prompt). Location: `Geolocator::RequestAccessAsync` — Windows' "Let Windows and apps access your location?" prompt when undecided. Camera / microphone: `AppCapability::RequestAccessAsync` when undecided |
| Linux / other native | every permission → `Unsupported` (no native runtime-permission model) | same |

**Verification.** The **web** path and the **Android** request path are
verified (see the checklist). The **Apple** native paths are
**compile-checked only** — exercising them needs a real device/simulator with
the matching plist usage strings. Their structure mirrors the verified
`biometrics` / `camera` / `storage` SDKs; the block / delegate / JNI lifetime
invariants are documented inline.

The **camera / microphone** grants live here (AVCaptureDevice /
`checkSelfPermission` / `navigator.permissions`), so `camera` / `microphone`
call `permissions::request(Permission::Camera | Microphone)` instead of
carrying their own. The Apple camera path is compile-checked through this
crate; confirm it on a device run.

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

## Android request round trip

Android delivers a permission answer only to the host Activity's
`onRequestPermissionsResult` override. The round trip:

1. `request` parks a oneshot under a fresh request code and calls the
   `io.idealyst.permissions.RustPermissions` Kotlin shim this crate ships
   (`[package.metadata.idealyst.android] runtime_kotlin`, compiled in by
   `idealyst run android`).
2. The shim registers a handler with the backend's
   `io.idealyst.runtime.RustPermissionsResult` registry and calls
   `Activity.requestPermissions`.
3. The `MainActivity` that `idealyst run android` generates forwards
   `onRequestPermissionsResult` to `RustPermissionsResult.dispatch`. The
   handler calls the `nativeOnPermissionsResult` JNI export, which resolves
   the oneshot: `Granted` / `Denied`, or `Undetermined` when the prompt was
   interrupted (Android's empty result arrays).

A custom host Activity must make the same one-line forward; without it,
`request` waits until app teardown.

## Robot test controls

A system permission prompt is the OS's, and an E2E test can't tap it. A
robot (dev) build can fake the answer instead, so a test assumes the grant
(or the denial) it needs. Register the controls once at boot — a no-op in
release builds:

```rust
permissions::register_robot_controls();
```

| Function | Effect |
|---|---|
| `permissions.set {permission, status}` | `status()` reports `status` and `request()` resolves to it, with no prompt. `permission`: `camera`, `microphone`, `notifications`, `location_when_in_use`, `location_always`; `status`: `granted`, `denied`, `restricted`, `undetermined`, `unsupported`. |
| `permissions.clear {permission?}` | The OS answers again (for every permission when left out). |
| `permissions.requests {clear?}` | What the app requested, oldest first, faked or not. |

```rust
app.call_function("permissions.set", json!({ "permission": "camera", "status": "denied" }));
app.test_id("scan").click();
app.test_id("camera-denied-banner").assert_visible();
```

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [x] `cargo test -p permissions` — portable logic (status helpers, host `Unsupported` fallback, the Android pending-request registry)
- [ ] `cargo build -p permissions --features catalog` — recipes/docs compile
- [ ] `cargo build -p permissions --target wasm32-unknown-unknown` — web target
- [x] `cargo test -p permissions --target wasm32-unknown-unknown` (headless Chrome through the workspace runner) — `tests/web_permissions.rs`: no `Notification` API → `Unsupported`; notification status/request mapping incl. the callback-only `requestPermission`; `permissions.query` states and every failure shape → `Unsupported`

**Behavior**
- [ ] **Web** — `status(Notifications)` reads `Notification.permission` without prompting; `request(Notifications)` shows the browser prompt and resolves `Granted`/`Denied`. Geolocation/camera/microphone `status` reports `Undetermined` ("prompts on first use") and never blocks.
- [ ] **iOS** — for each `Permission` variant, `request()` prompts once with the matching plist usage string and returns `Granted`/`Denied`; a second `request()` after grant doesn't re-prompt. `status()` reflects the grant without prompting.
- [ ] **macOS** — same per-variant `request()`/`status()` flow against `UNUserNotificationCenter`/`CLLocationManager`/`AVCaptureDevice`; re-confirm the relocated camera path still grants after the move.
- [x] **Android** (Lenovo TB336FU, API 36, via `camera::Camera::open`; CAMERA revoked first) — `request()` raises the runtime dialog and resolves `Granted` on Allow and `Denied` on Deny, on the first ask; a second `request()` after a grant doesn't re-prompt.
