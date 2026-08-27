# Server functions

Write your backend as ordinary `async fn`s next to your app code. The same
function compiles two ways: the **server build** keeps the body and registers
an HTTP handler for it; the **client build** (web, iOS, Android, macOS,
terminal — every UI target) replaces the body with a typed RPC stub that
POSTs the arguments and decodes the result. The call site is identical on
both sides.

```rust
use server::{server, ServerError};

#[server]
async fn add(a: i32, b: i32) -> Result<i32, ServerError> {
    Ok(a + b)
}

// Anywhere in the app — same line of code on client and server:
let sum = add(2, 3).await?;
```

The system lives in [`crates/api/server`](../crates/api/server) (the
primitive: runtime SDK + the `DispatchHook` policy seam),
[`crates/api/server-macros`](../crates/api/server-macros) (the attribute
macros), and [`crates/api/server-kit`](../crates/api/server-kit) (the
conventional policy layer — middleware chain, `Auth`, guards — built on the
seam; see §5). The architecture rationale is
[`crates/api/server/DESIGN.md`](../crates/api/server/DESIGN.md); this document
is the author-facing guide.

Working examples:

- [`crates/api/server/examples/server-fn-demo`](../crates/api/server/examples/server-fn-demo)
  — full stack: wasm UI + API from one process, plus `#[sse]` streaming.
- [`examples/login-demo`](../examples/login-demo) — auth end to end: httpOnly
  session cookie (web BFF), bearer token in the OS keystore (native),
  auth-guard middleware, CSRF defense.

---

## 1. The split: one function, two compilations

`#[server]` expands an `async fn` into **three items**:

```rust
// 1. Server half — the original body, untouched.
#[cfg(feature = "server")]
pub async fn add(a: i32, b: i32) -> Result<i32, ServerError> { Ok(a + b) }

// 2. Server registration — an inventory entry the router collects at boot.
#[cfg(feature = "server")]
mod __server_fn_add { /* decode args → call add → encode result */ }

// 3. Client stub — same name, wire-args-only signature, body = one RPC call.
#[cfg(not(feature = "server"))]
pub async fn add(a: i32, b: i32) -> Result<i32, ServerError> {
    server::__private::call("add", SCHEMA_HASH, &(a, b)).await
}
```

The cfg key is a Cargo feature literally named `server` **on the crate that
contains the `#[server]` fn**. The server binary builds with
`--features server`; every client target builds with default features.

### The body never reaches the client

`#[cfg]`-stripped items are removed *before name resolution*. A server fn
body can therefore reference server-only crates — a db pool, `tokio`,
filesystem APIs — and the client build will neither compile nor ship any of
it, **as long as those references live inside the body**:

```rust
#[server]
async fn lookup(id: u64) -> Result<Record, ServerError> {
    use my_db::Query;                    // body-local: stripped with the body
    my_db::connect().find(Query::by_id(id)).await
        .map_err(|e| ServerError::failed(e.to_string()))
}
```

This is the rule that keeps colocated code cfg-free: **imports used only by
server bodies belong inside the bodies** (or fully qualified). A module-level
`use my_db::…` must resolve on *both* builds and is what forces hand-written
gates.

### What still needs a gate, and why

The macro can only see the item it is attached to. Three things sit outside
its reach:

| What | Why | Fix |
|---|---|---|
| Server-only helper fns / modules (session stores, db setup, `install_*` calls) | module-level items, invisible to the macro | wrap in `#[cfg(feature = "server")]` (see login-demo's `srv` module) — or put them in a server-only crate |
| Client-only code calling the stubs in the same crate | on the server build the real fn has the *extractor-bearing* signature, so a stub-shaped call (`me()` vs `me(user: Auth<…>)`) won't compile | gate the UI with `#[cfg(not(feature = "server"))]`, or use the layered crate shape (§2) so the UI crate never builds with the feature |
| Server-only dependencies in `Cargo.toml` | a proc macro cannot declare features or mark deps optional | `optional = true` + list them in the `server` feature (§2) |

The layered project shape makes the first two disappear; the third is
one-time-per-crate plumbing.

---

## 2. Project shapes

### Layered (recommended)

```
api/         #[server] fns + shared DTOs + the domain error enum.
             Deps: server, serde, and (behind its `server` feature) db/tokio/…
             No UI deps → a body *cannot* name Signal/Element — compile error.
ui/          The app. Depends on api with default features → sees stubs only.
             Zero cfg(feature = "server") anywhere in this crate.
server-bin/  Depends on api with features = ["server"]; calls server::router().
```

Besides killing the cfg noise, layering is the **enforcement boundary**
(DESIGN.md §0): the api crate doesn't link the UI runtime, so server bodies
can't touch client state even by accident.

### Colocated (single crate)

Supported — the demos use it for compactness — but you opt into the gates
described in §1. The Cargo.toml plumbing is the same either way:

```toml
[features]
default = []
# Forward `server-kit/server` too if you use the kit's middleware/guards.
server = ["server/server", "server-kit/server", "dep:axum", "dep:tokio", "dep:tower-http"]

[dependencies]
server = { workspace = true }
axum = { version = "0.7", optional = true }
tokio = { version = "1", features = ["macros", "rt-multi-thread"], optional = true }
tower-http = { version = "0.6", features = ["fs"], optional = true }

[[bin]]
name = "server"
path = "src/bin/server.rs"
required-features = ["server"]
```

### Rules of intent

1. **Server functions are top-level free functions.** The macro rejects a
   `self` receiver, and they should not be *defined* inside components —
   define them in an `api` module or crate and **reference** them from
   components.
2. **Server-only support code is separated** — a gated module in colocated
   crates, or simply "the api crate" in the layered shape.
3. **Imports used only by server bodies live inside the bodies.**

### CLI integration

Declare the server bin in the app metadata and `idealyst dev` runs the whole
stack:

```toml
[package.metadata.idealyst.app]
server_bin = "server"    # → cargo run --bin server --features server
```

- `idealyst dev --web` runs your server bin instead of the static-only dev
  server, so `/_srv/*` works during development.
- For native targets the CLI exports `IDEALYST_SERVER_URL` to the app
  process; read it with `server::dev_base_url()`:

```rust
let base = server::dev_base_url()
    .unwrap_or_else(|| "http://127.0.0.1:3000".into());
server::configure(server::ClientConfig::new(base));
```

#### What a save actually rebuilds

The dev loop runs two independent watchers and they do different things:

| what changed | what happens | is the port down? |
|---|---|---|
| client bundle only | bundle restaged into `dist/web`, "refresh the browser" | no |
| server sources | server rebuilt, **then** the process is restarted | only for the rebind |
| a crate they share | both — rebuild, restage, restart | only for the rebind |

Two properties are load-bearing:

- **A bundle-only rebuild never restarts the server.** Both server shapes
  serve `dist/web` through a runtime `ServeDir` — the in-crate shape bakes
  only the *path* (`env!("CARGO_MANIFEST_DIR")`), never the contents — so
  the running process already serves the new files.
- **The server is rebuilt before it is killed, never after.** A save that
  doesn't compile leaves the running server up and costs a log line. The
  restart itself only happens once a build has succeeded *and* actually
  relinked the binary, so the downtime is a rebind rather than a compile.

The dev server also builds into its own target directory rather than the
workspace's `target/`. Cargo locks a build directory exclusively for the
duration of a build, so sharing it lets any concurrent `cargo check` in the
tree — rust-analyzer, CI, a second person or agent — block the server's
rebuild and hold the port down with it.

---

## 3. Parameters: wire args vs. injected extractors

Every parameter is classified one of two ways:

- **Wire arg** — serialized into the request body, present in the client
  stub's signature. The default.
- **Injected extractor** — resolved server-side from the request `Context`,
  and *dropped* from the client stub.

A parameter is an extractor if it is annotated `#[ctx]` **or** its type names
one of the reserved wrappers `State` / `Headers` / `Extension` / `Auth` /
`Cookies` (matched on the last path segment, so `server::State<T>` counts).

```rust
#[server]
async fn create_todo(
    input: NewTodo,          // wire arg
    db: State<Db>,           // injected — absent from the client stub
    user: Auth<Principal>,   // injected by the auth guard (401 when missing)
) -> Result<Todo, ServerError<TodoError>> {
    db.insert(user.id, input).await     // wrappers Deref to their inner T
}

// Client-side, the stub is exactly:
//   async fn create_todo(input: NewTodo) -> Result<Todo, ServerError<TodoError>>
```

Built-in extractors:

| Type | Resolves from | On failure |
|---|---|---|
| `State<T>` | process-wide state registry (`server::install_state(t)` at boot) | 500 |
| `Headers` | the request's `HeaderMap` | — |
| `Extension<T>` | `ctx.get::<T>()`, set by middleware | 500 |
| `Auth<T>` (from `server-kit`) | like `Extension<T>`, set by an auth guard | **401** |
| `Role<M>` (from `server-kit`) | a capability marker set by an auth guard | **401** anonymous / **403** signed-in without it |
| `Cookies` | parsed request cookies (`cookies.0` is the map) | — |

`FromContext` is an open trait — implement it on your own type and annotate
the param `#[ctx]` to inject it. For the common "app resource as a named
extractor" case (a db pool, a mail client), `server_kit::context!` writes
the wrapper for you:

```rust
server_kit::context! {
    /// The app's database pool.
    pub struct Db(sqlx::PgPool);
}

// boot: server::install_state(pool);
#[server]
async fn list_todos(filter: Filter, #[ctx] db: Db) -> Result<Vec<Todo>, ServerError> {
    db.query("…").fetch_all().await          // Db derefs to the pool
}
```

The fn names its dependency as a domain type instead of `State<PgPool>`,
and a missing resource fails with a clear 500 naming the type and the fix.

Note: a type used inside a wrapper (`Principal` above) appears in the
*shared* signature, so it must be defined on both builds — the client names
it but never constructs one. Keep such types plain data.

Ambient alternatives, callable anywhere inside a handler's request scope:
`server::use_state::<T>()`, `server::use_request_header(name)`,
`server::use_request_headers()`.

---

## 4. Errors

The unified error is `ServerError<E>`
([`error.rs`](../crates/api/server/src/error.rs)), generic over your domain
error and defaulting to `String`:

```rust
pub enum ServerError<E = String> {
    Failed(E),                // your body returned Err(e) — typed, over the wire
    Network(String),          // DNS/TCP/TLS/timeout (client-side only)
    Codec(String),            // same-version (de)serialization bug
    Server { status: u16, message: String },   // dispatcher-level rejection
    Cancelled,                // aborted via a cancel token
    IncompatibleVersion { .. } // schema drift — "your app is outdated" (§8)
}
```

Stringly errors: `Err(ServerError::failed("invalid password"))`. Typed
errors: define the enum in the shared crate and spell the return type —
nothing else changes:

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum TodoError { NotFound, Forbidden }

#[server]
async fn get_todo(id: u64) -> Result<Todo, ServerError<TodoError>> {
    find(id).await.ok_or(ServerError::Failed(TodoError::NotFound))
}
```

Transport failures fold into the same return type via the `ServerFnReturn`
trait, so the client always matches on one error enum — `Failed(domain)` vs.
the infrastructure variants.

---

## 5. Policy: the dispatch hook, and server-kit

The primitive holds **no policy**. Its single interception surface is the
[`DispatchHook`](../crates/api/server/src/hook.rs) slot — one hook, installed
once at boot, that wraps every entry point into author code:

- `around(ctx, next)` wraps each **unary** invocation — the single
  `POST /_srv/<path>` dispatch and *each entry* of a batched request. Mutate
  `ctx` (insert principals/extensions), call `next.run(ctx)` to proceed, or
  return `Err(TransportError::Server { status, .. })` without running `next`
  to short-circuit with that status.
- `on_open(ctx)` gates each **stream** before it is accepted (`#[channel]` /
  `#[subscription]` upgrades, `#[sse]` requests).

