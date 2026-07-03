# `notifications`

Cross-platform **local notifications** — show a notification now or after a
delay, update or cancel it by id — plus a raw **push-token** seam for
app-owned remote delivery. One author API (`Notification` builder +
`notify` / `schedule` / `cancel` / `cancel_all`), every backend, identical
observable behavior; the platforms diverge in *mechanism*, not in the
surface you call.

Authorization is **not** re-implemented here — it goes through the
[`permissions`](../permissions) crate (`Permission::Notifications`).
`authorize()` is a thin convenience over `permissions::request(...)`.

```rust
use notifications::{authorize, notify, schedule, Notification};
use std::time::Duration;

# async fn demo() -> Result<(), notifications::NotifyError> {
// Ask once (the OS prompt, via the shared `permissions` crate).
if authorize().await.is_granted() {
    // Immediate local notification.
    notify(Notification::new("Saved", "Your note is safe")).await?;

    // After a delay, with a stable id so a later post replaces it
    // and `cancel` can take it back.
    let id = schedule(
        Notification::new("Reminder", "Stand up and stretch").id("break"),
        Duration::from_secs(60),
    )
    .await?;
    let _ = id;
}
# Ok(())
# }
```

## What you get

A `Notification` builder and four async ops with a uniform shape across
every backend:

- `Notification::new(title, body)` — plus `.subtitle(...)`, `.id(...)` (a
  stable string for update/cancel), `.with(k, v)` / `.data(map)` for an
  opaque payload (e.g. a deep-link route surfaced to your tap handler).
- `authorize() -> PermissionStatus` — the runtime grant, via `permissions`.
  Gate posting on `.is_granted()`.
- `notify(n) -> Result<NotificationId, NotifyError>` — deliver immediately.
- `schedule(n, after) -> Result<NotificationId, NotifyError>` — deliver after
  a `Duration` delay (the platform's interval/calendar trigger). Returns the
  id it was posted under.
- `cancel(&id)` / `cancel_all()` — take back pending/delivered notifications.
- `push_token() -> Result<PushToken, NotifyError>` — the device's remote-push
  token (APNs / FCM / web-push `PushSubscription` JSON) for *app-owned*
  delivery. **This SDK never sends a remote push** — it hands you the token;
  your server sends.

Posting with an explicit `id` again **replaces** the existing notification
(update semantics) on every backend.

## Per-platform mechanism

| Target | Local notify / schedule | Push token (`push_token`) |
| --- | --- | --- |
| web (wasm32) | `new Notification(title, {body, tag:id})` (immediate). `schedule` → `NotSupported` (no native delayed-delivery API). **Runnable.** | web-push `PushManager.subscribe` — **service-worker seam** (needs a registered SW + VAPID key) → `NotSupported` until the host installs it |
| iOS / macOS / tvOS | `UNUserNotificationCenter` + `UNMutableNotificationContent`; `schedule` uses `UNTimeIntervalNotificationTrigger` (objc2). **Compile-checked only ⚠️** | APNs device token via `registerForRemoteNotifications` + the app-delegate callback — **host seam** → `NotSupported` |
| Android | `NotificationManager` + a default `NotificationChannel` (API 26+) + `Notification.Builder`; `manager.notify(intTag, …)` keyed by a hash of the id so re-posts replace (JNI). **Compile-checked only ⚠️** Immediate `notify` is implemented fully; `schedule` → `NotSupported` (AlarmManager + a manifest `BroadcastReceiver` is a host seam) | FCM token via `FirebaseMessaging.getToken()` — **host seam** (Firebase project + `google-services.json`) → `NotSupported` |
| Windows / Linux / other native (incl. test host) | fallback: `notify`/`schedule` are no-op successes returning the resolved id; `cancel` no-ops | `NotSupported` |

The public surface is uniform — `notify` / `schedule` / `cancel` /
`push_token` behave the same everywhere a backend supports the op, and report
`NotifyError::NotSupported` honestly where a platform genuinely can't (rather
than faking success).

> The Apple and Android backends are **compile-checked only** — they message
> the real platform APIs through objc2 / JNI but have not been exercised on a
> device from this crate. The web immediate-`notify` path and the
> shared id/builder logic are host- and web-runnable.

## Push is a *seam*, not a service

`push_token()` is the **only** remote-push surface here: it returns the
platform token. Registering for remote push and *delivering* a message are
deliberately out of scope — they need host wiring the app owns (the iOS
`registerForRemoteNotifications` app-delegate callback, an FCM project, a web
service worker) and a server to send through. Where that wiring isn't present,
`push_token()` returns `NotSupported` rather than fabricating a token. This
keeps the SDK an honest *capability*, not a managed push service.

## Permissions

Declares the `notifications` capability in `[package.metadata.idealyst]`. The
CLI injects the per-target requirement:

- **Android** → `android.permission.POST_NOTIFICATIONS` (runtime, API 33+).
- **iOS / macOS** → no plist key; the runtime prompt is shown by `authorize()`
  (via the `permissions` crate's `UNUserNotificationCenter` authorization).
- **web** → no manifest entry; `authorize()` calls `Notification.requestPermission()`.

**Remote push needs more, declared by the app, not this crate:** the iOS
`aps-environment` entitlement + a `remote-notification` background mode (a
deeper build seam). This crate flags it; it does not wire it.

## Scope

Local notifications (immediate + a single delay) and the raw push-token
capability — the unopinionated raw surface. Rich notification actions and
attachments, notification categories/grouping, calendar/recurring schedules,
and server-side remote-push *delivery* are deliberately later layers, not
baked in here.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p notifications` — builder + `resolve_id` + host no-op `notify`/`authorize`
- [ ] `cargo build -p notifications --features catalog` — recipes/docs compile
- [ ] `cargo build -p notifications --target wasm32-unknown-unknown` — web target

**Behavior**
- [ ] **Web** — after `authorize()`, `notify()` shows a browser banner; re-posting with the same `.id(...)` (the `tag`) replaces it. `schedule()` returns `NotSupported`.
- [ ] **iOS** — `authorize()` shows the OS prompt; `notify()` shows a banner; `schedule(.., 60s)` fires after the delay via the interval trigger; re-post with the same id replaces; `cancel`/`cancel_all` clear pending/delivered.
- [ ] **Android** — Android 13+ shows the `POST_NOTIFICATIONS` runtime prompt first (confirm it's in the merged manifest); `notify()` posts to the default channel; re-post with the same id replaces (id hash → int tag). `schedule()` returns `NotSupported` (AlarmManager seam).
- [ ] **macOS** — `authorize()` + `notify()` show a Notification Center banner; id-replace and `cancel` behave.
- [ ] **Push seam** — `push_token()` returns `NotSupported` until the host installs the APNs/FCM/service-worker wiring; verify it never fabricates a token. Confirm a real token once that wiring lands.

**Permissions**
- [ ] iOS/macOS/web grant is driven by `authorize()` (via `permissions`); Android 13+ also needs `POST_NOTIFICATIONS` injected into the manifest — confirm both the prompt and the manifest entry.
