//! Server functions — one async fn, two compilations. The body runs
//! on the server; the client call site compiles into a typed HTTP
//! stub. Companion to the `#[server]` macro + `crates/api/server`.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection};
use crate::routes::CONCEPTS_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let pitch_ref: Ref<ViewHandle> = Ref::new();
    let how_it_works_ref: Ref<ViewHandle> = Ref::new();
    let wire_ref: Ref<ViewHandle> = Ref::new();
    let project_ref: Ref<ViewHandle> = Ref::new();
    let extractors_ref: Ref<ViewHandle> = Ref::new();
    let tags_ref: Ref<ViewHandle> = Ref::new();
    let kit_ref: Ref<ViewHandle> = Ref::new();
    let batching_ref: Ref<ViewHandle> = Ref::new();
    let cancellation_ref: Ref<ViewHandle> = Ref::new();
    let reactive_ref: Ref<ViewHandle> = Ref::new();
    let cli_ref: Ref<ViewHandle> = Ref::new();
    let next_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: pitch_ref, label: "What server functions are" },
        TocEntry { handle: how_it_works_ref, label: "How the macro splits" },
        TocEntry { handle: wire_ref, label: "The wire" },
        TocEntry { handle: project_ref, label: "Project layout" },
        TocEntry { handle: extractors_ref, label: "Injected parameters" },
        TocEntry { handle: tags_ref, label: "Route tags" },
        TocEntry { handle: kit_ref, label: "The API kit" },
        TocEntry { handle: batching_ref, label: "Batching, for free" },
        TocEntry { handle: cancellation_ref, label: "Cancellation, end-to-end" },
        TocEntry { handle: reactive_ref, label: "Wiring into the UI" },
        TocEntry { handle: cli_ref, label: "Running it" },
        TocEntry { handle: next_ref, label: "Where to go from here" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Server functions",
                blurb = "Define server logic — including database queries — directly inside \
                 your app, as if the client were running it. The compiler splits the \
                 paths based on the build target: the server runs the body, the client \
                 turns the call site into a typed network request, and the matching \
                 server-side handler is registered automatically.",
            )
            PageSection(handle = pitch_ref) { pitch() }
            PageSection(handle = how_it_works_ref) { how_macro_splits() }
            PageSection(handle = wire_ref) { wire_protocol() }
            PageSection(handle = project_ref) { project_layout() }
            PageSection(handle = extractors_ref) { extractors() }
            PageSection(handle = tags_ref) { route_tags() }
            PageSection(handle = kit_ref) { api_kit() }
            PageSection(handle = batching_ref) { batching() }
            PageSection(handle = cancellation_ref) { cancellation() }
            PageSection(handle = reactive_ref) { reactive_integration() }
            PageSection(handle = cli_ref) { cli_flow() }
            PageSection(handle = next_ref) { where_next() }
        }
    };
    layout_with_toc(content, toc)
}

// ============================================================================
// Sections
// ============================================================================

fn pitch() -> Element {
    let snippet = "// In your app crate, alongside your UI code:\n\
                   \n\
                   use server::{server, ServerError, State};\n\
                   \n\
                   #[server]\n\
                   async fn list_todos(user_id: u64, db: State<Db>) -> Result<Vec<Todo>, ServerError> {\n    \
                       db.query(\"SELECT * FROM todos WHERE user_id = $1\", &[&user_id]).await\n\
                   }\n\
                   \n\
                   // In the very same crate, in your UI component. The injected\n\
                   // `db` param never appears at the call site:\n\
                   let todos = list_todos(current_user.id).await?;";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "What server functions are".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "You write the function once. The body runs database \
                queries, reads request headers, touches whatever server-side state your \
                handler needs \u{2014} all expressed in plain Rust. The call site \u{2014} \
                in your UI component, on the same `await` you'd use for a local async \
                fn \u{2014} reads as if the client itself were running that body.".to_string())
            CodePanel(src = snippet)
            Typography(content = "Under the hood, the `#[server]` macro splits the \
                function based on the build target. On the SERVER build, the body \
                compiles verbatim and a handler gets auto-registered at `/_srv/list_todos`. \
                On the CLIENT build, the body is discarded \u{2014} `list_todos(user_id)` \
                becomes a POST that ships `[user_id]` to the server, awaits the response, \
                and decodes it back into `Result<Vec<Todo>, ServerError>`. The signature \
                you wrote IS the wire contract; the compiler checks it on both sides.".to_string())
            Typography(content = "The result is one mental model: one Rust function \
                that happens to execute across a network boundary, with the boundary \
                decided at compile time.".to_string())
        }
    }
}