The slot is deliberately singular — a list with ordering semantics *is* a
middleware system, which is exactly the opinion the primitive refuses to
hold. Composition belongs to the layer above; two claimants panic loudly at
boot instead of silently ordering themselves. With no hook installed,
handlers run directly.

```rust
struct ApiKeyGate;
impl server::DispatchHook for ApiKeyGate {
    fn around<'a>(&'a self, ctx: &'a mut server::Context, next: server::Next)
        -> server::HookFuture<'a>
    {
        Box::pin(async move {
            match verify(ctx.headers().get("x-api-key")) {
                Some(caller) => { ctx.insert(caller); next.run(ctx).await }
                None => Err(server::TransportError::Server {
                    status: 401, message: "bad api key".into(),
                }),
            }
        })
    }
}
server::install_dispatch_hook(ApiKeyGate);   // once, at boot
```

### server-kit: the conventional layer

Most apps don't write a hook — they use
[`server-kit`](../crates/api/server-kit/), the conventional policy crate
built entirely on that seam (proof the seam is sufficient: the kit uses only
public `server` APIs). It provides the ordered **middleware chain**, and its
first `install_middleware` claims the hook slot for the chain:

```rust
server_kit::install_middleware(server_kit::from_fn(|ctx| {
    let session = ctx.headers().get("cookie") /* … extract session id … */;
    Box::pin(async move {
        if let Some(user) = lookup(session) {
            ctx.insert(Principal { username: user });   // Auth<Principal> now resolves
        }
        Ok(())        // Err(e) short-circuits with e's HTTP status
    })
}));
```

Middleware runs in registration order before every handler — single calls,
each batch entry, and stream opens. Two roles fall out of one trait:
**context-producing** (the auth guard above) and **short-circuiting** (rate
limits, `server_kit::csrf_guard`). The kit also owns `Auth<T>` — the
401-on-missing extractor — the `require::<T>(prefix)` perimeter guard, its
declarative sibling `require_tag::<T>(tag)`, and the `context!` resource
macro (§3).

### Endpoint tags

Every `#[server]` / `#[channel]` / `#[subscription]` / `#[sse]` accepts
`tags(...)` — static metadata carried on the registration entry and copied
onto the request `Context`, readable by any hook or middleware via
`ctx.has_tag(name)` / `ctx.tag(name)` / `ctx.route_tags()`:

```rust
#[server(tags(admin))]                     // bare tag → ("admin", "")
async fn rotate_keys(admin: Auth<Admin>) -> Result<(), ServerError> { … }

#[server(tags(limit = "30/min"))]          // valued tag → ("limit", "30/min")
async fn send_message(msg: NewMessage) -> Result<(), ServerError> { … }
```

The primitive attaches **no meaning** to any tag — it just carries the
endpoint's self-description to the policy layer. That's what makes guards
declarative: `require_tag::<Admin>("admin")` protects everything that says
it's admin, with no path list to maintain, and a renamed route can't
silently fall out of the guarded set.

Because the slot is singular, "my custom `DispatchHook` + the kit's chain"
panics at boot: pick one policy owner. A custom hook that wants the kit's
guards can call `server_kit::run_chain(ctx)` inside itself instead.

### Protecting a set of routes with a guard

Two mechanisms compose; use both for defense in depth.

**1. The extractor defines the protected set (opt-in per fn).** The guard
middleware only *validates and inserts* — it never rejects, so public fns
keep working for anonymous requests. Any fn that declares `Auth<T>` is
protected automatically: when no guard inserted a `T`, extraction fails with
**401 before the body runs**. To protect a *privileged* subset, insert a
marker type only for qualifying sessions — the type system becomes the ACL:

```rust
use server_kit::Auth;

// Shared types — plain data, defined on both builds (they appear in
// extractor positions of shared signatures; the client names them but
// never constructs one).
#[derive(Clone)]
pub struct Principal { pub username: String }
#[derive(Clone)]
pub struct Admin(pub Principal);   // present in ctx only for admin sessions

// Server boot — the session guard: validate, insert, NEVER reject.
server_kit::install_middleware(server_kit::from_fn(|ctx| {
    // Read headers synchronously, then move owned data into the future.
    let session = ctx.headers().get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(session_from_cookie);          // or the bearer header
    Box::pin(async move {
        if let Some(user) = lookup(session).await {
            if user.is_admin {
                ctx.insert(Admin(Principal { username: user.name.clone() }));
            }
            ctx.insert(Principal { username: user.name });
        }
        Ok(())    // anonymous requests continue — public fns must still work
    })
}));
```

```rust
#[server]                       // public: no Auth param, no protection
async fn list_public() -> Result<Vec<Post>, ServerError> { … }

#[server]                       // any signed-in user; 401 otherwise
async fn my_profile(user: Auth<Principal>) -> Result<Profile, ServerError> { … }

#[server]                       // admins only; non-admins get 401
async fn delete_user(id: u64, admin: Auth<Admin>) -> Result<(), ServerError> { … }
```

**2. A path perimeter makes a whole group deny-by-default.** The extractor
pattern is opt-in — a fn whose author forgets the `Auth` param is silently
public. For a route *set* that must never be reachable without privilege,
give the group a shared wire-path prefix and enforce it centrally: the guard
reads `ctx.path()` (the matched wire path) and short-circuits.

```rust
#[server(path = "admin/delete_user")]
async fn delete_user(id: u64, admin: Auth<Admin>) -> Result<(), ServerError> { … }

#[server(path = "admin/rotate_keys")]
async fn rotate_keys(admin: Auth<Admin>) -> Result<(), ServerError> { … }

// Server boot, AFTER the session guard (registration order = run order).
// 403: authenticated-or-not, this caller may not enter the group.
// (Missing Auth<T> yields 401 — unauthenticated — by contrast.)
server_kit::install_middleware(server_kit::require::<Admin>("admin/"));
```

Now the perimeter rejects *every* `admin/*` call from a non-admin — including
a future fn someone adds to the group without an `Auth<Admin>` param — and
the extractor still gives each body typed access to the principal.

Notes that make this robust:

- **Order matters.** Middleware runs in registration order; install the
  context-*producing* session guard before any context-*checking* perimeter.
- **Coverage is total.** The same chain runs for single calls, for each
  entry of a batched request, and at `#[channel]` / `#[subscription]` /
  `#[sse]` upgrade time — one guard protects every transport.
- **Client-side surface.** A short-circuit arrives as
  `ServerError::Server { status: 401 | 403, .. }` — infrastructure, distinct
  from the body's domain `Failed`, so the app can route it to a login screen
  generically.
- **Reusable guards.** `require` and `csrf_guard` are values returning
  `impl Middleware` — package your own parameterized guards the same way:

  ```rust
  server_kit::install_middleware(server_kit::require::<Admin>("admin/"));
  server_kit::install_middleware(server_kit::require::<Principal>("account/"));
  ```

