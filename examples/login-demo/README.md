# login-demo

A small full-stack app — login, a protected call, logout — built on idealyst
server functions. It's here to teach the **concepts of doing authentication
securely** across web and native, not just to show the API. Read it top to
bottom and you'll understand where the secret lives on each platform and why.

Log in with `demo` / `password`.

---

## The concepts

### The question every auth system answers: where does the secret live?

After you log in, the client holds *some* proof it's you — a session id or a
token — and sends it on each later request. The entire security of the system
comes down to one question: **where is that proof stored, and who can read
it?** Get this wrong and an attacker reads the secret and becomes you. This
demo's whole job is to store that proof in the safest place each platform
offers, and to never store it somewhere it can leak.

### Server functions: one function, two compilations

`login`, `me`, and `logout` are `#[server]` functions. The macro compiles each
one **twice**:

- On the **server**, it's the real function with its body, registered to run at
  `POST /_srv/<name>`.
- On the **client**, the body is replaced by a stub that serializes the
  arguments, does the HTTP call, and deserializes the result.

So `me().await` in the UI is a type-checked call that happens to cross the
network. The body — which touches the session store and `set_cookie` — only
ever exists server-side; the browser never sees it.

### Web auth: the httpOnly session cookie (the BFF pattern)

In a browser, **any JavaScript on your origin can read anything your code can
read.** So if you keep a token in `localStorage` or a JS variable, a single XSS
(a malicious script injected into your page) steals it. There is no way to
"encrypt around" this — the attacker runs in the same context as you.

The answer is to keep the secret **out of JavaScript entirely**. On login, the
server sends:

```
Set-Cookie: session=<id>; HttpOnly; Secure; SameSite=Lax
```

`HttpOnly` means JavaScript **cannot read this cookie** — `document.cookie`
won't show it, and neither will an XSS. The browser still attaches it
automatically to every same-origin request, so the client proves who it is
without ever holding the secret. The server is acting as a *Backend For
Frontend* (BFF): it holds the real session, and the browser only carries an
opaque, unreadable handle to it.

In the code, that's one line in `login`:

```rust
server::set_cookie(server::Cookie::new("session", session_id));
```

### Knowing who's calling: sessions, the guard, and `Auth<Principal>`

The cookie only carries a session **id**. The server maps that id back to a
user. Two pieces do this:

- A **session store** (`srv`): `id → username`. Created at login, looked up on
  each request, removed at logout.
- An **auth-guard middleware** (`install_auth_guard`): runs before every
  handler, reads the session id, looks up the user, and injects a `Principal`
  into the request context.

A protected function then just asks for the user as a parameter:

```rust
pub async fn me(user: server::Auth<Principal>) -> Result<String, ServerError> {
    Ok(user.username.clone())
}
```

`Auth<Principal>` resolves from whatever the guard injected. If no valid session
was present, it resolves to a 401 and the body never runs. Authentication
becomes a parameter, not boilerplate inside each function.

### CSRF: why cookie auth needs a guard, and the layered defense

Cookies have a sharp edge: the browser attaches them **automatically**, even on
requests started by *another* site. So `evil.com` can submit a form to your API
and the browser will helpfully include your session cookie — a *Cross-Site
Request Forgery*. The demo defends in two layers:

1. **`SameSite=Lax` on the cookie** (the primary, automatic defense). The
   browser refuses to attach the cookie to a cross-site POST at all. Server
   functions are POSTs, so a forged cross-site call arrives with no cookie and
   is simply unauthenticated.
2. **`server::csrf_guard([trusted origins])`** (defense-in-depth). It rejects
   any request whose `Origin` header isn't one you listed, covering older
   browsers that don't honor `SameSite` and making the policy explicit. Native
   clients send no `Origin` and use a bearer token instead, so they pass
   through untouched.

### Native auth: a bearer token in the OS keystore

On iOS/Android there's no browser, so no cookie jar — and, importantly, there
*is* a real secure store the browser never had: the **OS keychain / keystore**,
hardware-backed on most devices. So native flips the model:

- `login` returns a **bearer token** in the response body instead of setting a
  cookie.
- The client writes it to the keystore via the `credentials` SDK
  (`creds.set("session", token)`), where the OS protects it at rest and only
  this app can read it.
- A credential provider attaches `Authorization: Bearer <token>` to every later
  request, reading it back from the keystore each time.

One `login` serves both platforms. It branches on a header the native client
sends (`x-idealyst-client: native`): native gets the token, web gets the cookie
and **never receives the token in JS**. The auth guard accepts the session from
*either* the cookie or the bearer, so the rest of the server is identical.

### The unifying idea

There is one session, stored once on the server. The client only ever holds a
*handle* to it, kept in the safest unreadable place the platform has — an
httpOnly cookie in the browser, the hardware keystore on a device. The secret
never lives anywhere a script or another app can read it. That's the whole
game.

---

## Run it (web)

```sh
idealyst dev --web examples/login-demo
# open the printed URL, log in with  demo / password
```

`server_bin = "server"` in `Cargo.toml` makes `idealyst dev --web` run this
crate's server bin (which serves both `/_srv/*` and the static bundle) rather
than the static-only dev server.

Try this to *see* the concepts:

- Log in, hit **Call protected me()** → it returns your username.
- **Reload the page** → you're still logged in, even though nothing was saved in
  JS. That's the httpOnly cookie doing its job.
- **Log out** → `me()` now returns a 401. The server session is gone.
- Open DevTools → Application → Cookies: the `session` cookie shows `HttpOnly`,
  and `document.cookie` in the console won't print it.

## Run it on Android (device-tests the keystore path)

```sh
# 1. Start the server on the dev machine:
cargo run -p login-demo --bin server --features server

# 2. The app points at 10.0.2.2:3000 (the Android emulator's alias for the
#    host's localhost). For a physical device, set NATIVE_SERVER_URL in
#    src/lib.rs to the dev machine's LAN IP and join the same network.

# 3. Build + run:
idealyst dev --android --local examples/login-demo
```

Log in, then **kill and relaunch the app** and hit **Call protected me()** — it
succeeds without re-login, because the bearer token survived in the
AndroidKeyStore (AES-GCM, key held in the TEE). That round-trip is the live test
of the `credentials` Android backend. The `INTERNET` permission is injected
automatically — `net` declares an `internet` capability and the CLI adds the
`<uses-permission>` to the manifest.

## Code map

- [src/lib.rs](src/lib.rs) — the `#[server]` `login`/`me`/`logout`, the session
  store + auth guard, and the client `app()` (login form + buttons + the
  web/native auth wiring).
- [src/bin/server.rs](src/bin/server.rs) — installs the auth guard + CSRF guard
  and serves `/_srv/*` plus the static bundle.

## Honest limits (it's a demo)

- The session id is a per-process counter, **not** a CSPRNG token — a real app
  uses a random or signed session id and a shared store (Redis/DB). Restarting
  the server drops all sessions, so you'll need to log in again.
- Web is same-origin only. A cross-origin API would also need
  `credentials: 'include'` on the fetch and `Access-Control-Allow-Credentials`
  CORS on the server.
- httpOnly stops an XSS from *reading* the session, but a script on your page
  can still *make* requests while it's open. Short sessions, CSRF protection,
  and `SameSite` are all part of the wall — no single mechanism is the whole
  defense.
