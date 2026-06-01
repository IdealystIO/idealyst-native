# Server functions — full-stack architecture (v1 design)

Status: design. The v0 proof-of-concept (the `#[server]` macro, JSON-over-HTTP
via the `net` SDK, inventory dispatch, tick-coalesced batching) is shipped and
documented in the crate headers. This document specifies the v1 expansion into a
foundation for full-stack development. It is the contract the implementation
phases below must satisfy; each phase lands with tests per the repo's testing
rules.

The full-stack API layer lives under `crates/api/` (`server`, `server-macros`,
and the future middleware / `storage` crates), kept separate from the UI-facing
`crates/sdk/` extension primitives. General transport primitives (`net`, and a
future cross-platform `storage`) stay in `crates/sdk/`; the API layer composes
them. Note the distinct sense of "api" in §0 below: that refers to the *author's*
app crate that holds their `#[server]` fns, not this repo's `crates/api/` home.

## Goals

1. **Type-safe domain errors** across the wire, not stringified.
2. **State, auth, and request context** as injected handler parameters, not
   per-call boilerplate.
3. **A provable client/server boundary** — a server function's only effect on the
   client is its return value; it cannot name or touch client state.
4. **Middleware** as the extension seam for cross-cutting server logic.
5. **Stable, collision-resistant function identity** and a precise, identifiable
   error when the client and server schemas have drifted.
6. **API versioning** that does not force a full client-bundle ship for
   additive, backward-compatible changes.
7. **Auth-scheme agnosticism**, backed by the cross-platform primitives auth
   actually needs (header injection, cookies, secure storage).
8. **Deliberate batching** — opt-in, not a silent default.

Non-goals for v1: streaming/subscriptions, exposing idealyst fns *as* GraphQL.
GraphQL *consumption* is addressed as a BFF recipe, not framework surface.

---

## 0. The enforcement model (the provable boundary)

A server function's contract is: typed data in, typed data out. Three
independent constraints make that boundary airtight rather than conventional.

1. **Type boundary (have it).** Args and return must be
   `Serialize + DeserializeOwned`. A `Signal<T>`, `Element`, or backend handle is
   not serializable, so it cannot cross in either direction. Enforced by the
   trait bounds the macro emits.

2. **Capture boundary (have it).** `#[server]` functions are free functions: the
   macro rejects a `self` receiver, and a free function captures nothing from an
   enclosing scope. There is no client state for a body to close over.

3. **Reference boundary (the v0 gap).** Even with 1 and 2, a v0 body can still
   *name* a client-only item (`signal!()`, `web_sys::window()`) because the
   shared crate links the UI runtime. We close this with **dependency layering**:

   ```
   api/         #[server] fns + shared DTOs + the domain error enum.
                deps: server, serde, and (gated behind `server` feature) db/tokio/…
                Does NOT depend on the UI runtime. A body cannot name `Signal`
                because the type is not in scope — a compile error, not a lint.
   ui/          The app. Depends on `api` (for the generated stubs) + the UI runtime.
   server-bin/  Depends on `api` with `features = ["server"]`; calls `router()`.
   ```

   The CLI scaffolds this shape by default. For authors who insist on colocating
   server fns next to UI in one crate, the supported fallback is to gate UI
   imports behind `#[cfg(not(feature = "server"))]` so the server build of the
   body cannot see them, plus a shipped clippy `disallowed-types` config naming
   `runtime_core::Signal` et al. as a belt-and-suspenders check. Layering is the
   recommendation; colocation is possible but the author opts into the weaker
   guarantee.

Runtime note: the body is `#[cfg]`'d out of every client build, so even absent
all of the above it never executes client-side. In AAS / runtime-server mode the
reactive runtime runs host-side, so the boundary is logical rather than a
process split — the type-level enforcement above still holds, and a server-fn
call from host-resident app code may be lowered to a local `await` (see §7).

---

## 1. Typed errors

`ServerError` becomes generic over the author's domain error:

