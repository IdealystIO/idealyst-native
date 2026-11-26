# `notifications`

Cross-platform **local notifications and their responses**. Show a notification now or after a delay, with action buttons and a reply field. Update or cancel it by id, and hear what the user did with it. A raw **push-token** seam covers app-owned remote delivery. One author API covers every backend; the platforms differ in *mechanism*, not in the surface you call. What a platform does beyond the shared surface is reached through that platform's **hook**, which hands you its own notification object before it is posted.

Authorization goes through the [`permissions`](../permissions) crate (`Permission::Notifications`). `authorize()` is a thin convenience over `permissions::request(...)`.

```rust
use notifications::{authorize, notify, on_response, Action, Notification, Priority, ResponseAction};

// At boot, before the first screen mounts: hear every response,
// including the tap that launched the app.
let _responses = on_response(|r| match &r.action {
    ResponseAction::Open => open_chat(&r.data["chat"]),
    ResponseAction::Action(id) if id == "reply" => send_reply(&r.data["chat"], r.text.as_deref()),
    ResponseAction::Action(id) if id == "mute" => mute(&r.data["chat"]),
    _ => {}
});

# async fn demo() -> Result<(), notifications::NotifyError> {
if authorize().await.is_granted() {
    notify(
        Notification::new("Sam", "Lunch at noon?")
            .id("chat-sam")                 // re-post with the same id to replace it
            .with("chat", "sam")            // payload, handed back with every response
            .group("chats")
            .priority(Priority::High)
            .action(Action::new("reply", "Reply").text_input("Send", "Message"))
            .action(Action::new("mute", "Mute").opens_app(false).destructive()),
    )
    .await?;
}
# Ok(())
# }
```

## The surface

**Posting**

- `Notification::new(title, body)` and its setters:
  - `.subtitle(...)`;
  - `.id(...)`, a stable string for update and cancel;
  - `.with(k, v)` / `.data(map)`, an opaque payload (keys starting with `idealyst.` are reserved);
  - `.action(Action)`, a button;
  - `.priority(Priority)`, one of `Low`, `Default`, `High`, `Urgent`;
  - `.sound(Sound)`, `Default` or `Silent`;
  - `.group(...)`, which stacks notifications together;
  - `.image(...)`, a local file path on native, a URL on web.
- `Action::new(id, title)` and its setters:
  - `.text_input(button, placeholder)`, a reply field;
  - `.opens_app(bool)`, default `true`;
  - `.destructive()`;
  - `.requires_unlock()`.
- `notify(n)` delivers now and `schedule(n, after)` delivers after a delay. Both return the `NotificationId` used.
- `cancel(&id)` / `cancel_all()` take back pending and delivered notifications.
- `authorize() -> PermissionStatus` asks for the permission. Gate posting on `.is_granted()`.
- `push_token()` returns the device's APNs / FCM / web-push token for *app-owned* delivery. This SDK never sends a remote push.

**Responses**

- `on_response(|r: &NotificationResponse| …)` runs on the UI thread for every response. Dropping the returned guard unregisters the handler. A response carries:
  - `id`;
  - `action`: `Open` (a tap on the body), `Dismiss`, or `Action(action_id)`;
  - `text`, the reply;
  - `data`, the payload.
- Responses that arrive before any handler exists are held and handed to the first one. A handler registered at boot therefore receives the tap that cold-started the app.
- `init()` installs the platform listener and is called by `on_response` / `notify` / `schedule`. On iOS and macOS the launch tap only reaches the app if this happened before launch finished, so call `init()` or register the handler at boot.

**Words**

- `set_labels(Labels { … })` replaces the words the user sees in system settings: Android's channel names, one per priority plus one for silent notifications. They are English until replaced.

## Shared fields per platform

A field a platform can't show is dropped, never faked.

