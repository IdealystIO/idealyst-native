# login-demo

Full-stack auth with server functions, showing the **web BFF pattern** end
to end — the secure way to do browser auth without ever putting a secret in
JS.

## Run it

```sh
idealyst dev --web examples/login-demo
# open the printed URL — log in with  demo / password
```

`server_bin = "server"` in `Cargo.toml` tells `idealyst dev --web` to run this
crate's `server` bin (which serves both `/_srv/*` and the static bundle)
instead of the static-only dev server — without it the login POST would hit a
server with no API routes and get a **405**.

Equivalent manual steps:

```sh
idealyst build --web examples/login-demo            # produce pkg/
cargo run -p login-demo --bin server --features server
# open http://127.0.0.1:3000/
```

Try: log in, hit **Call protected me()**, then **reload the page** — you stay
logged in (the session cookie survived; nothing was kept in JS). Log out and
the protected call starts returning 401.

## What it demonstrates

1. **httpOnly session cookie (BFF).** `login` validates the credentials and
   calls `server::set_cookie(Cookie::new("session", id))`. The session id
   lives only in an httpOnly cookie — JS (and any XSS) can't read it. The
   browser sends it automatically on same-origin server-fn calls.
2. **Auth guard → `Auth<Principal>`.** An installed middleware
   (`install_auth_guard`) reads the cookie on every request and injects a
   `Principal`; the protected `me(user: Auth<Principal>)` reads it (401 if
   absent).
3. **CSRF defense, layered:**
   - the cookie is `SameSite=Lax`, so browsers won't attach it to a
     cross-site POST at all (the primary defense, automatic), and
   - `server::csrf_guard([...])` rejects requests from untrusted origins
     (defense-in-depth, installed in the server bin).
4. **Logout** drops the server session and clears the cookie.

## Files

- [src/lib.rs](src/lib.rs) — shared `Credentials`/`Principal`, the `#[server]`
  `login`/`me`/`logout`, the in-memory session store (`srv`), the auth guard,
  and the client `app()` (login form + buttons).
- [src/bin/server.rs](src/bin/server.rs) — installs the auth + CSRF guards,
  serves `/_srv/*` + the wasm bundle.

## Native (iOS / Android): bearer token in the OS keystore

The same app + same server also run on native, where there's no browser cookie
jar. There, `login` returns a **bearer token** (instead of setting a cookie),
the client stores it in the `credentials` Keychain / AndroidKeyStore, and a
credential provider attaches `Authorization: Bearer …` on every later call. One
`login` serves both: it branches on an `x-idealyst-client: native` header so
**web never receives the token in JS** and **native never relies on a cookie**.
The auth guard accepts the session from *either* the cookie or the bearer.

### Run it on Android (device-tests the AndroidKeyStore path)

```sh
# 1. Start the server on the dev machine (binds 127.0.0.1:3000):
cargo run -p login-demo --bin server --features server

# 2. Point the app at the server. Emulator → host loopback is 10.0.2.2
#    (the default in src/lib.rs). For a PHYSICAL device, edit
#    NATIVE_SERVER_URL to the dev machine's LAN IP, e.g. http://192.168.1.20:3000
#    and make sure the device is on the same network.

# 3. Build + run on the emulator/device:
idealyst dev --android --local examples/login-demo
```

Log in with `demo` / `password`. The token is written to the AndroidKeyStore
(AES-GCM, key in the TEE). Kill and relaunch the app, then hit **Call protected
me()** — it succeeds *without* re-login, because the bearer token survived in
the keystore (and the server session is still live). That round-trip is the
end-to-end test of the `credentials` Android backend.

> The Android `INTERNET` permission is injected automatically — `net` declares
> the `internet` capability, and the CLI walks the dependency graph and adds the
> `<uses-permission>` to the manifest. No hand-editing.

## Honest limits (it's a demo)

- The session id is a per-process counter, **not** a CSPRNG token — a real app
  uses random/signed session ids and a shared store (Redis/DB). Restarting the
  server drops all sessions.
- Same-origin only on web. A cross-origin API would also need
  `credentials:'include'` on the web fetch + `Access-Control-Allow-Credentials`
  CORS.
- The Android `credentials` backend is **device-verified by running this demo** —
  it isn't exercised by host tests (JNI signatures resolve at runtime). This
  demo is how you confirm it on a real device/emulator.
