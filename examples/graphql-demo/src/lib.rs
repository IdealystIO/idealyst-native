//! `graphql-demo` — a **full-stack** GraphQL books app on the [`graphql`]
//! SDK.
//!
//! - The **server** (`--bin server --features server`) holds an
//!   `async-graphql` schema (the [`schema`] module) behind a single
//!   `#[server]` fn, [`graphql_endpoint`], and serves the wasm client from
//!   the same origin.
//! - The **client** runs typed `graphql_client`-derived operations
//!   ([`GetBooks`], [`AddBook`]) through [`graphql::use_query`] /
//!   [`graphql::use_mutation`]. Add a book → the mutation runs server-side,
//!   then the query refetches and the list updates.
//!
//! ## "Works with server functions, without depending on them"
//!
//! The GraphQL request rides the **server-functions** transport — the
//! [`graphql_transport!`](graphql::graphql_transport) macro bridges
//! [`graphql_endpoint`] (a `#[server]` fn) into the client, so it reuses the
//! server-fn HTTP path, CSRF, and per-platform config. The `graphql` crate
//! itself never depends on `server`: an app off that stack would instead use
//! `GraphqlClient::http(url)` against any GraphQL endpoint.
//!
//! ## Run it
//!
//! ```text
//! idealyst dev --web examples/graphql-demo
//! ```

use std::rc::Rc;
#[cfg(feature = "server")]
use std::sync::Arc;

use graphql::{use_mutation, use_query, GraphqlClient};
use graphql_client::GraphQLQuery;
use idea_ui::{
    install_idea_theme, light_theme, tone, typography_kind, variant, Button, Card, CardPadding,
    Field, Stack, StackAlign, StackAxis, StackGap, StackPadding, Typography,
};
use runtime_core::{component, effect, memo, rx, signal, ui, Element};
use server::{server, ServerError};

// ============================================================================
// Typed operations — `graphql_client` codegen against the committed
// schema.graphql. Compile on BOTH builds (no async-graphql here). `Clone` is
// required by `use_query`/`use_mutation` (resource/mutation value types).
// ============================================================================

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "schema.graphql",
    query_path = "operations.graphql",
    response_derives = "Debug, Clone"
)]
pub struct GetBooks;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "schema.graphql",
    query_path = "operations.graphql",
    response_derives = "Debug, Clone"
)]
pub struct AddBook;

// ============================================================================
// Server-only schema (async-graphql). Gated by `server` so the wasm client
// never compiles the engine — exactly the decoupling the SDK is built for.
// ============================================================================

#[cfg(feature = "server")]
pub mod schema {
    // Direct `async_graphql` import — its derive macros expand to
    // `::async_graphql::…` paths, so the crate must be at the root.
    use async_graphql::{Context, EmptySubscription, Object, Schema, SimpleObject, ID};
    use std::sync::{Arc, Mutex};

    /// A book in the catalog.
    #[derive(Clone, SimpleObject)]
    pub struct Book {
        pub id: ID,
        pub title: String,
        pub author: String,
    }

    /// The authoritative store. A real app swaps this for a DB pool.
    pub type Db = Arc<Mutex<Vec<Book>>>;

    pub struct Query;

    #[Object]
    impl Query {
        /// All books, newest first.
        async fn books(&self, ctx: &Context<'_>) -> Vec<Book> {
            ctx.data_unchecked::<Db>()
                .lock()
                .unwrap()
                .iter()
                .rev()
                .cloned()
                .collect()
        }
    }

    pub struct Mutation;

    #[Object]
    impl Mutation {
        /// Add a book and return it.
        async fn add_book(&self, ctx: &Context<'_>, title: String, author: String) -> Book {
            let mut db = ctx.data_unchecked::<Db>().lock().unwrap();
            let book = Book {
                id: ID(format!("{}", db.len() + 1)),
                title,
                author,
            };
            db.push(book.clone());
            book
        }
    }

    /// The concrete schema type installed as server state.
    pub type AppSchema = Schema<Query, Mutation, EmptySubscription>;

    /// Build the schema, pre-seeded with one book.
    pub fn build() -> AppSchema {
        let db: Db = Arc::new(Mutex::new(vec![Book {
            id: ID("1".into()),
            title: "The Rust Programming Language".into(),
            author: "Klabnik & Nichols".into(),
        }]));
        Schema::build(Query, Mutation, EmptySubscription)
            .data(db)
            .finish()
    }
}

// ============================================================================
// The single GraphQL endpoint. On the wasm client this is an RPC stub that
// POSTs to `/_srv/graphql_endpoint`; on the server it executes the schema.
// ============================================================================