Prefer `require_tag` + `tags(admin)` over the path-prefix form when the
group is a *capability* — membership then lives on each endpoint's own
declaration instead of a naming convention. The prefix form remains useful
when the URL structure itself is the contract (e.g. everything under
`internal/` is VPN-only at the load balancer too).

### Roles: capabilities as facts

The full role model rests on one principle: **the session guard translates
"how you logged in" into facts; routes declare which facts they need.**
Capabilities are plain `Clone` types; hierarchy (and unions) are encoded
once, at insertion — every downstream check is set-membership.

```rust
#[derive(Clone)] pub struct Employee(pub Person);
#[derive(Clone)] pub struct Employer(pub Person);
#[derive(Clone)] pub struct Member(pub Person);    // the named union

// The session guard — whichever login flow validated, insert the facts
// that session satisfies, plus the kit's domain-free Authenticated:
server_kit::install_middleware(server_kit::from_fn(|ctx| {
    let session = read_session(ctx.headers());
    Box::pin(async move {
        match lookup(session).await {
            Some(Login::Employee(p)) => {
                ctx.insert(server_kit::Authenticated);
                ctx.insert(Employee(p.clone()));
                ctx.insert(Member(p));
            }
            Some(Login::Employer(p)) => {
                ctx.insert(server_kit::Authenticated);
                ctx.insert(Employer(p.clone()));
                ctx.insert(Member(p));
            }
            None => {}          // anonymous continues — guest routes work
        }
        Ok(())
    })
}));
```

`server_kit::Authenticated` is the kit's one domain-free fact — "a session
was established," whatever the scheme. It's what makes failures
status-accurate: `Role<M>` resolves `M` from the context and answers
**401** when there's no session at all, **403** when there's a session
without that capability (`Auth<T>` by contrast always 401s).

Routes then come in exactly the shapes you'd expect:

```rust
#[server]                                   // guest = the ABSENCE of a
async fn browse(q: Query) -> …              // requirement, not a role

#[server(tags(role = "member"))]            // either class (the union fact)
async fn lobby(who: Role<Member>) -> …

#[server(tags(role = "employer"))]          // one class only
async fn payroll(who: Role<Employer>) -> …
```

The deny-by-default net is `role_guard` — the kit owns enforcement, the
app registers its vocabulary (this registration is the kit/user seam made
explicit):

```rust
server_kit::install_middleware(
    server_kit::role_guard()
        .role::<Employer>("employer")
        .role::<Employee>("employee")
        .role::<Member>("member"),
);
```

Per request (unary, each batch entry, stream opens): no `role` tag →
pass; registered value → require the mapped capability (present → pass,
`Authenticated` without it → 403, anonymous → 401); **unregistered value
→ 500, fail closed** — a typo'd role must never silently open a route.

When a route serves multiple classes but *branches* on which, put the sum
type in the signature instead of a union marker: `Auth<Identity>` where
`Identity` is an enum — adding a class later is then a compile error at
every branching route.

### Rate limiting

Same declaration/enforcement/vocabulary split as roles: the endpoint
declares its limit as a valued tag, the kit enforces, the app registers
what "one caller" means:

```rust
#[server(tags(limit = "30/min"))]      // token bucket: burst 30, refill 30/min
async fn send_message(msg: NewMessage, who: Role<Member>) -> Result<(), ServerError> { … }

server_kit::install_middleware(
    server_kit::rate_limit()
        // First matching key_by wins; register after your session guard
        // so the principal is available. Fallbacks: x-forwarded-for,
        // then one shared per-route bucket.
        .key_by(|ctx| ctx.get::<Principal>().map(|p| format!("user:{}", p.username)))
        // Optional: also limit routes with no `limit` tag.
        .default_limit("300/min"),
);
```

Semantics worth knowing:

- Buckets are per `(route, caller)`; grammar is `"<count>/<sec|min|hour|day>"`.
- A rejection is **429 with `Retry-After: <secs>`** — on unary calls, per
  batch entry, and refused stream opens alike. The header rides the
  primitive's *response-header jar* (`server::ResponseHeaderJar` on the
  `Context` for hooks/middleware, `server::append_response_header` inside
  handler bodies — the generalized cookie jar, drained on success AND
  error paths).
- A **malformed `limit` tag fails closed** (500 naming the grammar) —
  a typo never means "unlimited".
- **Where buckets live is a seam** (`LimitStore`). The default
  `MemoryLimitStore` is in-process (N instances ≈ N× the nominal limit).
  For shared limits, provide a Redis connection **as app context — the
  same way a database is provided** — and point the store at it
  (feature `redis` on `server-kit`):

  ```rust
  let client = redis::Client::open(cfg.redis_url)?;
  server::install_state(client.clone());        // fns can use it as context too
  server_kit::install_middleware(
      server_kit::rate_limit()
          .key_by(…)
          .store(server_kit::RedisLimitStore::from_installed()),
  );
  ```

  One connection, many consumers, none owning it. The Redis store runs
  the same token-bucket algorithm as one atomic Lua script per admit, so
  concurrent instances can't double-spend. **Failure policy**: a store
  *outage* fails **open** (with a loud stderr warning) — the limiter
  protects capacity, and its own dependency going down must not become a
  total outage; contrast with config errors, which fail closed. Wrap the
  store if your threat model needs fail-closed limiting.