```rust
pub enum ServerError<E = Infallible> {
    /// The server-side body returned `Err(e)`. Serialized across the wire;
    /// `E` lives in the shared `api` crate so both sides agree on the shape.
    App(E),
    Network(String),
    Codec(String),
    Server { status: u16, message: String },
    Cancelled,
    /// Client/server schema drift for this fn — see §5. Distinct from `Codec`
    /// so the app can react ("please update") instead of guessing.
    IncompatibleVersion { path: String, client_schema: u64, server_schema: u64 },
}
```

Recommended return shape: `Result<T, ServerError<MyError>>`. The whole `Result`
is JSON-serialized as today; the only change is that the error half now carries a
typed `E`. The existing `ServerFnReturn` trait is the seam where transport
failures fold in — it becomes generic over `E`:

```rust
impl<T, E> ServerFnReturn for Result<T, ServerError<E>> {
    fn from_server_error(error: ServerError<E>) -> Self { Err(error) }
}
```

Authors who want a bare `Result<T, MyError>` (no visible `ServerError` wrapper)
can `#[derive(ServerError)]` on `MyError` to auto-add a `Transport(ServerError)`
variant and the fold impl.

---

## 2 + 7. Context, state, and extractor parameters (the keystone)

The single hardcoded `headers` field on `RequestContext` is generalized to a
typed extension map:

```rust
pub struct Context { extensions: TypeMap }   // anymap-style, keyed by TypeId
impl Context {
    pub fn get<T: Clone + 'static>(&self) -> Option<T>;
    pub fn insert<T: Send + Sync + 'static>(&mut self, v: T);
}
```

Handler parameters are classified by the macro into **wire args** (serialized,
present in the client stub) and **injected extractors** (resolved server-side,
absent from the client stub). The discriminator is a trait:

```rust
pub trait FromContext: Sized {
    async fn from_context(ctx: &Context) -> Result<Self, ServerError>;
}
```

**Status: implemented** (`State`/`Headers`/`Extension`). `Context` is a typed
extension map (`Arc`-shared headers + `TypeId`→`Any` map), `FromContext` returns
`impl Future<…> + Send`, and an extraction failure is a `TransportError::Server {
status, .. }` (e.g. 500 for missing state) surfaced to the client as
`ServerError::Server` — infrastructure, not a domain `Failed`. `Auth<T>` is
`Extension<T>` set by a guard; the named `Auth`/`Cookies` aliases land with
middleware in Phase 2.

Built-in extractors, all `FromContext`:

| Type           | Source                                              |
|----------------|-----------------------------------------------------|
| `State<T>`     | the process-wide app-state registry (Deref to T)    |
| `Headers`      | the request's `HeaderMap` (Deref to it)             |
| `Extension<T>` | `ctx.get::<T>()`, set by middleware (Deref to T)    |
| `Auth<T>`      | (Phase 2) named alias of `Extension<T>` from a guard |
| `Cookies`      | (Phase 2) parsed request cookies                    |

**Classification rule (macro).** A proc-macro sees syntax, not trait impls, so the
wire-arg/extractor split is syntactic: a parameter is injected if it is annotated
`#[ctx]` **or** its type names a reserved wrapper (`State`/`Headers`/`Extension`,
matched on the final path segment so `server::State<T>` also counts). Reserved
names cover the built-ins with zero ceremony; `#[ctx]` opts any other
`FromContext` type in. The `#[ctx]` helper is consumed by `#[server]` and never
reaches rustc. Because the wrapper *types* appear in the shared signature but the
client stub strips the params, authors gate extractor imports and server-only
state types behind `#[cfg(feature = "server")]` (the bodies using them are
server-only anyway), which also avoids unused-import warnings on client builds.

So a handler reads:

```rust
#[server]
async fn create_todo(
    input: CreateTodo,      // wire arg
    db: State<Db>,          // injected; absent from the client stub
    user: Auth<Principal>,  // injected by the auth guard
) -> Result<Todo, ServerError<TodoError>> {
    db.insert(user.id, input)   // State<Db>/Auth<Principal> Deref to T
}
```

