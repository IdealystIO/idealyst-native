# Deep links

A deep link is a URL from outside the app that opens a screen inside it:
`myapp://projects/42` (a **custom scheme**) or
`https://example.com/projects/42` (a **universal link** on Apple, an **App
Link** on Android). Idealyst opens the linked screen on every target, using
the same rules as a URL on web.

- **The link launches the app:** the first mount opens the linked screen.
  If it is a stack screen, the stack's configured first screen goes below
  it, so Back returns there.
- **The app is already running:** the navigators move to the same screen.
  A swap navigator selects it. A stack navigator pushes it on top of where
  the user was. A navigator whose part of the path is already showing stays
  as it is, so only the nested navigator that has to change moves.

The same URL opens the same screen either way, and app code needs nothing
to make that happen. The [`deep-link` SDK](../crates/sdk/client/deep-link/README.md)
is there for app code that wants to see links, hold them, or replay them.

## 1. Declare the links

An OS only gives a URL to an app that declares it. Declare the links in the
app's `Cargo.toml`:

```toml
[package.metadata.idealyst.app.links]
# Custom schemes — myapp://…
schemes = ["myapp"]
# Domains whose https links open the app. "*.example.com" covers subdomains.
domains = ["example.com"]
# Needed for the domains: written into the verification files (step 2).
apple_team_id = "ABCDE12345"                  # developer.apple.com → Membership
android_cert_fingerprints = ["AB:CD:EF:…"]    # SHA-256 of the signing cert
```

The build adds what each platform needs:

| | `schemes` | `domains` |
|---|---|---|
| iOS | `CFBundleURLTypes` in Info.plist | `com.apple.developer.associated-domains` entitlement on device / App Store builds |
| macOS | `CFBundleURLTypes`; the dev bundle is registered with LaunchServices | the same entitlement in `publish macos` |
| Android | a `VIEW` / `BROWSABLE` intent filter | an `autoVerify` https intent filter per domain |
| web | — | `/.well-known/apple-app-site-association` and `/.well-known/assetlinks.json` |

A malformed value fails the build: a scheme with `://`, a reserved scheme
(`https`, `mailto`, …), a domain with a path or port, a team ID that isn't 10
characters, or a fingerprint that isn't 32 hex bytes. Without that check the
app would build and the OS would quietly never send it links.

## 2. Verify the domains (universal links / App Links)

Before iOS, macOS or Android lets the app open a domain's https links, it
fetches a file from that domain to confirm the app is allowed to:

- `https://<domain>/.well-known/apple-app-site-association`, which names
  `<apple_team_id>.<bundle_id>`.
- `https://<domain>/.well-known/assetlinks.json`, which names the Android
  package (the bundle id) and `android_cert_fingerprints`.

`idealyst build web` writes both files into the bundle when their IDs are
declared, and warns when `domains` is set but an ID is missing. In that case
the links open the browser instead of the app. Serve the web build from
every listed domain, or copy the two files to whatever serves it, with these
rules:

- Serve the AASA file as `application/json` with no redirects.
  `idealyst serve` / `idealyst dev` already do this.
- Every listed domain needs the files, including each subdomain a
  `*.` entry covers.

Fingerprints: `keytool -list -v -keystore <keystore>` prints the SHA-256.
`idealyst run android` signs with `~/.android/debug.keystore`, so its
fingerprint is the one that verifies dev builds. A Play Store build is signed
with Play's app-signing key; add that fingerprint too.

## 3. How a URL maps to a screen

The navigators route on the URL's path:

| URL | routes to |
|---|---|
| `https://example.com/projects/42?tab=2` | `/projects/42?tab=2` |
| `myapp://projects/42` | `/projects/42` (the part after `://` is the path) |
| `myapp:///projects/42` | `/projects/42` |
| `myapp://` | `/` |

The fragment is dropped and nothing is percent-decoded, the same as web's
`location.pathname`. Routes match by prefix exactly as at cold start. A path
no screen fully matches resolves the way the same URL would at launch,
usually to the index screen.

## 4. Seeing, holding and replaying links

Use the `deep-link` SDK:

```rust
// See every link that arrives while the app runs (analytics, a banner).
let sub = deep_link::on_link(|link| log::info!("opened {}", link.route_path()));

// Hold links until sign-in, then open the held one.
let gate = deep_link::intercept(move |link| {
    if signed_in.peek() { return false; }   // signed in: let it route
    held.set(Some(link.clone()));
    true                                    // claimed: the navigators don't move
});
// …after sign-in:
if let Some(link) = held.peek() { deep_link::route_link(&link); }

// The URL that launched the app, if any.
let launch = deep_link::initial_link();
```

Both `on_link` and `intercept` return a guard; dropping it unregisters.
Neither one sees the launch link. That link opens its screen when the
navigators first mount, before app code could intercept it. An auth gate
that mounts its navigators only after sign-in (`if signed_in { … }`) still
opens the launch link: the launch path waits until the root navigator
mounts.

## Where each platform receives the link

The host passes the URL to `runtime_shared::inbound_link`. `launch` takes
the URL that started the app, before the first mount. `deliver` takes a link
that arrives while the app runs, and the host then flushes.

| Target | Launch link | While running |
|---|---|---|
| iOS | `launchOptions[.url]` / the universal-link user activity in `didFinishLaunching` → `ios_launch_url` | `application(_:open:options:)`, `application(_:continue:restorationHandler:)` → `ios_open_url` |
| Android | launch `Intent` data in `onCreate` → `setLaunchUrl` | `onNewIntent` → `deliverLink` (the Activity is `singleTask`, so a link reaches the running instance instead of starting a second one) |
| macOS | `application:openURLs:` before `applicationDidFinishLaunching:` | `application:openURLs:` |
| web | the page address | — (following a link to the site loads the page again) |

On macOS, AppKit delivers the launch URL from inside `NSApp.run()`, after
the app has mounted. A URL that arrives before `applicationDidFinishLaunching:`
is therefore treated as the launch link: it becomes `initial_link()` and
routes without reaching observers or interceptors, the same as on the other
platforms. Its navigation commits before the first frame is drawn.

Runtime-server mode (`idealyst dev` without `--local`) does not forward links.
The app renders a tree streamed from the dev server and has no local
navigators to route them through.

## Testing a link

- iOS simulator: `xcrun simctl openurl booted "myapp://projects/42"`. iOS
  asks "Open in …?" first. For universal links on a device, open the URL from
  Notes or Messages. Safari's address bar never opens the app for the site
  being browsed.
- Android: `adb shell am start -a android.intent.action.VIEW -d "myapp://projects/42"`.
  Check App Link verification with
  `adb shell pm get-app-links <bundle_id>`.
- macOS: `open "myapp://projects/42"`.
- Web: load the URL.