fn how_macro_splits() -> Element {
    let server_snippet = "// Server build: --features server\n\
                          async fn add(a: i32, b: i32) -> Result<i32, ServerError> {\n    \
                              Ok(a + b)                                  // original body\n\
                          }\n\
                          \n\
                          // Plus an inventory::submit! that registers a handler:\n\
                          // POST /_srv/add → decode args → call add(a, b) → encode result";
    let client_snippet = "// Client build: default features\n\
                          async fn add(a: i32, b: i32) -> Result<i32, ServerError> {\n    \
                              server::__private::call::<(i32, i32), _>(\"add\", &(a, b)).await\n\
                          }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "How the macro splits".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "`#[server]` is an attribute macro. It expands the \
                async fn into two cfg-gated halves and keys off the `server` cargo \
                feature to decide which half each build sees.".to_string())
            CodePanel(src = server_snippet)
            CodePanel(src = client_snippet)
            Typography(content = "Both halves see the same source file. Only one ends \
                up compiled into each artifact \u{2014} the server-only body (and any \
                imports it uses: Diesel, tokio, your DB pool type) never reach the \
                client bundle.".to_string())
        }
    }
}

fn wire_protocol() -> Element {
    let snippet = "# single call\n\
                   POST /_srv/<path>\n\
                   Content-Type: application/json\n\
                   \n\
                   [arg0, arg1, ...]                  →  {\"Ok\": T} | {\"Err\": E}\n\
                   \n\
                   # batched calls (microtask-coalesced)\n\
                   POST /_srv/_batch\n\
                   [{\"path\": \"add\",     \"args\": [2, 3]},\n \
                    {\"path\": \"v1/ping\", \"args\": null}]   →  [{\"Ok\": 5}, {\"Ok\": \"pong\"}]";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "The wire".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "JSON over HTTP. Two routes: single and batched. The \
                framework picks single vs batch automatically based on how many calls \
                you fire in the same tick.".to_string())
            CodePanel(src = snippet)
            Typography(content = "Status codes are reserved for dispatcher-level \
                failures \u{2014} 404 for unknown path, 400 for malformed args. A \
                function that returned `Err(...)` still gets a 200 response with \
                `{\"Err\": ...}` in the body. That keeps domain errors and transport \
                errors visibly separate on the client side.".to_string())
        }
    }
}

fn project_layout() -> Element {
    let snippet = "my-app/\n\
                   \u{251c}\u{2500}\u{2500} shared/   # types + #[server] fns + cfg-gated server state\n\
                   \u{251c}\u{2500}\u{2500} server/   # bin, depends on shared with features=[\"server\"]\n\
                   \u{2514}\u{2500}\u{2500} client/   # one or more clients (web wasm, native, mobile);\n   \
                                  # depend on shared with default features";
    let cfg_snippet = "// shared/src/server_fns.rs\n\
                       \n\
                       // \u{274c} leaks: `use diesel::*` at module scope compiles in\n\
                       // both modes. If diesel isn't in the client's dep graph, this errors.\n\
                       use diesel::prelude::*;\n\
                       \n\
                       // \u{2705} clean: cfg-gated import, only compiled with the server half\n\
                       #[cfg(feature = \"server\")]\n\
                       use diesel::prelude::*;";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Project layout".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "The recommended layout is three crates. The `shared/` \
                crate is the dual-feature one \u{2014} it compiles twice, once with \
                `--features server` (the body runs, with access to your DB / state / \
                imports), once without (the body is replaced with the RPC stub).".to_string())
            CodePanel(src = snippet)
            Typography(content = "Server-only deps (Diesel, Redis, tokio, anything that \
                has no business in a wasm bundle) are declared `optional = true` and \
                activated only by the `server` feature. The macro discards server fn \
                bodies on the client side entirely \u{2014} so references to Diesel \
                inside those bodies never reach the client compilation. Import shape \
                matters too:".to_string())
            CodePanel(src = cfg_snippet)
        }
    }
}

