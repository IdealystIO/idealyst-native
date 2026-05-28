//! Server functions — one async fn, two compilations. The body runs
//! on the server; the client call site compiles into a typed HTTP
//! stub. Companion to the `#[server]` macro + `crates/sdk/server`.

use runtime_core::{ui, Primitive, Ref, ViewHandle};
use idea_ui::{stack, typography, StackGap};

use crate::pages::common::{code_panel, page_header, page_section};
use crate::routes::CONCEPTS_ROUTE;
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Primitive {
    let pitch_ref: Ref<ViewHandle> = Ref::new();
    let how_it_works_ref: Ref<ViewHandle> = Ref::new();
    let wire_ref: Ref<ViewHandle> = Ref::new();
    let project_ref: Ref<ViewHandle> = Ref::new();
    let extractors_ref: Ref<ViewHandle> = Ref::new();
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
        TocEntry { handle: extractors_ref, label: "App state & request data" },
        TocEntry { handle: batching_ref, label: "Batching, for free" },
        TocEntry { handle: cancellation_ref, label: "Cancellation, end-to-end" },
        TocEntry { handle: reactive_ref, label: "Wiring into the UI" },
        TocEntry { handle: cli_ref, label: "Running it" },
        TocEntry { handle: next_ref, label: "Where to go from here" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            { page_header(
                "Server functions",
                "Define server logic — including database queries — directly inside \
                 your app, as if the client were running it. The compiler splits the \
                 paths based on the build target: the server runs the body, the client \
                 turns the call site into a typed network request, and the matching \
                 server-side handler is registered automatically."
            ) }
            { page_section(pitch_ref, vec![pitch()]) }
            { page_section(how_it_works_ref, vec![how_macro_splits()]) }
            { page_section(wire_ref, vec![wire_protocol()]) }
            { page_section(project_ref, vec![project_layout()]) }
            { page_section(extractors_ref, vec![extractors()]) }
            { page_section(batching_ref, vec![batching()]) }
            { page_section(cancellation_ref, vec![cancellation()]) }
            { page_section(reactive_ref, vec![reactive_integration()]) }
            { page_section(cli_ref, vec![cli_flow()]) }
            { page_section(next_ref, vec![where_next()]) }
        }
    };
    layout_with_toc(content, toc)
}

// ============================================================================
// Sections
// ============================================================================

