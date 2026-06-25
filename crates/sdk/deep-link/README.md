# `deep-link`

Cross-platform **inbound-URL** handling — deep links (custom URL schemes
like `myapp://…`) and universal / app links (`https://example.com/…`). It
delivers the URL that cold-started the app and notifies you of every link
that arrives while it runs, parsed into a small `DeepLink`. You subscribe
with `on_link` and get an RAII guard that unsubscribes on drop.

It is deliberately a *raw inbound channel*. Mapping a URL onto a navigator
route is the app's (or a router SDK's) job — this crate hands you the parsed
URL and stops there.

```rust
use deep_link::{initial_link, on_link};

# fn demo() {
// The URL that cold-started the app, if any.
if let Some(link) = initial_link() {
    println!("launched via {}{}", link.scheme, link.path);
}

// Every inbound link while this guard is alive. Drop it to unsubscribe.
let sub = on_link(|link| {
    for (k, v) in link.query_pairs() {
        println!("{k} = {v}");
    }
});
# drop(sub);
# }
```

## What you get

A small parsed `DeepLink` plus a tiny subscription API:

- `DeepLink::parse(raw) -> Result<DeepLink, ParseError>` — parse any custom
  or web URL. Fields: `scheme` (lowercased), `host: Option<String>`, `path`,
  `query: Option<String>`, and `query_pairs() -> Vec<(String, String)>`
  (percent-decoded, order-preserving).
- `initial_link() -> Option<DeepLink>` — the **first** link ever fed; the
  cold-start URL. Set once, stable for the app's lifetime, so a handler
  registered after launch can still recover the launch URL.
- `on_link(handler) -> LinkSubscription` — fires for every inbound link
  while the returned guard is alive; dropping the guard unsubscribes. There
  is no `mem::forget` — the guard *is* the lifetime.
- `feed_link(raw_url)` — **host ingress**: the door the platform host calls
  when the OS hands it a URL. The first call seeds `initial_link`; every
  call parses and dispatches to all live handlers. This is the seam — the
  SDK owns parse + registry + dispatch; the host owns *calling* it.
- `seed_initial_from_platform()` — convenience the web bootstrap can call to
  seed `initial_link` from `window.location.href`; a no-op on native (the
  native host reads the launch URL / intent itself and calls `feed_link`).

Every target delivers the **same shape** — platforms diverge in *where*
`feed_link` is called from, not in the API you use.

## Per-platform mechanism

| Target | Where the host calls `feed_link` / seeds `initial_link` |
| --- | --- |
| web (wasm32) | `window.location.href` on bootstrap (→ `initial_link`); app-internal navigations / `popstate` → `feed_link`. **Runnable on web.** |
| iOS / macOS | `application(_:open:options:)` (custom scheme) + `application(_:continue:restorationHandler:)` (universal links) → `feed_link`; launch URL seeds `initial_link`. **Compile-checked only.** |
| Android | launch `Intent.getData()` in `onCreate` (→ `initial_link`) and `onNewIntent` (→ `feed_link`); `<intent-filter>` declares the scheme/host. **Compile-checked only.** |

The parse + registry + dispatch below `feed_link` is pure Rust and identical
on every target, so once a host calls `feed_link` the observable behavior is
the same everywhere. Wiring each backend host to *call* `feed_link` is the
framework's job, not the app's.

## Permissions

None at runtime — but inbound links require **build-time manifest
configuration**, not a runtime permission:

- **iOS / macOS** — declare a custom URL scheme under `CFBundleURLTypes`
  (custom-scheme links) and/or **Associated Domains** entitlement +
  `apple-app-site-association` (universal links).
- **Android** — an `<intent-filter>` on the launch Activity with
  `android:scheme` (and `android:host` for app links + Digital Asset Links).
- **web** — none; the entry URL is just `window.location`.

This is app build-config, so the CLI's per-platform capability injection
doesn't grant it the way it grants, say, a microphone permission. Injecting
`CFBundleURLTypes` / associated-domains / `<intent-filter>` entries from the
app's declared scheme/domain is a **separate build-tool seam** — not part of
this crate.

## Scope

Deliver the parsed inbound URL — the unopinionated raw capability. Mapping a
`DeepLink` onto a navigator route, validating it, or guarding it behind auth
is deliberately left to the app or a higher-level router SDK rather than
baked in here.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p deep-link` — parse, `query_pairs`, initial-link dedupe, subscription drop, reentrancy (the pure registry is fully unit-tested)
- [ ] `cargo build -p deep-link --features catalog` — recipes/docs compile
- [ ] `cargo build -p deep-link --target wasm32-unknown-unknown` — web target

**Behavior** (the host must call `feed_link` — these verify the host wiring, not just the registry)
- [ ] **Web** — bootstrap seeds `initial_link()` from `window.location.href`; an app-internal navigation / `popstate` fed via `feed_link` fires `on_link` with the parsed URL.
- [ ] **iOS** — open a `myapp://…` URL (and a universal `https://…` link) — `on_link` fires with the parsed link; a cold-start launch URL appears in `initial_link()`. Confirm the host forwards from `application(_:open:options:)` / `application(_:continue:…)`.
- [ ] **macOS** — same custom-scheme + universal-link flow forwarded from the AppKit delegate.
- [ ] **Android** — launching via an `<intent-filter>` URL surfaces the URL in `initial_link()` (from `onCreate`'s `Intent.getData()`); a warm `onNewIntent` fires `on_link`. Confirm the host forwards both into `feed_link`.
