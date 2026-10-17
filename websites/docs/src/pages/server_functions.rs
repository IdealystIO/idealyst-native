//! Server functions page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "server-functions",
    title = "Server functions",
    category = Foundation,
    description = "Define server logic — database queries included — directly inside your app. The compiler splits client and server paths at build time; the signature is the wire contract.",
    related = ["async-reactivity", "net"],
    concepts = [
        ServerFn,
        ServerFnWire,
        ServerFnBatch,
        ServerError,
        ServerState,
        ServerExtractor,
        ServerFnCancel,
    ],

    section(heading = "Overview") {
        p("Server functions exist to collapse a particular kind of busywork: \
           maintaining a client API, a server API, and a DTO crate in lockstep \
           forever. The point isn't that the framework hands you a nicer HTTP \
           client — it's that ", code("server logic, including database queries,"),
          " can be defined directly inside your app, and the compiler figures out \
           where each piece runs."),
        code(rust, r##"
            use server::{server, ServerError};

            #[server]
            async fn list_todos(user_id: u64) -> Result<Vec<Todo>, ServerError> {
                let db = server::use_state::<Arc<Db>>()
                    .ok_or_else(|| ServerError::failed("Db not installed"))?;
                db.query("SELECT * FROM todos WHERE user_id = $1", &[&user_id]).await
            }

            // In a UI component in the SAME crate:
            let todos = list_todos(current_user.id).await?;
        "##),
        p("The call site reads as if the client were running that body directly. \
           Under the hood, the ", code("#[server]"),
          " macro splits the function based on the build target:"),
        list(
            ["Server build (", code("--features server"),
             "): the body compiles verbatim and a handler gets auto-registered at ",
             code("/_srv/list_todos"), ". Imports like ", code("diesel::prelude::*"),
             " — cfg-gated to the server build — are visible to that body."],
            ["Client build (default): the body is DISCARDED. The call site compiles \
              into a typed RPC stub — ", code("list_todos(uid)"), " becomes a ",
             code("POST /_srv/list_todos"),
             " that ships the args as JSON, awaits the response, and decodes it \
              back into ", code("Result<Vec<Todo>, ServerError>"), "."],
        ),
        p("The Rust types you wrote in the signature ARE the wire contract. No IDL, \
           no protobuf, no ", code("cargo generate-types"),
          " step you have to remember. The compiler verifies the contract on both \
           sides of the network boundary because the same source file is the \
           authority for both."),
        p("That last point compounds: the cost the framework absorbs isn't \"one \
           fewer codegen step\" — it's the entire client-API + server-API + \
           types-in-the-middle ceremony that most full-stack codebases pay forever. \
           Here the boundary is a compile-time decision, not a code-organization \
           tax."),
    },

    section(heading = "The macro split") {
        p(code("#[server]"),
          " is an attribute macro. It expands an async fn into two cfg-gated \
           halves, keying off the ", code("server"),
          " cargo feature to pick which half each build sees."),
        p("Server build (", code("--features server"), "):"),
        code(rust, r##"
            async fn add(a: i32, b: i32) -> Result<i32, ServerError> {
                Ok(a + b)                                        // original body
            }

            mod __server_fn_add {
                inventory::submit! {
                    server::__private::ServerFnEntry {
                        path: "add",
                        handler: |body_bytes| Box::pin(async move {
                            let (a, b): (i32, i32) =
                                server::__private::decode_args(&body_bytes)?;
                            let result: Result<i32, ServerError> = super::add(a, b).await;
                            server::__private::encode_result(&result)
                        }),
                    }
                }
            }
        "##),
        p("Client build (default features):"),
        code(rust, r##"
            async fn add(a: i32, b: i32) -> Result<i32, ServerError> {
                let args = (a, b);
                server::__private::call::<(i32, i32), Result<i32, ServerError>>(
                    "add",
                    &args,
                ).await
            }
        "##),
        p("Both halves see the same source file; only one half of the expansion \
           ends up in each compiled artifact."),
    },

    section(heading = "Attribute arguments") {
        code(rust, r##"
            #[server]                              // path = function name ("add")
            #[server(path = "v1/users/list")]      // explicit path
            #[server(strict_version)]              // reject schema-mismatched clients
            #[server(tags(admin, limit = "30/min"))] // metadata a dispatch hook reads
        "##),
        p("The path appears after ", code("/_srv/"),
          " in the URL. Use it to version endpoints or scope groups of functions. ",
          code("strict_version"), " makes the server reject a client whose wire \
           schema hash differs, with ", code("ServerError::IncompatibleVersion"),
          " — see Errors below. ", code("tags(...)"),
          " attaches static metadata a dispatch hook reads with ",
          code("ctx.has_tag"), " / ", code("ctx.tag"), "."),
    },

    section(heading = "Constraints") {
        list(
            ["Must be ", code("async"), "."],
            ["Must declare a return type (no implicit ", code("()"), ")."],
            ["No ", code("self"), " receivers (server fns are free functions)."],
            ["Args must be ", code("Serialize + DeserializeOwned"), "."],
            ["Return must be ", code("Result<T, E>"),
             " where T is Serialize + DeserializeOwned and E implements ",
             code("ServerFnReturn"), ". The built-in ",
             code("ServerError"), " already does."],
        ),
    },

    section(heading = "The wire protocol") {
        p("Deliberately small:"),
        list(
            ["Single call: ", code("POST /_srv/<path>"), " with JSON body ",
             code("[arg0, arg1, ...]"), ". Response is JSON ",
             code("{\"Ok\": T}"), " or ", code("{\"Err\": E}"),
             " — the standard serde Result encoding. Status is always ",
             code("200"), " for a function that ran (success or domain error); \
              only the dispatcher itself returns 4xx/5xx (404 for unknown path, \
              400 for malformed args)."],
            ["Batched calls: ", code("POST /_srv/_batch"),
             " with a JSON array of ", code("{path, args}"),
             ". Response is a same-length array of Result slots."],
        ),
        p("The batch route is the same protocol applied N times. The framework \
           picks single vs batch automatically — see the Batching section below."),
        note(kind = Info) {
            p("Why JSON? Easy to debug, universally supported by every client \
               transport without extra deps. The ", code("IntoBody"), " / ",
              code("FromBody"), " traits in ", link("Net", to = "net"),
              " are where another body format would plug in."),
        },
    },

    section(heading = "The build setup") {
        p("The recommended layout for a real app is three crates:"),
        code(text, r##"
            my-app/
            ├── shared/   # types + #[server] fns + cfg-gated server state
            ├── server/   # bin, depends on shared with features=["server"]
            └── client/   # one or more clients (web wasm, native, mobile);
                          # depend on shared with default features
        "##),
        p(code("shared/"), "'s ", code("Cargo.toml"), ":"),
        code(text, r##"
            [features]
            default = []
            server = ["server/server", "dep:diesel", "dep:tokio"]

            [dependencies]
            server = { workspace = true }
            serde = { version = "1", features = ["derive"] }
            diesel = { version = "2", features = ["postgres"], optional = true }
            tokio = { version = "1", features = ["macros", "rt-multi-thread"], optional = true }
        "##),
        list(
            ["Server-only deps (Diesel, Redis, tokio — anything that has no \
              business in a wasm bundle) are declared ", code("optional = true"),
             " and activated only by the ", code("server"), " feature."],
            ["The ", code("server"), " feature forwards to ", code("server/server"),
             " so the macro expands its server half."],
            ["Build commands MUST be separate per binary so cargo doesn't unify \
              features across them."],
        ),
    },

    section(heading = "Cargo hygiene — keeping server deps out of the client") {
        p("Correct:"),
        code(text, r##"
            # ✓ correct
            cargo build -p server --features server
            idealyst build --web client

            # ✗ leaks server-only code into the client
            cargo build --all-bins --features server
        "##),
        p("Diesel never gets compiled into the wasm bundle. The macro discards \
           server function bodies on the client side entirely, so the references \
           to Diesel in those bodies never reach the client compilation. The shape \
           of the IMPORTS matters too:"),
        code(rust, r##"
            // ❌ leaks: `use diesel::*` at module scope is compiled in both
            // modes. If diesel isn't in the client's dep graph, this errors.
            use diesel::prelude::*;

            #[server]
            async fn list_todos() -> Result<Vec<Todo>, ServerError> { ... }
        "##),
        code(rust, r##"
            // ✅ clean: cfg-gated import, only compiled with the server half
            #[cfg(feature = "server")]
            use diesel::prelude::*;

            #[server]
            async fn list_todos() -> Result<Vec<Todo>, ServerError> { ... }
        "##),
    },

    section(heading = "Wiring up the server") {
        p("The server binary builds a router from the inventory of registered \
           functions:"),
        code(rust, r##"
            use server_fn_demo::state::AppState;
            use std::sync::Arc;

            #[tokio::main]
            async fn main() {
                // App-level state — see Extractors below.
                server::install_state(Arc::new(AppState::new()));

                // server::router() returns an axum::Router with /_srv/* routes.
                // Compose with whatever else your server needs (static files,
                // health checks, custom middleware) before serving.
                let app = server::router()
                    .nest_service("/pkg", ServeDir::new("./pkg"))
                    .fallback_service(ServeDir::new("."));

                let addr: SocketAddr = "0.0.0.0:3000".parse().unwrap();
                let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
                axum::serve(listener, app).await.unwrap();
            }
        "##),
        p(code("server::router()"),
          " is just axum. It walks the inventory once and registers ",
          code("/_srv/_batch"), " + ", code("/_srv/*path"), ". Compose with ",
          code(".nest_service"), ", ", code(".layer"), ", ",
          code(".merge"), " — whatever axum supports."),
        p(code("server::serve(addr)"),
          " is a one-liner shortcut if you don't need custom composition."),
    },

    section(heading = "Wiring up the client") {
        code(rust, r##"
            use server::ClientConfig;

            server::configure(ClientConfig::new("https://api.example.com"));
        "##),
        p(code("configure"),
          " is idempotent — call it once at app start, or call it again to \
           retarget (useful for tests against an ephemeral server)."),
        p("For web apps, the natural base URL is the page's own origin:"),
        code(rust, r##"
            #[cfg(target_arch = "wasm32")]
            {
                let origin = deep_link::origin()
                    .unwrap_or_else(|| "http://localhost:3000".to_string());
                server::configure(ClientConfig::new(origin));
            }
        "##),
        p("Native apps run by ", code("idealyst dev"),
          " get the dev server's address from ", code("server::dev_base_url()"),
          " (it reads the ", code("IDEALYST_SERVER_URL"),
          " the CLI sets); it returns ", code("None"),
          " in production and on web."),
        p("Server-fn calls happen against ", code("<base_url>/_srv/..."),
          ". The transport underneath is the ", link("net", to = "net"),
          " SDK — same per-platform HTTP stack, no separate code path."),
    },

    section(heading = "Batching") {
        p("Three calls fired in the same tick coalesce into one HTTP request:"),
        code(rust, r##"
            let (user, todos, projects) = tokio::join!(
                get_user(uid),
                list_todos(uid),
                list_projects(),
            );
        "##),
        p("becomes one ", code("POST /_srv/_batch"),
          " with three entries, not three separate posts. The server unpacks the \
           array, dispatches each entry against its handler, and returns the \
           results in the same order."),
        p("The mechanism is INLINE MICROTASK COALESCING:"),
        list(
            ["Each call enqueues into a process-global pending queue."],
            ["The first caller to observe an empty queue becomes the FLUSHER for \
              that batch — it yields once (giving sibling calls a chance to \
              enqueue on the same tick), then drains the queue and dispatches."],
            ["Solo flushes (only one call in the queue at flush time) keep the \
              single-call wire (", code("POST /_srv/<path>"),
             "). Multi-entry flushes go through ", code("_batch"), "."],
        ),
        p("Authors don't opt in — every server-fn call funnels through this \
           pipeline. A solo call pays one task-yield of latency, which is one \
           frame at most."),
        note(kind = Info) {
            p("The on-mount fan-out is where batching pays for itself. A typical \
               app loads several resources when a page mounts (three ",
              code("resource(...)"), " fetchers calling ", code("get_user"), ", ",
              code("list_todos"), ", and ", code("list_projects"),
              "). With batching, that fan-out is one request on the wire, not \
               three. Open the network tab in any of the example apps to see the ",
              code("_srv/_batch"), " line."),
        },
    },

    section(heading = "Cancellation — via a resource fetcher") {
        p("When a ", code("resource()"),
          "'s deps change, the in-flight fetch should abort. Wrap the fetcher's \
           future in ", code("server::with_cancel"), ":"),
        code(rust, r##"
            use runtime_core::resource;
            use server::with_cancel;

            let user_id = signal(1u64);

            let user = resource(user_id, |id, resource_cancel| async move {
                with_cancel(resource_cancel, get_user(id)).await
            });
        "##),
        p(code("with_cancel"), " does three things:"),
        list(
            ["Creates a fresh ", code("(net::CancelHandle, net::CancelToken)"),
             " pair."],
            ["Registers ",
             code("resource_cancel.on_cancel(|| handle.cancel())"),
             " so a resource cancel flows downstream to the HTTP layer."],
            ["Wraps the inner future in a scope that puts the token into a \
              thread-local. The macro's client stub reads it and threads it into \
              the underlying ", code("net::RequestBuilder::cancel_on(token)"), "."],
        ),
        p("A single ", code("dep.set(new_value)"), " cancels three things at \
           once: the resource's prior fetch (via ", code("resource_cancel"),
          "), the in-flight HTTP request (via ", code("net::CancelToken"),
          "), and the actual network read on whichever platform (reqwest drops the \
           future, browser fetches abort, iOS ",
          code("NSURLSessionTask::cancel"), ", Android ",
          code("HttpURLConnection::disconnect"), ")."),
    },

    section(heading = "Cancellation — explicit") {
        p("For UIs that own the cancel decision (a button, a user gesture), use ",
          code("server::with_cancel_token"), " with a token you constructed:"),
        code(rust, r##"
            let (handle, token) = net::cancel_token();

            let task = tokio::spawn(server::with_cancel_token(token, do_long_op()));

            // later:
            handle.cancel();
            let result = task.await?;  // → Err(ServerError::Cancelled)
        "##),
    },

    section(heading = "Cancellation + batching interop") {
        p("If a cancellable call is still QUEUED (not yet flushed), the flusher \
           REMOVES it from the batch before sending. The cancelled call's awaiter \
           returns ", code("Cancelled"), " immediately; the rest of the batch is \
           unaffected."),
        p("If the cancel fires AFTER the batch has gone over the wire, the HTTP \
           request runs to completion (other calls in the batch deserve their \
           results), but the cancelled call's awaiter still returns ",
          code("Cancelled"), " — its slot in the response is discarded."),
        p("For solo calls (queue size 1 at flush time), cancel aborts the \
           underlying HTTP via ", code("net::RequestBuilder::cancel_on"),
          ". The future returns ", code("Cancelled"),
          " and the transport tears the connection down."),
    },

    section(heading = "Errors and ServerFnReturn") {
        p("Server fns return ", code("Result<T, E>"), " where E implements ",
          code("ServerFnReturn"), ":"),
        code(rust, r##"
            pub trait ServerFnReturn: Sized {
                /// The domain error `E` of this return type's `ServerError<E>`.
                type Error;

                /// Construct a Self representing a non-domain failure
                /// (transport, codec, version, dispatcher-level error).
                fn from_server_error(error: ServerError<Self::Error>) -> Self;
            }
        "##),
        p(code("Result<T, ServerError<E>>"),
          " already implements it, for any serializable domain error ",
          code("E"), ". ", code("E"), " defaults to ", code("String"),
          ", so most apps just write ", code("Result<T, ServerError>"),
          ". For typed errors, spell ", code("Result<T, ServerError<MyError>>"),
          " — no extra impl needed. The error enum has six variants:"),
        code(rust, r##"
            pub enum ServerError<E = String> {
                Failed(E),                               // server fn returned Err
                Network(String),                         // transport failure (client only)
                Codec(String),                           // JSON encode/decode failed
                Server { status: u16, message: String }, // dispatcher rejected
                Cancelled,                               // cancel token fired
                IncompatibleVersion {                    // client/server schema drift
                    path: String,
                    client_schema: u64,
                    server_schema: u64,
                },
            }
        "##),
        p("The split matters:"),
        list(
            [code("Failed"),
             " is what your code returns when business logic fails (",
             code("return Err(ServerError::failed(\"not found\"))"),
             "). Encoded into a ", code("200"), " body as ",
             code("{\"Err\": {\"Failed\": \"not found\"}}"),
             ". The client sees ",
             code("Err(ServerError::Failed(\"not found\"))"), "."],
            [code("Network"),
             " is only ever observed on the client; it means the request never \
              reached the function."],
            [code("Codec"),
             " means the wire bytes didn't decode. Usually a sign of schema drift \
              between client and server builds."],
            [code("Server"),
             " is the dispatcher's response — 404 (unknown path), 400 (malformed \
              args), 500 (handler panic). Distinct from ", code("Failed"), "."],
            [code("Cancelled"), " is a cancel-token cancellation."],
            [code("IncompatibleVersion"),
             " means the client and server disagree on this function's wire \
              schema. The macro hashes the arg and return types on both \
              sides; when a payload fails to decode and the hashes differ, \
              you get this instead of ", code("Codec"),
             ". A ", code("strict_version"),
             " function rejects a hash mismatch up front (HTTP 426)."],
        ),
    },

    section(heading = "Extractors — app-level state") {
        p(code("install_state(value)"),
          " registers something globally. ", code("use_state::<T>()"),
          " inside a server fn reads it back:"),
        code(rust, r##"
            // In your server bin's main:
            server::install_state(Arc::new(Db::connect().await));

            // In a server fn:
            #[server]
            async fn list_todos(user_id: u64) -> Result<Vec<Todo>, ServerError> {
                let db = server::use_state::<Arc<Db>>()
                    .ok_or_else(|| ServerError::failed("Db not installed"))?;
                db.query("SELECT * FROM todos WHERE user_id = ?", &[&user_id]).await
            }
        "##),
        p("The registry is ", code("TypeId"),
          "-keyed, so install one of each type you want to expose. Wrap heavy \
           state in ", code("Arc"), " so retrieval is cheap. Available only when ",
          code("feature = \"server\""),
          " is on — the client build doesn't see any of this."),
    },

    section(heading = "Extractors — per-request data") {
        p("Request headers:"),
        code(rust, r##"
            #[server]
            async fn whoami() -> Result<String, ServerError> {
                let auth = server::use_request_header("authorization")
                    .ok_or_else(|| ServerError::failed("missing Authorization"))?;
                Ok(format!("authenticated as: {auth}"))
            }
        "##),
        p("Mechanically: the dispatcher wraps the handler's future in a ",
          code("tokio::task_local!"), " scope carrying the request's ",
          code("HeaderMap"), ". ", code("use_request_header(name)"),
          " reads from it. Outside a handler (utility code, background tasks) the \
           accessor returns ", code("None"), " rather than panicking."),
        p(code("use_request_headers()"), " returns the full ",
          code("Arc<HeaderMap>"), " if you need to iterate."),
    },

    section(heading = "How the pieces connect to async reactivity") {
        p("Server functions are async fns. They compose with every primitive on \
           the ", link("Async reactivity", to = "async-reactivity"), " page:"),
        code(rust, r##"
            // load on mount, refresh on dep change
            let user = resource(user_id, |id, cancel| async move {
                with_cancel(cancel, get_user(id)).await
            });

            // fire-and-forget mutation (analytics, etc.)
            let log = mutation(|name: String| async move { log_event(name).await });

            // the workhorse: mutation that folds response into local state
            let create = async_reducer(
                todos,
                |input| async { create_todo(input).await },
                |list, new_todo| list.push(new_todo),
            );
        "##),
        p("The shape is always: ", code("#[server]"),
          " fn → wrap in the primitive that fits how the UI uses it."),
    },

    section(heading = "Streaming") {
        p("Three sibling attributes stream instead of returning once. They \
           share the cfg split, extractors, middleware, and errors with ",
          code("#[server]"), "; only the transport and return shape differ."),
        list(
            [code("#[subscription]"),
             " — server → client over WebSocket. The fn returns ",
             code("impl Stream<Item = T>"), "; the client stub returns a \
              scope-bound ", code("UseSocket<T, ()>"),
             " that connects on mount and closes on unmount."],
            [code("#[channel]"),
             " — full duplex over WebSocket. The first param is a ",
             code("Socket<In, Out>"), "; the client gets the mirrored ",
             code("UseSocket<Out, In>"), " with ", code("send(msg)"), " and ",
             code("incoming()"), "."],
            [code("#[sse]"),
             " — server → client over plain HTTP (",
             code("text/event-stream"),
             "). The client stub returns the URL; consume it with ",
             code("server::use_sse::<T>(url)"),
             ". Prefer it for one-way streams."],
        ),
    },

    section(heading = "CLI integration") {
        p("Declare the server bin in the app metadata and ",
          code("idealyst dev"), " runs the whole stack:"),
        code(text, r##"
            [package.metadata.idealyst.app]
            server_bin = "server"    # → cargo run --bin server --features server
        "##),
        p("On web, the dev loop runs your server bin instead of the static \
           dev server, so ", code("/_srv/*"),
          " works and the bundle and API are served same-origin. A server \
           in another package is named with ", code("server_manifest"),
          ". Native apps get the address through ",
          code("server::dev_base_url()"), "."),
    },

    section(heading = "Per-call credentials") {
        p("Attach a ", code("CredentialProvider"),
          " to the client config and its headers go on every server-fn \
           request. It is called per request, so a refreshed token is picked \
           up without reconfiguring:"),
        code(rust, r##"
            server::configure(
                ClientConfig::new(base)
                    .with_credentials(server::bearer(|| read_token())),
            );
        "##),
        p(code("server::credentials_from_fn(|| vec![...])"),
          " covers any other header scheme. Cookie auth needs no provider."),
    },

    section(heading = "Limits") {
        list(
            ["Wire format is JSON. There is no binary format option."],
            [code("idealyst new"),
             " has no template for the three-crate full-stack layout."],
        ),
    },

    section(heading = "Where to read more") {
        list(
            [link("Async reactivity", to = "async-reactivity"), " — ",
             code("resource"), ", ", code("mutation"), ", ",
             code("async_reducer"),
             ", and how they consume server-fn futures."],
            [link("Net", to = "net"),
             " — the HTTP client SDK underneath. Same cancel-token primitive, \
              same ", code("IntoBody"), " / ", code("FromBody"), " traits."],
            [code("crates/api/server/examples/server-fn-demo"),
             " — runnable full-stack todo app exercising every concept on this \
              page."],
        ),
    },
}
