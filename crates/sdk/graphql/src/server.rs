//! Server-side GraphQL execution, powered by [`async-graphql`].
//!
//! Compiled only under the `server` feature so client / wasm builds never
//! pull the engine. This layer is deliberately transport-agnostic: it turns
//! a [`GraphqlRequest`] into an `async_graphql::Request`, runs it against a
//! schema, and converts the result back into a [`GraphqlResponse`]. *How*
//! the request arrived is not its concern — call [`execute_request`] from a
//! `#[server]` fn (server-functions stack), from a hand-rolled axum handler,
//! or from anything else.
//!
//! ```ignore
//! // Define the schema with async-graphql as usual:
//! let schema = graphql::async_graphql::Schema::build(Query, Mutation, EmptySubscription)
//!     .data(db)
//!     .finish();
//!
//! // Run it through a server function (reuses auth/CSRF/credentials):
//! #[server]
//! pub async fn graphql(req: graphql::GraphqlRequest, schema: State<MySchema>, who: Auth<User>)
//!     -> Result<graphql::GraphqlResponse, ServerError> {
//!     // Inject the authenticated principal into the GraphQL context so
//!     // resolvers can read `ctx.data::<User>()`.
//!     Ok(graphql::execute_request_with(&schema, req, |r| r.data(who.0)).await)
//! }
//! ```

use crate::{GqlError, GqlLocation, GraphqlRequest, GraphqlResponse};

/// Re-export the engine so apps author their schema against the exact same
/// version this SDK links (`graphql::async_graphql::Object`, `Schema`, …).
pub use async_graphql;

/// Convenience alias for an `async-graphql` schema.
pub type Schema<Q, M, S> = async_graphql::Schema<Q, M, S>;

/// Execute a [`GraphqlRequest`] against `schema` and return the canonical
/// [`GraphqlResponse`] envelope.
pub async fn execute_request<Q, M, S>(
    schema: &Schema<Q, M, S>,
    request: GraphqlRequest,
) -> GraphqlResponse
where
    Q: async_graphql::ObjectType + 'static,
    M: async_graphql::ObjectType + 'static,
    S: async_graphql::SubscriptionType + 'static,
{
    execute_request_with(schema, request, |r| r).await
}

/// Like [`execute_request`], but lets you customize the underlying
/// `async_graphql::Request` first — typically to inject per-request context
/// data (`|r| r.data(principal)`) that resolvers read via `ctx.data::<T>()`.
pub async fn execute_request_with<Q, M, S, F>(
    schema: &Schema<Q, M, S>,
    request: GraphqlRequest,
    customize: F,
) -> GraphqlResponse
where
    Q: async_graphql::ObjectType + 'static,
    M: async_graphql::ObjectType + 'static,
    S: async_graphql::SubscriptionType + 'static,
    F: FnOnce(async_graphql::Request) -> async_graphql::Request,
{
    let mut gql = async_graphql::Request::new(request.query);
    if let Some(variables) = request.variables {
        gql = gql.variables(async_graphql::Variables::from_json(variables));
    }
    if let Some(name) = request.operation_name {
        gql = gql.operation_name(name);
    }
    gql = customize(gql);

    convert_response(schema.execute(gql).await)
}

/// Export the schema's SDL (the GraphQL type system definition). Feed this
/// to `graphql_client`'s `schema_path` so the client derives typed
/// operations against the very schema the server runs.
pub fn sdl<Q, M, S>(schema: &Schema<Q, M, S>) -> String
where
    Q: async_graphql::ObjectType + 'static,
    M: async_graphql::ObjectType + 'static,
    S: async_graphql::SubscriptionType + 'static,
{
    schema.sdl()
}

fn convert_response(resp: async_graphql::Response) -> GraphqlResponse {
    let data = match serde_json::to_value(&resp.data) {
        Ok(serde_json::Value::Null) | Err(_) => None,
        Ok(value) => Some(value),
    };
    let errors = resp.errors.iter().map(convert_error).collect();
    // Drop an absent/empty extensions map; keep anything substantive.
    let extensions = serde_json::to_value(&resp.extensions)
        .ok()
        .filter(|v| match v {
            serde_json::Value::Null => false,
            serde_json::Value::Object(map) => !map.is_empty(),
            _ => true,
        });

    GraphqlResponse {
        data,
        errors,
        extensions,
    }
}

fn convert_error(err: &async_graphql::ServerError) -> GqlError {
    let path = match serde_json::to_value(&err.path) {
        Ok(serde_json::Value::Array(segments)) => segments,
        _ => Vec::new(),
    };
    GqlError {
        message: err.message.clone(),
        path,
        locations: err
            .locations
            .iter()
            .map(|pos| GqlLocation {
                line: pos.line as u32,
                column: pos.column as u32,
            })
            .collect(),
        extensions: err
            .extensions
            .as_ref()
            .and_then(|ext| serde_json::to_value(ext).ok()),
    }
}