The client stub is `create_todo(input: CreateTodo) -> Result<Todo, …>`. This
removes the v0 `use_state::<Arc<AppState>>().ok_or_else(…)` repeated in every
body: a missing `State<Db>` is one clear failure at the injection point, and the
client signature never sees server-only params. `FromContext` is open, so SDKs
and apps add their own extractors.

This single mechanism is the resolution of the v0 critiques in items 2 and 7: the
global `TypeId` registry stops being read ad-hoc inside bodies, and per-request
data stops being a one-off field.

---

## 4. Middleware

```rust
pub trait Middleware: Send + Sync {
    async fn handle(&self, ctx: &mut Context, next: Next<'_>) -> Result<Vec<u8>, ServerError>;
}
```

A tower-style onion the dispatcher runs around `(entry.handler)(body)`. Two roles
fall out of the same trait:

- **Context-producing** — an auth guard runs, validates credentials, and
  `ctx.insert(principal)`. A downstream `Auth<Principal>` extractor reads it.
- **Wrapping** — logging, timing, rate-limiting, short-circuit.

Attach points: global on `router()`, per-namespace on an `#[api(guard = …)]`
group, or per-fn. The dispatcher already opens a `Context` scope per call (and
per batch entry), so middleware runs correctly for batched calls per-entry with
no extra work. The trait is framework-level (not raw axum) so it also covers the
in-process/AAS path where there is no HTTP layer to hang tower on.

---

## 5. Function identity + schema drift

**Stable path = fully-qualified module path.** The macro embeds `module_path!()`
into the registration, yielding `/_srv/todos::list`. Rust guarantees uniqueness
within a crate by construction, it is human-readable in a network trace, and it
is stable when only the body changes (unlike a signature hash, which changes on
every edit, or a name-only hash, which is just the name obscured). Explicit
`#[server(path = "…")]` still overrides.

**Collision enforcement = fail-fast at boot.** `router()` builds a
`HashMap<(path, schema_hash), handler>` once and **panics on a duplicate**, so a
server with a collision refuses to start. (Inventory's separate-compilation model
makes true cross-crate compile-time detection impractical; a boot panic plus a CI
test helper that asserts no duplicates is the pragmatic enforcement.)

**Schema hash = structural fingerprint.** A derived trait contributes each type's
structural shape:

```rust
pub trait SchemaHash { fn schema_hash(h: &mut Hasher); }   // #[derive(SchemaHash)]
```

The per-fn fingerprint hashes `(args schema, return schema)`. It is the version
tag, carried alongside the stable path — not as the path.

**The hash is a diagnostic, never a gate.** Interoperability is decided by
whether the bytes actually (de)serialize, not by hash equality. A response that
decodes cleanly into the client's expected type is used as-is, *regardless of the
hash* — this is what lets additive, serde-tolerant changes (a new
`#[serde(default)]` field, a widened enum) keep working across versions without
ceremony. The hash only earns its keep on the failure path: when decode genuinely
fails, it tells us *why*.

- **Return side.** The server echoes its return-schema hash in a response header.
  The client decodes first; on success it does nothing with the hash. Only if
  decode **fails** does it compare: a differing hash means the types are no longer
  interoperable → `IncompatibleVersion { path, client_schema, server_schema }`
  ("your app is outdated"); a matching hash means a genuine `Codec` bug at the
  same version. This replaces v0's undifferentiated `Codec` failure with a precise
  cause, without ever rejecting a payload that decoded fine.
- **Arg side.** Symmetric: the server decodes the args first; only on decode
  failure does it use the client-sent hash to distinguish `IncompatibleVersion`
  from a real `Codec` error.

So the hash is purely a *diagnostic refinement on the error path*. Routing
between deliberately-kept multiple live versions (§6) still uses the hash as a
hint, but a single-version server never rejects a call on hash grounds — it tries
to serve it and only escalates if the bytes don't fit.

**Opt-in strict gating.** Tolerance-first is the default, but an endpoint that
must refuse any drift can opt into hard gating per-fn:

```rust
#[server(strict_version)]
async fn charge_card(req: ChargeRequest) -> Result<Receipt, ServerError<PayError>> { … }
```