| Field | iOS / macOS | Android | Windows | Linux | Web |
| --- | --- | --- | --- | --- | --- |
| subtitle | `subtitle` | sub-text | a line of its own | folded into the body | folded into the body |
| actions | category actions (max 4) | `addAction` (max 3) | toast `<action>` (max 5) | `actions` | dropped (page API has none) |
| reply field | `UNTextInputNotificationAction` | `RemoteInput` | toast `<input>` | KDE `inline-reply` (one) | dropped |
| priority | interruption level: Low passive, Default/High active, Urgent time-sensitive | one channel per level (Low → low importance, High/Urgent → high importance, heads-up) | Low → no popup, High/Urgent → high priority | `urgency` hint 0/1/2 | Urgent → `requireInteraction` |
| replace (same id) | alerts again | alerts again | alerts again | alerts again | alerts again (`renotify`) |
| silent | no sound set | the silent channel (normal importance whatever the priority, since sound belongs to the channel) | `<audio silent>` | `suppress-sound` | `silent` |
| group | `threadIdentifier` | `setGroup` | dropped | dropped | dropped |
| image | attachment (the OS moves the file into its store) | large icon + big picture | hero image | `image-path` | `image` (URL) |

## Responses per platform

| Platform | Tap / actions / reply | Dismiss | While the app is not running |
| --- | --- | --- | --- |
| iOS / macOS | `UNUserNotificationCenter` delegate | yes (categories carry `customDismissAction`) | launches or wakes the app; delivered once it boots |
| Android | a translucent activity (taps and opening actions), a receiver (background actions) | delete intent | queued in `SharedPreferences`, delivered on the next launch |
| Windows | toast `Activated` | `Dismissed` with `UserCanceled` | not delivered: an unpackaged app needs a COM activator; scheduled toasts have no events |
| Linux | `ActionInvoked` / `NotificationReplied` signals | `NotificationClosed` reason 2 | not delivered (signals reach running clients only) |
| Web | `click` (focuses the page) | not reported (`close` also fires on expiry) | not delivered (page-level API) |

Every response removes its notification, as iOS does on its own: Android cancels it after an action, Windows and Linux close it, and web closes it on click.

## Per-platform hooks

The hook setters exist only on their own target (`#[cfg]`-gated). Each receives the platform object after the shared fields are filled in and before posting:

| Target | Setter | Receives |
| --- | --- | --- |
| iOS / macOS / tvOS | `.apple(\|content: &AnyObject\| …)` | the `UNMutableNotificationContent` (objc2) |
| Android | `.android(\|env, builder\| …)` | the `Notification.Builder` (jni) |
| Windows | `.windows(\|doc: &XmlDocument\| …)` | the toast XML document |
| Linux | `.linux(\|hints\| …)` | the `Notify` hints map (zvariant values) |
| Web | `.web(\|options\| …)` | the `NotificationOptions` JSON object |

```rust
#[cfg(target_os = "android")]
let n = n.android(|env, builder| {
    env.call_method(builder, "setProgress", "(IIZ)Landroid/app/Notification$Builder;",
        &[100.into(), 40.into(), false.into()])?;
    Ok(())
});
```

## Per-platform mechanism

| Target | Post / schedule | Push token |
| --- | --- | --- |
| web (wasm32) | `new Notification(title, options)`, kept by tag so `cancel` can close it. `schedule` → `NotSupported`. **Runnable** (browser tests). | `NotSupported` (needs a service worker + VAPID key) |
| iOS / macOS / tvOS | `UNUserNotificationCenter`. Categories are registered on demand and awaited before the post (`getNotificationCategories` answers once the set is applied). `schedule` → `UNTimeIntervalNotificationTrigger`. Without an app bundle (a test binary) every call reports `Backend` instead of aborting. **Compile-checked only ⚠️** | `NotSupported` (host seam) |
| Android | Kotlin shim (`runtime/kotlin/…/RustNotifications.kt`) + manifest fragment, both picked up by the run tool. `schedule` → `AlarmManager`; the alarm carries the built notification. It is exact with `SCHEDULE_EXACT_ALARM`, inexact otherwise, and lost on reboot. **Compile-checked only ⚠️** (Rust and Kotlin) | `NotSupported` (needs Firebase) |
| Linux (desktop) | `org.freedesktop.Notifications` over the session D-Bus (`zbus`); re-posting an id replaces via `replaces_id`. `schedule` → `NotSupported`. | `NotSupported` |
| Windows | WinRT toasts; the id is the toast `Tag`. `schedule` → `ScheduledToastNotification`. Posting and scheduling **verified on a Windows 11 VM**; actions and responses compile-checked only ⚠️ | `NotSupported` (WNS needs a packaged app) |
| other native (incl. test host) | `notify`/`schedule` are no-op successes; nothing responds | `NotSupported` |