### Sessions

The cookie/bearer session recipe, productized — and deliberately **built
on the cache abstraction, not a database**: a session store is a
KV-with-TTL, which is exactly the `Cache` contract (below), so sessions
ride whatever store the app provides.

```rust
// boot
let sessions = server_kit::Sessions::new(cache::MemoryCache::new());
//            …or ::new(cache::RedisCache::from_installed()) for multi-instance
server::install_state(sessions.clone());
server_kit::install_middleware(sessions.guard::<Principal>());

// the app's half of login is ONLY the credential check:
#[server]
async fn login(creds: Credentials, sessions: State<Sessions>) -> Result<LoginOk, ServerError> {
    let user = verify(&creds).await.ok_or(ServerError::failed("bad credentials"))?;
    let ticket = sessions.start(&Principal { username: user.name }).await
        .map_err(|e| ServerError::failed(e.to_string()))?;
    Ok(LoginOk { token: ticket.token })   // web: None (cookie set); native: Some(bearer)
}

#[server] async fn me(user: Auth<Principal>) -> Result<String, ServerError> { … }
#[server] async fn logout(sessions: State<Sessions>) -> Result<(), ServerError> {
    sessions.end().await.map_err(|e| ServerError::failed(e.to_string()))
}
```

Everything mechanical lives in the kit: CSPRNG tokens, the httpOnly/
Secure/SameSite=Lax cookie for web + bearer for native (one server, both
transports — `start` branches on the `x-idealyst-client: native` header),
the guard that resolves either transport and inserts `Authenticated` +
your serde principal (so `Auth<P>` / `Role<M>` compose unchanged), TTL /
sliding expiration, and logout. The guard never rejects — an invalid
session is simply anonymous, and the route's own requirements answer. A
session-store outage inserts nothing (protected routes fail closed) with
a stderr warning. `examples/login-demo` runs on this.

### The cache

`crates/sdk/server/cache` is the shared KV-with-TTL provider: a
`Cache` trait (`get`/`set`+TTL/`delete`, byte-oriented, with
`get_json`/`set_json` typed helpers), `MemoryCache`, and `RedisCache`
(feature `redis`) over the **app-provided client** — the same
`install_state`'d `redis::Client` the rate limiter and your server fns
use. Deliberately minimal: anything needing atomic read-modify-write
(rate-limit buckets) is *not* a cache and keeps its purpose-built store
(`LimitStore`); the two share the provided connection, not the trait.
Response-caching is done in fn bodies with `get_json`/`set_json` — an
automatic response-cache middleware is intentionally absent (safe keying
across authenticated callers is app knowledge).

### Observability

The chain's hook wraps every invocation, so the kit reports one
`RequestRecord` per call/open — path, tags, transport outcome, duration —
to any installed observer:

```rust
server_kit::install_observer(server_kit::stderr_logger());
// → [srv] payroll 403 0.4ms (call) [role=employer]

server_kit::install_observer(|r| {
    metrics::histogram!("srv_ms", r.duration, "path" => r.path.to_string());
});
```

Records are transport-level: chain rejections (401/403/429), handler
transport failures, and successful replies with size. A domain
`Err(Failed)` rides a 200 and reports as `Ok` — observing domain errors
is the app's business. Observers run synchronously on the request path;
keep them cheap.

### Kit scope, and extending who/what to guard

The scoping rule: **the kit ships policy machinery whose correctness is
protocol-shaped (401/403, ordering, batch + stream coverage, fail-closed
— where a subtle bug is a silent security hole); the app ships policy
vocabulary, which is domain-shaped.** The kit never names your roles;
your code never re-implements enforcement. Extension surfaces, in
increasing depth:

1. **New capability**: define a `Clone` type, insert it from your guard.
   `Auth<T>` / `Role<M>` / `require_tag::<T>` are generic over it — zero
   kit code.
2. **New credential source** (API keys, webhook HMACs, mTLS): another
   fact-producing middleware inserting the *same* fact types. Routes
   never learn how the caller authenticated.
3. **New policy dimension** (tenancy, feature flags, business hours): a
   middleware reading headers, facts, and your own invented tags — the
   primitive gave tags no meaning precisely so you can.
4. **New requirement semantics** (composite requirements, custom
   statuses): implement `FromContext` on your own wrapper; `#[ctx]`
   wires it into any signature. `Role<M>` itself is nothing more.