fn pitch() -> Primitive {
    let snippet = "// In your app crate, alongside your UI code:\n\
                   \n\
                   use server::{server, ServerError};\n\
                   \n\
                   #[server]\n\
                   async fn list_todos(user_id: u64) -> Result<Vec<Todo>, ServerError> {\n    \
                       let db = server::use_state::<Arc<Db>>()\n        \
                           .ok_or_else(|| ServerError::failed(\"Db not installed\"))?;\n    \
                       db.query(\"SELECT * FROM todos WHERE user_id = $1\", &[&user_id]).await\n\
                   }\n\
                   \n\
                   // In the very same crate, in your UI component:\n\
                   let todos = list_todos(current_user.id).await?;";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "What server functions are".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(content = "You write the function once. The body runs database \
                queries, reads request headers, touches whatever server-side state your \
                handler needs \u{2014} all expressed in plain Rust. The call site \u{2014} \
                in your UI component, on the same `await` you'd use for a local async \
                fn \u{2014} reads as if the client itself were running that body.".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "Under the hood, the `#[server]` macro splits the \
                function based on the build target. On the SERVER build, the body \
                compiles verbatim and a handler gets auto-registered at `/_srv/list_todos`. \
                On the CLIENT build, the body is discarded \u{2014} `list_todos(user_id)` \
                becomes a POST that ships `[user_id]` to the server, awaits the response, \
                and decodes it back into `Result<Vec<Todo>, ServerError>`. The signature \
                you wrote IS the wire contract; the compiler checks it on both sides.".to_string())
        },
        ui! {
            Typography(content = "The result is one mental model. You're not maintaining \
                a client API and a server API and a DTO crate in lockstep \u{2014} you're \
                writing one Rust function that happens to execute across a network \
                boundary. The boundary is a compile-time decision, not a code-organization \
                tax.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn how_macro_splits() -> Primitive {
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
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "How the macro splits".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(content = "`#[server]` is an attribute macro. It expands the \
                async fn into two cfg-gated halves and keys off the `server` cargo \
                feature to decide which half each build sees.".to_string())
        },
        code_panel(server_snippet),
        code_panel(client_snippet),
        ui! {
            Typography(content = "Both halves see the same source file. Only one ends \
                up compiled into each artifact \u{2014} the server-only body (and any \
                imports it uses: Diesel, tokio, your DB pool type) never reach the \
                client bundle.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn wire_protocol() -> Primitive {
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
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "The wire".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(content = "JSON over HTTP. Two routes: single and batched. The \
                framework picks single vs batch automatically based on how many calls \
                you fire in the same tick.".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "Status codes are reserved for dispatcher-level \
                failures \u{2014} 404 for unknown path, 400 for malformed args. A \
                function that returned `Err(...)` still gets a 200 response with \
                `{\"Err\": ...}` in the body. That keeps domain errors and transport \
                errors visibly separate on the client side.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn project_layout() -> Primitive {
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
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Project layout".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(content = "The recommended layout is three crates. The `shared/` \
                crate is the dual-feature one \u{2014} it compiles twice, once with \
                `--features server` (the body runs, with access to your DB / state / \
                imports), once without (the body is replaced with the RPC stub).".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "Server-only deps (Diesel, Redis, tokio, anything that \
                has no business in a wasm bundle) are declared `optional = true` and \
                activated only by the `server` feature. The macro discards server fn \
                bodies on the client side entirely \u{2014} so references to Diesel \
                inside those bodies never reach the client compilation. Import shape \
                matters too:".to_string())
        },
        code_panel(cfg_snippet),
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn extractors() -> Primitive {
    let state_snippet = "// At server startup:\n\
                         server::install_state(Arc::new(Db::connect().await));\n\
                         \n\
                         // Inside any server fn body:\n\
                         #[server]\n\
                         async fn list_todos(user_id: u64) -> Result<Vec<Todo>, ServerError> {\n    \
                             let db = server::use_state::<Arc<Db>>()\n        \
                                 .ok_or_else(|| ServerError::failed(\"Db not installed\"))?;\n    \
                             db.query(...).await\n\
                         }";
    let header_snippet = "#[server]\n\
                          async fn whoami() -> Result<String, ServerError> {\n    \
                              let auth = server::use_request_header(\"authorization\")\n        \
                                  .ok_or_else(|| ServerError::failed(\"missing Authorization\"))?;\n    \
                              Ok(format!(\"authenticated as: {auth}\"))\n\
                          }";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "App state and per-request data".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(content = "Server-side code gets two flavors of context. \
                App-level state (DB pool, config, S3 client) is registered once at \
                startup and read with `use_state::<T>` \u{2014} the registry is \
                `TypeId`-keyed, so install one of each type:".to_string())
        },
        code_panel(state_snippet),
        ui! {
            Typography(content = "Per-request data (headers today; authenticated user / \
                trace id / extracted query params later) is set by the dispatcher into \
                a `tokio::task_local!` scope before invoking the handler:".to_string())
        },
        code_panel(header_snippet),
        ui! {
            Typography(content = "Both extractors are server-only \u{2014} they don't \
                exist in the client build, so importing them inside a `#[server]` body \
                is safe; that body never reaches the wasm compilation.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn batching() -> Primitive {
    let snippet = "// Three calls in the same tick:\n\
                   let (user, todos, projects) = tokio::join!(\n    \
                       get_user(uid),\n    \
                       list_todos(uid),\n    \
                       list_projects(),\n\
                   );\n\
                   \n\
                   // → one POST /_srv/_batch on the wire, not three.";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Batching, for free".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(content = "Multiple server-fn calls fired in the same tick \
                coalesce into a single HTTP request. The mechanism is inline microtask \
                coalescing: each call enqueues, the first one becomes the flusher, \
                yields once for siblings to enqueue, then drains the queue into one \
                `POST /_srv/_batch`.".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "On a typical app-load fan-out \u{2014} \
                `use_query(get_user)` + `use_query(list_todos)` + \
                `use_query(list_projects)` \u{2014} you go from three round-trips to \
                one. Authors don't opt in. Open the network tab in any app that uses \
                server functions and you'll see `_srv/_batch` lines for every \
                page mount.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn cancellation() -> Primitive {
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
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Cancellation, end-to-end".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(content = "When a `resource` fetcher's deps change, the \
                in-flight server-fn call should actually abort \u{2014} not just have \
                its result discarded. `server::with_cancel(...)` bridges the reactive \
                system's `ResourceCancel` token to the HTTP transport's cancel \
                primitive, all the way down to the per-platform stack.".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "Cancellation interop with batching: if a cancellable \
                call is still queued when its token fires, the flusher removes it from \
                the batch before sending. If it's already in flight, the HTTP completes \
                (the other calls in the batch deserve their results) but the cancelled \
                caller still returns `Cancelled`.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn reactive_integration() -> Primitive {
    let snippet = "let todos: Signal<Vec<Todo>> = signal!(Vec::new());\n\
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
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Wiring into the UI".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(content = "Server functions are async fns. They compose with \
                every reactive async primitive: `resource()` for dep-driven reads, \
                `mutation()` for fire-and-forget writes, `async_reducer()` for writes \
                that fold their response into local state \u{2014} the workhorse \
                pattern for any mutation that updates a list / map / record.".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "Each reducer exposes loading / error state via its \
                own `AsyncStatus<E>` signal, so UI bindings get spinners + error \
                rendering for free. The data lives in your `Signal<S>`; the lifecycle \
                lives on the handle.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn cli_flow() -> Primitive {
    let snippet = "# Cargo.toml\n\
                   [package.metadata.idealyst.app]\n\
                   targets    = [\"web\"]\n\
                   server_bin = \"server\"      # opt the project into the full-stack flow\n\
                   \n\
                   # one command — builds wasm, runs the server bin, watches src/ for changes:\n\
                   idealyst dev --web --local my-app";
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Running it".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(content = "Declare `server_bin = \"<name>\"` in your manifest \
                and the CLI runs the full stack with one command \u{2014} builds the \
                wasm bundle into `pkg/`, launches `cargo run --bin <name> --features \
                server`, and watches your source for changes. Every edit triggers a \
                fresh wasm build + a server restart.".to_string())
        },
        code_panel(snippet),
        ui! {
            Typography(content = "The server bin serves both the API (at `/_srv/*`) \
                AND the wasm bundle (at `/` and `/pkg/*`) on one port. Open the URL it \
                prints and the whole app \u{2014} UI + API \u{2014} comes from one \
                process.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}

fn where_next() -> Primitive {
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Where to go from here".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(content = "Server functions plug into the rest of the framework \
                through the same reactive primitives you'd use for any async work. If \
                you haven't read the Core concepts page yet, the signals + components \
                model is the foundation everything here builds on.".to_string())
        },
        ui! {
            Link(route = &CONCEPTS_ROUTE, params = ()) {
                Typography(content = "Read \u{2192} Core concepts".to_string())
            }
        },
        ui! {
            Typography(content = "The example app at `examples/server-fn-demo` is a \
                runnable todo app exercising every concept on this page \u{2014} \
                CRUD, batching, cancellation, extractors, the async_reducer pattern, \
                all of it.".to_string())
        },
    ];
    ui! { Stack(gap = StackGap::Md) { children } }
}
