//! Reactive hooks — `use_query` and `use_mutation` — that bind a GraphQL
//! operation to the reactive runtime.
//!
//! They are thin wrappers over `runtime_core`'s [`resource`] and
//! [`mutation`] primitives, so they inherit `data()` / `error()` /
//! `loading()` / `refetch()` (queries) and `trigger()` / `state()`
//! (mutations) for free, and integrate with `ui!` exactly like any other
//! resource.
//!
//! Like every reactive hook, these must run **inside a `#[component]`
//! body** (an owning scope) for re-render to work — see
//! `project_flatlist_needs_component_scope`.
//!
//! ## Derive requirements
//!
//! `resource`/`mutation` require their value and error types to be `Clone`.
//! `GraphqlError` already is; the generated `ResponseData` is not unless you
//! ask for it. Add `response_derives = "Clone"` (and `"Debug"` for
//! ergonomics) to the `#[derive(GraphQLQuery)]` attribute:
//!
//! ```ignore
//! #[derive(GraphQLQuery)]
//! #[graphql(
//!     schema_path = "schema.graphql",
//!     query_path = "queries.graphql",
//!     response_derives = "Debug, Clone"
//! )]
//! struct GetBooks;
//! ```

use crate::{GraphqlClient, GraphqlError, GraphqlRequest};
use graphql_client::GraphQLQuery;
use runtime_core::{mutation, resource, Mutation, Resource, Signal};

/// Run a GraphQL query bound to the current component scope. Fetches on
/// mount; call [`Resource::refetch`] to re-run with the same variables.
///
/// Returns a [`Resource`] whose `data()` is the decoded `ResponseData`.
///
/// ```ignore
/// #[component]
/// fn book_list(client: GraphqlClient) -> Element {
///     let books = use_query::<GetBooks>(&client, get_books::Variables {});
///     ui! {
///         when(move || books.loading(),
///             || ui! { text("Loading…") },
///             move || ui! { text(move || format!("{:?}", books.data())) })
///     }
/// }
/// ```
pub fn use_query<Q>(
    client: &GraphqlClient,
    variables: Q::Variables,
) -> Resource<Q::ResponseData, GraphqlError>
where
    Q: GraphQLQuery,
    Q::ResponseData: Clone + 'static,
{
    let client = client.clone();
    // Build the request once. The `resource` fetcher may run repeatedly
    // (refetch), so capture the *result* — both `GraphqlRequest` and
    // `GraphqlError` are `Clone`, so the captured `Result` is too.
    let built = GraphqlRequest::from_query::<Q>(variables);

    // A stable, never-changing trigger: the resource fetches on mount and
    // re-runs only via `refetch()`. (`resource` needs a `Trackable` dep;
    // a fresh `Signal` is the minimal one.)
    let trigger: Signal<u32> = Signal::new(0);

    resource(trigger, move |_, _cancel| {
        let client = client.clone();
        let built = built.clone();
        async move {
            let request = built?;
            client.execute_typed::<Q::ResponseData>(request).await
        }
    })
}

/// Create a GraphQL mutation handle bound to the current component scope.
/// Nothing runs until you call [`Mutation::trigger`] with the variables.
///
/// Returns a [`Mutation`] whose input is the operation's `Variables` and
/// whose `data()` is the decoded `ResponseData`.
///
/// ```ignore
/// #[component]
/// fn add_book_form(client: GraphqlClient) -> Element {
///     let add = use_mutation::<AddBook>(&client);
///     ui! {
///         button(on_press = move || add.trigger(add_book::Variables { title: "…".into() }))
///             { text("Add") }
///     }
/// }
/// ```
pub fn use_mutation<Q>(client: &GraphqlClient) -> Mutation<Q::Variables, Q::ResponseData, GraphqlError>
where
    Q: GraphQLQuery,
    Q::Variables: 'static,
    Q::ResponseData: Clone + 'static,
{
    let client = client.clone();
    mutation(move |variables: Q::Variables| {
        let client = client.clone();
        async move { client.query::<Q>(variables).await }
    })
}