## Push is a *seam*, not a service

`push_token()` is the only remote-push surface here. Registering for remote push and *delivering* a message need host wiring the app owns: the iOS `registerForRemoteNotifications` app-delegate callback and `aps-environment` entitlement, an FCM project, a web service worker. They also need a server to send through. Where that wiring isn't present, `push_token()` returns `NotSupported` rather than fabricating a token.

## Permissions

Declares the `notifications` capability in `[package.metadata.idealyst]`. The CLI injects the per-target requirement:

- **Android** → `android.permission.POST_NOTIFICATIONS` (runtime, API 33+), plus this crate's `NotificationOpenActivity` and `NotificationReceiver` (`manifest/application.xml`).
- **iOS / macOS** → no plist key; the runtime prompt is shown by `authorize()`. Links `UserNotifications`. `Priority::Urgent` (time-sensitive) needs the `com.apple.developer.usernotifications.time-sensitive` entitlement, declared by the app; without it the OS delivers the notification as active.
- **web** → no manifest entry; `authorize()` calls `Notification.requestPermission()`.

## Robot test controls

A test can't read the device's notification shade or tap it. A robot (dev) build logs what the app posted and lets the test act as the user. Register the controls once at boot (a no-op in release builds):

```rust
notifications::register_robot_controls();
```

| Function | Does |
|---|---|
| `notifications.log` | Returns, oldest first: every `notify` / `schedule` since launch (`{op, id, title, body, subtitle, data, actions, delay_ms, error}`, with `error` set when the post failed), every `cancel` / `cancel_all`, and every response the handlers received (`{op: "response", id, action: "open"\|"dismiss"\|"action", action_id, text, data}`). |
| `notifications.clear_log` | Empties the log. |
| `notifications.respond` | `{id, action?, text?, data?}`: delivers a response to the `on_response` handlers exactly as a real one is. `action` is `"open"` (the default), `"dismiss"`, or an action id. `data` defaults to the payload of the last post logged under `id`. |

```rust
app.test_id("ask").click();
app.call_function("notifications.respond", json!({ "id": "chat-sam", "action": "reply", "text": "yes" }));
assert_eq!(app.call_function("notifications.log", json!({}))[1]["op"], "response");
```

## Testing checklist

An unchecked **native** box means the code compiles for that target but isn't confirmed on real hardware yet.

**Automated**
- [x] `cargo test -p notifications`: builder, labels, the response queue (launch response held for the first handler), Apple category ids and the no-bundle guard (macOS), Linux hint/action mapping and signal handling (Linux), Windows toast XML and activation arguments (Windows), Android shim encodings (Android), robot log + respond.
- [x] `cargo build -p notifications --features catalog`: recipes compile.
- [x] `cargo test -p notifications --target wasm32-unknown-unknown` (headless Chrome through the workspace runner): `tests/web_notify.rs`.
- [x] Kotlin shim compiles against `android.jar` (API 36).

**Behavior**
- [ ] **Web**: a click focuses the page and reports `Open` with the payload; re-posting the same id replaces; `cancel` closes.
- [ ] **iOS / macOS**: banner while in the foreground; action buttons and the reply field show on the first post; tap, button, reply and swipe-away each report once; the tap that launches a killed app reaches a boot-time handler.
- [ ] **Android**:
  - tap, opening action, background action, reply and swipe-away each report once;
  - a tap on a killed app is delivered after launch, and a background action on a killed app on the next launch;
  - the trampoline activity never shows;
  - `schedule` fires, and `cancel` / `cancel_all` remove the pending alarm.
- [ ] **Windows**: buttons and the reply field show; the reply text arrives; dismissing reports `Dismiss`.
- [ ] **Linux**: GNOME (buttons) and KDE (inline reply); an action is not followed by a `Dismiss`.
- [ ] **Push seam**: `push_token()` returns `NotSupported` until the host wiring lands.