`strict_version` flips the diagnostic into a gate for *that* endpoint: the server
compares the client-sent hash up front and returns `IncompatibleVersion` *without
running the body* on any mismatch, decode-tolerant or not. This is the right
default-off knob for money-movement / irreversible operations where "it happened
to deserialize" is not good enough — the author declares the endpoint
version-locked. Everything else stays tolerant.

---

## 6. Versioning

The compatibility unit is the per-fn schema hash from §5, **decoupled from the
client bundle hash**, so most API changes do not force a new bundle:

- **Additive, backward-compatible changes** (a new field with `#[serde(default)]`)
  are tolerated by serde already; old clients keep working, no rev.
- **Multi-version registration.** The router holds several `(path, hash)` entries
  per path. A new `todos::list@v2` can ship while `@v1` stays live; old clients
  keep hitting v1. How long v1 survives is host config, not framework policy.
- **Graceful failure.** When no compatible version exists, the client receives
  `IncompatibleVersion` (§5) and the app decides — soft prompt, hard block, etc.
  A helper renders the canonical "update required" state.

Trade-off acknowledged: keeping N versions live is real host-side cost. The
framework provides the mechanism (multi-version registration + negotiation) and
leaves the retention policy to the host.

---

## 8. Deliberate batching

Default flips to a direct single call (`POST /_srv/<path>`). Coalescing becomes
an explicit scope:

```rust
server::batch(|| async {
    let (todos, me) = join!(list_todos(), whoami());  // these coalesce into one POST
}).await
```

A task-local flag set by `batch(…)` is what the client dispatch checks; calls
outside the scope go direct. This makes the latency-coupling trade-off (a slow
call delaying a fast one in the same request) a visible, opted-into choice. The
reactive layer may open a batch scope per render tick, but only when the author
opts in — never implicitly. The existing batch machinery is retained; only its
entry point is gated.

---

## 3. Cross-platform auth primitives

The SDK stays scheme-agnostic (bearer / JWT / cookie session / custom) by
shipping the primitives auth composes from. Grounding from the `net` SDK today:
per-request and default headers exist; there is **no** cookie jar, web fetch does
**not** set `credentials`, and there is **no** storage abstraction anywhere.

- **Header injection (small).** `ClientConfig` gains a credential source the
  request/batch flusher reads and attaches:
  ```rust
  pub trait CredentialSource: Send + Sync {
      fn authorize(&self, req: &mut RequestParts);   // add header(s), or no-op for cookie mode
  }
  ```
  Covers bearer/JWT immediately with no new transport work.

- **Cookies (medium, per-platform).** Add a credentials/with-cookies switch to
  `net`. On web it maps to fetch `credentials: 'include'` and the browser's jar
  (ideally an httpOnly, XSS-safe session cookie the app never sees) does the rest.
  On native each platform has a jar to enable + persist (NSURLSession
  `HTTPCookieStorage`, reqwest `cookies`, Android `CookieHandler`). One switch,
  per-platform plumbing.

- **Storage (medium-large, new crate).** Token auth on native needs somewhere to
  persist the token; nothing exists. A new `storage` SDK crate parallel to `net`:
  an async KV API (async so IndexedDB / Keychain fit), with a plain tier (web
  `localStorage` / iOS `UserDefaults` / Android `SharedPreferences` / native
  file) and a **secure** tier (Keychain / Android Keystore / OS keyring) for
  tokens.

Server side defines an **auth guard** (`Middleware` that validates and
`ctx.insert(principal)`); client side defines a **credential source**. The scheme
is user-land or a thin opt-in helper crate, never baked into core. The
recommended shape differs per platform (httpOnly cookie on web; secure-storage +
header on native), and the primitives make both expressible.

---

## Wire protocol v1 (delta from v0)

- Path: `/_srv/<module::path>` (was bare fn name).
- New request header `x-idealyst-schema: <hex>` (negotiation; §5).
- New response header `x-idealyst-schema: <hex>` (return-drift detection; §5).
- Body shapes unchanged: args tuple JSON in; `Result<T, E>` JSON out — `E` is now
  the author's typed error.