fn extractors() -> Element {
    let state_snippet = "// At server startup:\n\
                         server::install_state(Db::connect().await);\n\
                         \n\
                         // Declared as a parameter, resolved per request on the server:\n\
                         #[server]\n\
                         async fn list_todos(filter: Filter, db: State<Db>) -> Result<Vec<Todo>, ServerError> {\n    \
                             db.query(...).await          // State<T> derefs to T\n\
                         }\n\
                         \n\
                         // The client stub's signature: list_todos(filter) \u{2014}\n\
                         // injected params are stripped; only wire args cross the network.";
    let ctx_snippet = "// Name the dependency as a domain type instead of State<PgPool>:\n\
                       server_kit::context! {\n    \
                           /// The app's database pool.\n    \
                           pub struct Db(sqlx::PgPool);\n\
                       }\n\
                       \n\
                       #[server]\n\
                       async fn list_todos(filter: Filter, #[ctx] db: Db) -> Result<Vec<Todo>, ServerError> {\n    \
                           db.query(\"select ...\").fetch_all().await    // Db derefs to PgPool\n\
                       }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Injected parameters (extractors)".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "A `#[server]` fn's parameters come in two kinds. Wire \
                args serialize and cross the network. Injected params \u{2014} `State<T>`, \
                `Headers`, `Cookies`, `Extension<T>`, and any parameter marked `#[ctx]` \
                \u{2014} resolve on the server from the request context and are stripped \
                from the client stub, so a handler's dependencies read as part of its \
                signature:".to_string())
            CodePanel(src = state_snippet)
            Typography(content = "The `context!` macro goes one step further: declare an \
                app resource as a named domain type, and server fns ask for `db: Db` \
                with the wrapper resolving from the same state registry:".to_string())
            CodePanel(src = ctx_snippet)
            Typography(content = "Extraction failures are status-accurate and separate \
                from your domain errors: missing app state is a 500 that names the fix, \
                a missing authenticated principal is a 401 (see the API kit below), and \
                `Headers` / `Cookies` always resolve. The wrapper types exist on both \
                builds \u{2014} they appear in the shared signature \u{2014} while the \
                resolution machinery compiles server-only.".to_string())
        }
    }
}

fn route_tags() -> Element {
    let snippet = "// Static route metadata: a bare marker, or key = \"value\".\n\
                   #[server(tags(admin))]\n\
                   async fn revoke_key(id: KeyId) -> Result<(), ServerError> { ... }\n\
                   \n\
                   #[server(tags(role = \"employer\"))]\n\
                   async fn payroll(who: Role<Employer>) -> Result<Payroll, ServerError> { ... }\n\
                   \n\
                   #[server(tags(limit = \"30/min\"))]\n\
                   async fn search(q: String) -> Result<Vec<Hit>, ServerError> { ... }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Route tags".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Routes describe themselves with `tags(...)` \u{2014} \
                static metadata the dispatcher hands to the policy layer on every call. \
                The primitive only carries the tags; what a tag means is decided by \
                whatever middleware reads it.".to_string())
            CodePanel(src = snippet)
            Typography(content = "Tags ride everywhere a call does: single dispatches, \
                every entry of a batch, and stream opens (`#[sse]`, `#[channel]`, \
                `#[subscription]`) all present the same metadata. A guard written \
                against a tag covers all transports at once.".to_string())
        }
    }
}

