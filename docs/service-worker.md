# The service worker on web

A browser lets **one** service worker control a scope (normally the whole site). A notification click, a push subscription and a background-sync registration all belong to the worker that created them. So SDKs that need a service worker can't each register their own: they would replace each other. Instead, every package declares the handler scripts it contributes, and the web build composes them into one worker, `idealyst-sw.js`, served at the bundle root.

## Contributing handlers

An SDK, or the app crate itself, lists its handler files in its `Cargo.toml`:

```toml
[package.metadata.idealyst.web]
service_worker = ["sw/notifications.js"]
```

- **Paths** resolve against the declaring package's directory and must stay inside it (no absolute paths, no `..`).
- **Which packages count:** only packages the app actually depends on contribute. The build walks the app's normal dependencies (transitively, filtered to `wasm32`) in `cargo metadata`'s resolve graph. A workspace sibling or a dev-dependency never ships handlers into the app.
- **Order:** the framework prelude comes first, then SDK contributions in package-name order, then the app's own.
- **Isolation:** each file runs in its own function scope, labelled `// --- <package> : <path> ---`, so a stack trace or syntax error points at the package that shipped it.
- **Language:** handlers are plain JavaScript. A worker has no DOM, and loading the app's wasm to answer a single event would cost megabytes on every wake-up. Register listeners at the top level of the file (`self.addEventListener("push", …)`): browsers only dispatch to listeners added while the worker script first runs.
- **Errors:** a declared file that can't be read fails the build, naming the package.

The prelude creates `self.idealyst`, the namespace contributions share. It also has a new version take over at once (`skipWaiting` + `clients.claim`): contributions are event handlers with no cache an open page depends on, so a fixed handler shouldn't wait for every tab to close.

## Registration

When anything contributes, the build writes `idealyst-sw.js` and sets `window.IDEALYST_SERVICE_WORKER = "idealyst-sw.js"` in the page's `<head>`. `idealyst::boot::web` then registers it with `web_glue::service_worker::register` on both boot paths. A failed registration is logged to the console, never fatal.

- **URL:** relative, so an app served under a sub-path registers the worker beside its own `index.html`, and the worker's scope is that path.
- **Updates:** registered with `updateViaCache: "none"`, so the browser re-fetches the script for every update check instead of answering from the HTTP cache. A deploy that changes a handler takes effect on the next check.
- **Fingerprinting:** the file name is not fingerprinted. The browser detects an update by re-fetching the same URL and comparing bytes, so the URL must stay stable.
- **Secure context:** service workers need HTTPS or `localhost`. Elsewhere registration fails with the logged error and the app runs without one.
- **No contributions:** no worker is written. A rebuild after the last contributing SDK is removed deletes the stale file.

### An app with its own service worker

An app that already ships a worker (offline caching, say) can't have a second one at the same scope. It imports the composed handlers into its own worker instead:

```toml
[package.metadata.idealyst.app.web]
service_worker_registration = "import"   # default: "auto"
```

```js
// the app's own worker
importScripts("idealyst-sw.js");
```

With `"import"` the build still writes `idealyst-sw.js`, but the page doesn't register it; the app registers its own worker as it always has.

## Dev and deploy

| | Where `idealyst-sw.js` comes from | How the page is marked |
| --- | --- | --- |
| `idealyst build --web` | written into the staged bundle | the staged `index.html` |
| `idealyst dev --web --local` | written into the dev overlay (`target/idealyst/dev/web/`) and served at `/idealyst-sw.js` | injected into every served page |

In dev, editing a contributed file (or the app's `Cargo.toml`) recomposes the worker and reloads the page.
