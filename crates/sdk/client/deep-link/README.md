# `deep-link`

Inbound links in app code: custom-scheme deep links (`myapp://items/42`)
and universal links / App Links (`https://example.com/items/42`).

**The framework already routes links.** The link that launches the app
opens its screen on the first mount. A link that arrives while the app runs
moves the live navigators to the same screen. This crate is for app code
that wants to see links, hold them (an auth gate) or replay them, plus the
web address-bar calls below. The full guide, including the build
configuration that makes the OS send links at all, is
[`docs/deep-links.md`](../../../../docs/deep-links.md).

```rust
use deep_link::{initial_link, intercept, on_link, route_link};

// The URL that launched the app, if any.
if let Some(link) = initial_link() {
    log::info!("launched via {}", link.scheme);
}

// Every link that arrives while this guard is alive. Drop it to unsubscribe.
let _sub = on_link(|link| log::info!("opened {}", link.route_path()));
```

## What you get

- `DeepLink::parse(raw) -> Result<DeepLink, ParseError>` parses any custom
  or web URL. Fields: `scheme` (lowercased), `host: Option<String>`, `path`,
  `query: Option<String>`. `query_pairs()` returns the query
  percent-decoded and in order. `route_path()` is the app path the
  framework routes the link to.
- `initial_link() -> Option<DeepLink>` is the URL that launched the app.
  The host records it before the first mount and it never changes.
- `on_link(handler) -> LinkSubscription` observes every link that arrives
  while the app runs. Observing doesn't change routing.
- `intercept(handler) -> LinkSubscription` is offered every link before it
  routes. Returning `true` claims the link and the navigators don't move.
- `route_link(&link) -> bool` routes a link the way the framework does,
  which is how a claimed link is replayed later. Returns whether it landed.
- `feed_link(raw) -> bool` delivers a raw URL as if the OS had. The hosts
  call this themselves; it is public for custom hosts and tests.

Dropping a `LinkSubscription` unregisters it. Handlers run on the UI thread
in registration order: observers first, then interceptors, then routing.

### Holding links until sign-in

```rust
let held = signal::<Option<DeepLink>>(None);
let _gate = deep_link::intercept(move |link| {
    if signed_in.peek() { return false; }   // let it route
    held.set(Some(link.clone()));
    true                                    // claimed
});
// …after sign-in:
if let Some(link) = held.peek() { deep_link::route_link(&link); }
```

`on_link` and `intercept` never see the launch link, because it opens its
screen as the navigators first mount, before app code could step in. A gate
that mounts its navigators only after sign-in still opens the launch link:
the launch path waits until the root navigator mounts.

## The live address

`initial_link()` is fixed at launch. Where the app is **now** is a
separate question, answered by three calls:

- `current_url() -> Option<DeepLink>` — the address the app is at at the
  moment of the call (`window.location.href`), following every navigator
  write, every `replace_url` and the browser's Back/Forward. Use it for
  decisions that must be made before a navigator exists (a public share
  path that skips the auth gate) and for reading query-string state.
- `replace_url(url)` — rewrite the address **without navigating**
  (`history.replaceState`). No re-render, no new history entry, no
  `on_link` dispatch; the entry's existing `history.state` is kept so a
  navigator's bookkeeping survives. For making the address bar say where
  the screen already is — screen changes belong to the navigators.
- `origin() -> Option<String>` — `scheme://host[:port]`, for building
  absolute links to hand to someone else (a share URL, a second tab).

```rust
// A client-share link: the server knows the path, the page knows the origin.
let link = match deep_link::origin() {
    Some(origin) => format!("{origin}{path}"),
    None => path.to_string(),
};

// Keep a filter in the query string without touching history.
if let Some(here) = deep_link::current_url() {
    deep_link::replace_url(&format!("{}?date=2026-09-30", here.path));
}
```

| Target | `current_url` | `origin` | `replace_url` |
| --- | --- | --- | --- |
| web (wasm32) | `location.href`, parsed | `location.origin` (`None` for an opaque `"null"` origin) | `history.replaceState(state, "", url)` |
| iOS / macOS / Android / desktop | `None` | `None` | no-op |

The native answers are the real answer, not a stub: a native app is not
"at" a URL (its navigators hold the in-memory path) and is not served
from an origin anything could be relative to.

## Where links come from

The OS hands the host the URL, and the host passes it to the framework's
ingress, `runtime_shared::inbound_link`. This crate is the typed layer on
top of that ingress. Hosts don't depend on an SDK, and the launch link
arrives before any app code runs.

| Target | Launch link | While running |
| --- | --- | --- |
| iOS | `didFinishLaunching` launch options / user activity | `application(_:open:options:)`, `application(_:continue:restorationHandler:)` |
| Android | the launch `Intent` in `onCreate` | `onNewIntent` (`singleTask` Activity) |
| macOS | `application:openURLs:` before `applicationDidFinishLaunching:` | `application:openURLs:` |
| web | the page address | — |

## Configuration

There's no runtime permission. Declare the links under
`[package.metadata.idealyst.app.links]` (`schemes`, `domains`,
`apple_team_id`, `android_cert_fingerprints`). The build then writes the
Info.plist URL types, the associated-domains entitlement, the Android intent
filters and the web build's `.well-known` verification files. See
[`docs/deep-links.md`](../../../../docs/deep-links.md).

## Testing checklist

**Automated**
- [x] `cargo test -p deep-link`: parsing, `query_pairs`, `route_path`,
  the launch link, observe / intercept / route, unsubscribe on drop,
  re-entrancy.
- [x] `cargo test -p runtime-shared --lib inbound_link`: the ingress, the
  observe → intercept → route order, and the URL-to-path mapping.
- [x] `cargo test -p runtime-vocabulary --test inbound_links`: links that
  arrive while running move swap and stack navigators, nested navigators
  resolve from the link, and a query-only change updates the screen.
- [x] `cargo test -p deep-link --target wasm32-unknown-unknown`
  (`tests/location_web.rs`): `replace_url`, `current_url`, `origin`.

**On a device or simulator**
- [x] iOS simulator: a link delivered while running
  (`application(_:open:options:)`) opens Settings → About in
  `examples/nav-showcase`.
- [x] macOS: `open "navshowcase://settings/about"` opens About, both while
  running and when it launches the app (`examples/nav-showcase`).
- [ ] iOS: a cold-start custom-scheme link (`simctl openurl` asks "Open
  in …?" first), and universal links on a device.
- [ ] Android: `adb shell am start -a android.intent.action.VIEW -d "<url>"`,
  cold and warm.