fn api_kit() -> Element {
    let chain_snippet = "// Server boot: one ordered middleware chain runs before every handler.\n\
                         server_kit::install_middleware(server_kit::from_fn(|ctx| {\n    \
                             let session = read_session(ctx.headers());\n    \
                             Box::pin(async move {\n        \
                                 if let Some(user) = lookup(session).await {\n            \
                                     ctx.insert(server_kit::Authenticated);\n            \
                                     ctx.insert(user);              // Auth<Principal> now resolves\n        \
                                 }\n        \
                                 Ok(())\n    \
                             })\n\
                         }));\n\
                         server_kit::install_middleware(server_kit::require_tag::<Principal>(\"admin\"));\n\
                         server_kit::install_middleware(\n    \
                             server_kit::role_guard()\n        \
                                 .role::<Employer>(\"employer\")\n        \
                                 .role::<Employee>(\"employee\"),\n\
                         );\n\
                         server_kit::install_middleware(server_kit::rate_limit().key_by(client_key));";
    let roles_snippet = "// Auth<T>: 401 when the session guard inserted nothing.\n\
                         #[server]\n\
                         async fn me(user: Auth<Principal>) -> Result<Profile, ServerError> {\n    \
                             Ok(user.profile())\n\
                         }\n\
                         \n\
                         // Role<M>: 401 anonymous, 403 signed-in-without-the-capability,\n\
                         // typed access when the right class is present.\n\
                         #[server(tags(role = \"employer\"))]\n\
                         async fn payroll(who: Role<Employer>) -> Result<Payroll, ServerError> {\n    \
                             run_payroll(&who).await\n\
                         }";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "The API kit".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "The `server` crate holds no policy: its one \
                interception surface is a single dispatch-hook slot. `server-kit` is \
                the conventional occupant of that slot \u{2014} an ordered middleware \
                chain plus the auth, role, session, CSRF, rate-limit, and observability \
                helpers most APIs want.".to_string())
            CodePanel(src = chain_snippet)
            Typography(content = "Guards insert facts; extractors consume them. A \
                session guard validates credentials and inserts the principal \u{2014} \
                it never rejects on its own. Downstream, `Auth<T>` answers 401 when the \
                fact is missing, and `Role<M>` distinguishes 401 (no session) from 403 \
                (signed in, lacking this capability):".to_string())
            CodePanel(src = roles_snippet)
            Typography(content = "Perimeters make whole groups deny-by-default: \
                `require::<Principal>(\"admin/\")` guards a path prefix and \
                `require_tag::<Principal>(\"admin\")` guards a tag, so a route that \
                forgot its `Auth` param is still blocked. Rate limits are declared on \
                the route (`tags(limit = \"2/min\")`), keyed however you choose, and \
                refusals carry `Retry-After`. Misconfiguration fails closed: an \
                unregistered role value or a malformed limit is a 500 that names the \
                fix, never an open door.".to_string())
        }
    }
}

fn batching() -> Element {
    let snippet = "// Three calls in the same tick:\n\
                   let (user, todos, projects) = tokio::join!(\n    \
                       get_user(uid),\n    \
                       list_todos(uid),\n    \
                       list_projects(),\n\
                   );\n\
                   \n\
                   // → one POST /_srv/_batch on the wire, not three.";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Batching, for free".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Multiple server-fn calls fired in the same tick \
                coalesce into a single HTTP request. The mechanism is inline microtask \
                coalescing: each call enqueues, the first one becomes the flusher, \
                yields once for siblings to enqueue, then drains the queue into one \
                `POST /_srv/_batch`.".to_string())
            CodePanel(src = snippet)
            Typography(content = "On a typical app-load fan-out \u{2014} \
                `use_query(get_user)` + `use_query(list_todos)` + \
                `use_query(list_projects)` \u{2014} you go from three round-trips to \
                one. Batching is automatic. Open the network tab in any app that uses \
                server functions and you'll see `_srv/_batch` lines for every \
                page mount.".to_string())
        }
    }
}