5. **A different policy model entirely**: install your own
   `DispatchHook`; borrow the kit's pieces via `server_kit::run_chain`.

### Cookies and the auth patterns

Handlers set response cookies imperatively — the dispatcher drains a
per-request jar:

```rust
server::set_cookie(server::Cookie::new("session", id));  // httpOnly+Secure+Lax by default
server::clear_cookie("session");
```

The blessed shapes, both implemented end-to-end in
[`login-demo`](../examples/login-demo/src/lib.rs):

- **Web — BFF/cookie**: `login` sets an httpOnly session cookie; the browser
  attaches it to same-origin `/_srv/*` calls automatically; the secret never
  enters JS. Pair with `server_kit::csrf_guard([origins])` + SameSite.
- **Native — bearer**: no cookie jar, so `login` returns a token the client
  stores in the `credentials` SDK (Keychain / Android Keystore) and replays
  via a credential provider (§6). One server, one guard, accepts either.

---

## 6. The client side

Configure once at app start:

```rust
server::configure(
    server::ClientConfig::new("https://api.example.com")
        // optional: headers attached to every call — read per request,
        // so a rotated token is picked up automatically
        .with_credentials(server::bearer(|| creds.get("token").ok().flatten())),
);
```

On web, point at the page origin so calls are same-origin (that's what makes
the browser send the httpOnly cookie).

### Calling from UI code

Stubs are plain futures — drive them with `spawn_then`: the call goes in the
future, every signal touch goes in the callback.

```rust
let on_click = move || {
    runtime_core::spawn_then(
        async move { create_todo(input).await },
        move |result| match result {
            Ok(todo) => todos.update(|t| t.push(todo)),
            Err(e) => status.set(format!("failed: {e}")),
        },
    );
};
```

Writing those signals inside the future instead would be a teardown race:
every `.await` is a flush boundary, so navigating away on success (or any
other unmount) frees the component's slots before the continuation resumes,
and the write aborts with `idealyst[stale-signal-handle]`. The callback runs
inside a turn or not at all, so the whole update applies or none of it does.
The `signal-across-await` lint flags the raw form.

The same holds for the reload shape — an `effect!` that reads a counter and
calls a stub, so bumping the counter refetches. `spawn_then` anchors a call
issued from an effect body to that effect's owner, so the callback is skipped
if the component is torn down while the request is in flight, and a re-run of
the effect does *not* cancel a request that already went out.

### Batching (opt-in)

Each call is a direct `POST` by default. Concurrent calls coalesce into one
HTTP request only inside an explicit scope:

```rust
let (todos, me) = server::batch(async {
    futures::join!(list_todos(), whoami())    // one POST /_srv/_batch
}).await;
```

### Cancellation

Inside a `resource()` fetcher, bridge the resource's cancel signal so a dep
change aborts the in-flight request (it resolves as `ServerError::Cancelled`):

```rust
let r = resource(deps, |args, cancel| async move {
    server::with_cancel(cancel, search(args)).await
});
```

`server::with_cancel_token(token, fut)` is the explicit-token variant.

---

## 7. Streaming: `#[subscription]`, `#[channel]`, `#[sse]`

Same cfg-split, same extractors/middleware/errors — only the transport and
return shape differ.

### `#[subscription]` — server → client over WebSocket

```rust
#[subscription]
async fn ticks(rate_ms: u64, db: State<Clock>) -> impl Stream<Item = Tick> {
    db.stream(rate_ms)
}
```

Server: mounts `GET /_srv/_ws/ticks`, resolves extractors at upgrade, pumps
each yielded item to the socket. Non-extractor params are **open args**,
encoded into the connect URL. Client stub: `fn ticks(rate_ms: u64) ->
UseSocket<Tick, ()>` — a scope-bound reactive handle:

```rust
let sub = ticks(250);                       // connects on mount
let line = text(move || format!("{:?}", sub.latest()));
// sub.incoming() is a Signal<Option<Tick>>; the socket closes on unmount.
```

### `#[channel]` — full duplex

First param is `Socket<In, Out>` (In = what the server receives):

```rust
#[channel]
async fn chat(mut ch: Socket<ClientMsg, ServerMsg>, user: Auth<Principal>)
    -> Result<(), ServerError>
{
    while let Some(Ok(m)) = ch.recv().await {
        ch.send(reply(&user, m)).await.ok();
    }
    Ok(())
}
```

Client stub returns the mirrored `UseSocket<ServerMsg, ClientMsg>` —
`send(msg)` up, `incoming()` down, closes on unmount.

### `#[sse]` — server → client over plain HTTP

Same authoring shape as `#[subscription]`, but the transport is
`text/event-stream` (`GET /_srv/_sse/<path>`): survives WS-blocking proxies,
and browsers reconnect natively. The client stub returns the **URL**; consume
it with the typed hook:

```rust
let sse = server::use_sse::<Note>(notifications());
```

Prefer SSE for one-way streams (progress, notifications, LLM tokens); reach
for `#[channel]` only when the client→server direction is real.

**Streaming caveat**: the client stubs return client-only reactive types, so
— unlike plain `#[server]` fns — code consuming them cannot compile under the
server feature. In a colocated crate that code must sit in the
`#[cfg(not(feature = "server"))]` UI region; in the layered shape it lives in
the UI crate and needs nothing.

---

## 8. Versioning and schema drift

At expansion time the macro hashes the *spellings* of the wire arg types +
return type (fixed-seed, stable across compilations) and embeds the hash on
both sides.

- **The hash is a diagnostic, not a gate.** Interop is decided by whether the
  bytes actually (de)serialize — additive, serde-tolerant changes
  (`#[serde(default)]` fields, widened enums) keep working across versions.
  Only when a decode **fails** are hashes compared: differing →
  `ServerError::IncompatibleVersion` (actionable "please update"); matching →
  a genuine `Codec` bug.
- **Opt-in strictness** for endpoints where "it happened to deserialize"
  isn't good enough:

  ```rust
  #[server(strict_version)]
  async fn charge_card(req: ChargeRequest) -> Result<Receipt, ServerError<PayError>> { … }
  ```

  rejects any hash mismatch up front, before decoding, without running the body.
- **Collisions fail at boot**: `router()` builds the dispatch map eagerly and
  panics on a duplicate path, so a collision can't silently shadow a handler.
- `#[server(path = "…")]` overrides the wire path (default: the fn name).

---

## 9. Serving

```rust
#[tokio::main]
async fn main() {
    server::install_state(Arc::new(AppState::new()));   // State<T> source
    install_auth_guard();                               // middleware (§5)

    // Simplest form — API only:
    // server::serve("0.0.0.0:3000".parse().unwrap()).await.unwrap();

    // Composed form — API + the static wasm bundle from one process:
    let app = server::router()                          // /_srv/* + WS + SSE
        .fallback_service(ServeDir::new("dist/web"));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

`server::router()` mounts `POST /_srv/_batch`, every `#[channel]` /
`#[subscription]` / `#[sse]` route, then the catch-all `POST /_srv/*path`.

> **The dead-strip trap.** Handler registration rides `inventory` statics in
> the app lib. If the server bin references *nothing* from that lib, the
> linker strips them and `router()` mounts **zero** routes — every call 404s
> with no build error. Keep at least one real reference into the lib (the
> demo's `install_state(AppState::new())` doubles as this); `router()` also
> warns loudly at startup when it finds an empty inventory. See the comment
> in
> [`server-fn-demo/src/bin/server.rs`](../crates/api/server/examples/server-fn-demo/src/bin/server.rs).

### Wire protocol at a glance

| Route | Transport | Body |
|---|---|---|
| `POST /_srv/<path>` | request/response | JSON args tuple in → JSON `Result<T, E>` out |
| `POST /_srv/_batch` | coalesced calls | array of entries, per-entry middleware |
| `GET /_srv/_ws/<path>` | WebSocket | JSON frames; open args as hex JSON in `?args=` |
| `GET /_srv/_sse/<path>` | Server-Sent Events | each item as a `data:` JSON event |

Schema hashes ride an `x-srv-schema` header (request + response) for the
drift diagnostic.

---

## 10. Known rough edges (current state)

Honest limitations of today's implementation, in priority order:

1. **Colocated crates pay a signature-asymmetry tax.** Because the server
   build exposes the extractor-bearing signature under the same name, client
   code in the same crate must be cfg-gated. *Direction under consideration:*
   emit the wire signature on both builds (server side resolving extractors
   from the ambient request context internally), which would make stub-shaped
   call sites compile everywhere and eliminate most hand-written gates.
2. **No module-level attributes.** Server-only helper modules are gated with
   raw `#[cfg(feature = "server")]`; letting `#[server]` annotate `mod`/`use`
   items would keep authors inside one vocabulary. Per-fn `tags(...)` +
   `require_tag` (§5) now cover declarative group *membership*, but a
   module-level `#[api(prefix = …, guard = …)]` that applies a prefix/tags
   to everything inside still doesn't exist — each fn declares its own.
3. **No CLI scaffolding for the layered shape** (DESIGN.md Phase 6). The
   recommended api/ui/server-bin split and the Cargo feature plumbing are
   hand-assembled today, which is why the examples are colocated.
4. **Cargo plumbing is irreducible.** A proc macro can't declare features or
   optional deps, and Cargo doesn't evaluate custom cfgs for dependency
   gating — the `server` feature block in §2 will remain, whatever else
   improves; scaffolding can stamp it out, syntax can't.