/// Execute a GraphQL request against the installed schema.
#[server]
pub async fn graphql_endpoint(
    req: graphql::GraphqlRequest,
) -> Result<graphql::GraphqlResponse, ServerError> {
    let schema = server::use_state::<Arc<schema::AppSchema>>()
        .ok_or_else(|| ServerError::failed("GraphQL schema not installed"))?;
    // An authenticated app would inject its principal here:
    //   graphql::execute_request_with(schema.as_ref(), req, |r| r.data(user)).await
    Ok(graphql::execute_request(schema.as_ref(), req).await)
}

// Bridge the server fn into a client transport — the "works with server
// functions" seam. The crate stays free of any `server` dependency.
graphql::graphql_transport!(AppGraphql, graphql_endpoint);

/// Point the `server` SDK at the API host. Web uses the page origin (the
/// server bin serves bundle + endpoint from one port); native dev points at
/// the host running the server.
fn configure_server() {
    #[cfg(target_arch = "wasm32")]
    {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
        server::configure(server::ClientConfig::new(origin));
    }
    #[cfg(all(not(feature = "server"), not(target_arch = "wasm32")))]
    {
        let base = if cfg!(target_os = "android") {
            "http://10.0.2.2:3000" // Android emulator's host-loopback alias
        } else {
            "http://127.0.0.1:3000"
        };
        server::configure(server::ClientConfig::new(base));
    }
}

// ============================================================================
// Client UI.
// ============================================================================

/// A book flattened for the reactive list.
#[derive(Clone, PartialEq)]
struct BookRow {
    id: String,
    title: String,
    author: String,
}

/// Root component. Runs a typed `GetBooks` query on mount and an `AddBook`
/// mutation on submit, refetching the list when the mutation lands.
#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());
    configure_server();

    let client = GraphqlClient::new(Rc::new(AppGraphql));

    let title = signal!(String::new());
    let author = signal!(String::new());

    // Typed query + mutation bound to this component scope.
    let books = use_query::<GetBooks>(&client, get_books::Variables);
    let add = use_mutation::<AddBook>(&client);

    // The query result, projected into a reactive row list for the keyed
    // `for`. `memo` is scope-owned, so it survives past this fn and
    // recomputes when the query resolves.
    let rows = memo(move || {
        books
            .data()
            .map(|data| {
                data.books
                    .iter()
                    .map(|b| BookRow {
                        id: b.id.clone(),
                        title: b.title.clone(),
                        author: b.author.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });

    // Refetch the list whenever the mutation lands a new book.
    {
        let add = add.clone();
        effect!({
            if add.data().is_some() {
                books.refetch();
            }
        });
    }

    let on_title: Rc<dyn Fn(String)> = Rc::new(move |v: String| title.set(v));
    let on_author: Rc<dyn Fn(String)> = Rc::new(move |v: String| author.set(v));

    let submit: Rc<dyn Fn()> = {
        let add = add.clone();
        Rc::new(move || {
            let t = title.get();
            let a = author.get();
            if t.trim().is_empty() || a.trim().is_empty() {
                return;
            }
            title.set(String::new());
            author.set(String::new());
            add.trigger(add_book::Variables {
                title: t,
                author: a,
            });
        })
    };

    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Lg) {
            Typography(content = "GraphQL Books".to_string(), kind = typography_kind::H1)
            Typography(
                content = "A typed GraphQL client (graphql_client) over async-graphql, riding a server function. Add a book — the mutation runs server-side, then the query refetches.".to_string(),
                muted = true,
            )

            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::End) {
                Field(
                    label = Some("Title".to_string()),
                    value = title,
                    on_change = on_title,
                    placeholder = Some("Book title".to_string()),
                )
                Field(
                    label = Some("Author".to_string()),
                    value = author,
                    on_change = on_author,
                    placeholder = Some("Author".to_string()),
                )
                Button(label = "Add".to_string(), on_click = submit, tone = tone::Primary, variant = variant::Filled)
            }

            Typography(
                content = rx!(match books.error() {
                    Some(e) => format!("error: {e}"),
                    None if books.loading() => "loading…".to_string(),
                    None => format!("{} book(s)", rows.get().len()),
                }),
                muted = true,
            )

            Stack(gap = StackGap::Sm) {
                for row in rows, key = format!("{}|{}", row.id, row.title) {
                    Card(padding = CardPadding::Md) {
                        Stack(axis = StackAxis::Row, gap = StackGap::Md, align = StackAlign::Center) {
                            Typography(content = row.title.clone())
                            Typography(content = format!("— {}", row.author), muted = true)
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// CLI-generated wrapper hooks.
// ============================================================================

/// SDK-registration hook the platform wrappers call before mount.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

/// Recorder-side registration for the native dev-server sidecar.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}