fn cancellation() -> Element {
    let snippet = "let user_id = signal(1u64);\n\
                   \n\
                   let user = resource(user_id, |id, resource_cancel| async move {\n    \
                       server::with_cancel(resource_cancel, get_user(id)).await\n\
                   });\n\
                   \n\
                   // `user_id.set(2)` cancels:\n\
                   //   1. the resource's prior fetch (ResourceCancel)\n\
                   //   2. the in-flight HTTP request (net::CancelToken)\n\
                   //   3. the actual network read (reqwest drops / browser aborts / iOS \n\
                   //      task.cancel / Android conn.disconnect)";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Cancellation, end-to-end".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "When a `resource` fetcher's deps change, the \
                in-flight server-fn call aborts for real: `server::with_cancel(...)` \
                bridges the reactive system's `ResourceCancel` token to the HTTP \
                transport's cancel primitive, all the way down to the per-platform \
                network stack.".to_string())
            CodePanel(src = snippet)
            Typography(content = "Cancellation interop with batching: if a cancellable \
                call is still queued when its token fires, the flusher removes it from \
                the batch before sending. If it's already in flight, the HTTP completes \
                (the other calls in the batch deserve their results) but the cancelled \
                caller still returns `Cancelled`.".to_string())
        }
    }
}

fn reactive_integration() -> Element {
    let snippet = "let todos: Signal<Vec<Todo>> = signal(Vec::new());\n\
                   \n\
                   // load on mount, refresh on dep change\n\
                   let refresh = async_reducer(\n    \
                       todos,\n    \
                       |_| async { list_todos().await },\n    \
                       |list, new_list| *list = new_list,\n\
                   );\n\
                   \n\
                   // mutation that folds response straight into local state\n\
                   let create = async_reducer(\n    \
                       todos,\n    \
                       |input| async move { create_todo(input).await },\n    \
                       |list, new_todo| list.push(new_todo),\n\
                   );";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Wiring into the UI".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Server functions are async fns. They compose with \
                every reactive async primitive: `resource()` for dep-driven reads, \
                `mutation()` for fire-and-forget writes, `async_reducer()` for writes \
                that fold their response into local state \u{2014} the workhorse \
                pattern for any mutation that updates a list / map / record.".to_string())
            CodePanel(src = snippet)
            Typography(content = "Each reducer exposes loading / error state via its \
                own `AsyncStatus<E>` signal, so UI bindings get spinners + error \
                rendering for free. The data lives in your `Signal<S>`; the lifecycle \
                lives on the handle.".to_string())
        }
    }
}

fn cli_flow() -> Element {
    let snippet = "# Cargo.toml\n\
                   [package.metadata.idealyst.app]\n\
                   targets    = [\"web\"]\n\
                   server_bin = \"server\"      # opt the project into the full-stack flow\n\
                   \n\
                   # one command — builds wasm, runs the server bin, watches src/ for changes:\n\
                   idealyst dev --web --local my-app";
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Running it".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Declare `server_bin = \"<name>\"` in your manifest \
                and the CLI runs the full stack with one command \u{2014} builds the \
                wasm bundle into `pkg/`, launches `cargo run --bin <name> --features \
                server`, and watches your source for changes. Every edit triggers a \
                fresh wasm build + a server restart.".to_string())
            CodePanel(src = snippet)
            Typography(content = "The server bin serves both the API (at `/_srv/*`) \
                AND the wasm bundle (at `/` and `/pkg/*`) on one port. Open the URL it \
                prints and the whole app \u{2014} UI + API \u{2014} comes from one \
                process.".to_string())
        }
    }
}

fn where_next() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Where to go from here".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "Server functions plug into the rest of the framework \
                through the same reactive primitives you'd use for any async work. If \
                you haven't read the Core concepts page yet, the signals + components \
                model is the foundation everything here builds on.".to_string())
            link(route = &CONCEPTS_ROUTE, params = ()) {
                Typography(content = "Read \u{2192} Core concepts".to_string())
            }
            Typography(content = "The example app at `crates/api/server/examples/server-fn-demo` is a \
                runnable todo app exercising every concept on this page \u{2014} \
                CRUD, batching, cancellation, extractors, the async_reducer pattern, \
                all of it. For the policy layer, `crates/api/server-kit/tests/kit_e2e.rs` \
                runs the full chain \u{2014} session guard, tag perimeters, roles, rate \
                limits \u{2014} over real HTTP.".to_string())
        }
    }
}