- New structured failure for version mismatch (typed `IncompatibleVersion` body).
- Batch route unchanged; reached only inside a `server::batch(…)` scope.

## Macro expansion v1 (shape)

1. Partition params: `FromContext` impls → injected; the rest → wire args.
2. Client stub signature = wire args only; body calls
   `call(path, schema_hash, &args)`.
3. Server registration: handler decodes wire args, resolves each injected param
   via `from_context(ctx)`, awaits the body, encodes `Result<T, E>`.
4. Emit the `module_path!()`-based path and a `const SCHEMA_HASH`.

---

## Build phases

Each phase is independently shippable and lands with tests (repo rules §1, §8).

- **Phase 0 — Typed errors. ✅ DONE.** `ServerError<E = String>`, generic
  `ServerFnReturn` (assoc. `Error`), `TransportError`/`into_domain`,
  `IncompatibleVersion` variant defined. Tested both modes. (item 1)
  (`#[derive(ServerError)]` sugar deferred — `ServerError<E>` covers the need.)
- **Phase 1 — Keystone. ✅ DONE.** `Context` extension map, `FromContext`
  (`impl Future + Send`), `State<T>`/`Headers`/`Extension<T>`, `#[ctx]` +
  reserved-name macro classification, extraction-failure → HTTP status. Tested
  both modes + extractor unit tests. (items 2, 7)
- **Phase 2 — Middleware + guards. ✅ DONE.** `Middleware` trait + `from_fn` +
  `install_middleware`; the dispatcher runs the chain (single + per batch entry)
  before the handler, short-circuiting on error with its HTTP status. `Context`
  gained mutable `insert` + the matched `path` (for guard scoping). Added
  `Auth<T>` (missing → 401) and `Cookies` extractors. Tested both modes + unit
  tests. Post-handler wrapping (timing/logging) noted as a follow-on. (item 4)
- **Phase 3 — Identity + versioning. ✅ DONE (core).** Boot-time collision
  detection (`router()` builds a path→entry `HashMap`, panics on duplicate;
  dedup logic unit-tested). Per-fn schema hash (macro hashes wire arg + return
  type spelling via fixed-seed `DefaultHasher`, embedded both sides). Negotiation
  over `x-srv-schema` header: server runs a drift diagnostic on arg-decode
  failure (mismatch → 426 `IncompatibleVersion`, else 400) and a `strict_version`
  pre-decode gate; client echoes the same on response-decode failure (the
  return-type-drift "your app is outdated" signal). `schema_for(path)`
  introspection. Tested both modes + unit. Deferred: auto module-path
  qualification (inventory's const requirement makes `module_path!()`
  concatenation into a `&'static str` awkward; bare-name default + the boot
  collision panic is the safety net) and multi-version live registration. (items 5, 6)
- **Phase 4 — Deliberate batching. ✅ DONE.** Direct single call by default;
  coalescing happens only inside a `server::batch(future)` scope (a per-poll
  thread-local mirroring `cancel.rs`). Tested: in-scope concurrent calls →
  one `/_srv/_batch`; out-of-scope → N direct requests. (item 8)
- **Phase 5 — Auth primitives.** client credential source; `net` credentials +
  cookies; new `storage`/`secure_storage` crate. (item 3)
- **Phase 6 — Enforcement scaffolding.** layered `api`/`ui`/`server-bin` CLI
  templates; clippy `disallowed-types`; colocation cfg-gating recipe. (item 0)
- **Later — GraphQL BFF recipe.** server fns as a typed gateway over an existing
  GraphQL/REST system; DTOs codegen'd from the upstream schema.

## Open decisions

1. **`storage` as a new SDK crate vs. a `Backend` trait capability.** The repo's
   "core stays minimal / mobile-first" stance argues for a separate crate
   (parallel to `net`), keeping the `Backend` trait UI-focused. Leaning crate.
2. **Schema negotiation cadence: per-call header vs. once-per-session handshake.**
   Per-call is simplest and stateless but adds a header to every request;
   session handshake is leaner on the wire but needs connection state the current
   stateless HTTP model lacks. Leaning per-call for v1, revisit if measured.
