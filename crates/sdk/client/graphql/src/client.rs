//! The typed GraphQL client — a thin, transport-agnostic layer over
//! [`Transport`](crate::Transport) that speaks `graphql_client`-derived
//! query types.

use crate::{GraphqlError, GraphqlRequest, GraphqlResponse, HttpTransport, Transport};
use std::future::Future;
use std::rc::Rc;

/// A cheap (`Clone`) handle that executes GraphQL operations through a
/// pluggable [`Transport`].
///
/// ```ignore
/// // server-functions path (auth/CSRF/credentials reused):
/// graphql::graphql_transport!(AppGraphql, my_graphql_server_fn);
/// let client = graphql::GraphqlClient::new(std::rc::Rc::new(AppGraphql));
///
/// // or any GraphQL endpoint, no server functions:
/// let client = graphql::GraphqlClient::http("https://api.example.com/graphql");
///
/// // typed call (graphql_client derive):
/// let data = client.query::<GetBook>(get_book::Variables { id }).await?;
/// ```
pub struct GraphqlClient {
    transport: Rc<dyn Transport>,
}

impl Clone for GraphqlClient {
    fn clone(&self) -> Self {
        Self {
            transport: self.transport.clone(),
        }
    }
}

impl GraphqlClient {
    /// Build a client over any [`Transport`].
    pub fn new(transport: Rc<dyn Transport>) -> Self {
        Self { transport }
    }

    /// Build a client that POSTs to a GraphQL endpoint URL over the `net`
    /// SDK (the no-server-functions path).
    pub fn http(url: impl Into<String>) -> Self {
        Self::new(Rc::new(HttpTransport::new(url)))
    }

    /// Execute a raw [`GraphqlRequest`], returning the untyped envelope.
    /// The escape hatch for dynamically built operations.
    pub fn execute(
        &self,
        request: GraphqlRequest,
    ) -> impl Future<Output = Result<GraphqlResponse, GraphqlError>> + 'static {
        let transport = self.transport.clone();
        async move { transport.execute(request).await }
    }

    /// Run a typed operation (query or mutation) and decode its `data` into
    /// the derived `ResponseData`. Any GraphQL `errors` in the response
    /// surface as [`GraphqlError::Query`].
    ///
    /// The returned future is `'static` (it owns a clone of the transport),
    /// so it drops straight into `resource` / `mutation` / `spawn_async`.
    pub fn query<Q: graphql_client::GraphQLQuery>(
        &self,
        variables: Q::Variables,
    ) -> impl Future<Output = Result<Q::ResponseData, GraphqlError>> + 'static {
        let transport = self.transport.clone();
        // Build the request up front (consumes `variables`, so `Variables`
        // needs no `Clone` bound).
        let built = GraphqlRequest::from_query::<Q>(variables);
        async move {
            let request = built?;
            let response = transport.execute(request).await?;
            response.into_typed::<Q::ResponseData>()
        }
    }

    /// Alias for [`query`](GraphqlClient::query) that reads better at
    /// mutation call sites. GraphQL queries and mutations share the same
    /// POST transport; only the operation type in the document differs.
    pub fn mutate<Q: graphql_client::GraphQLQuery>(
        &self,
        variables: Q::Variables,
    ) -> impl Future<Output = Result<Q::ResponseData, GraphqlError>> + 'static {
        self.query::<Q>(variables)
    }

    /// Execute a pre-built request and decode into `D`. Used by the
    /// `use_query` hook, which builds the request once (eagerly) and then
    /// re-runs it on refetch without re-touching the variables.
    pub(crate) fn execute_typed<D: serde::de::DeserializeOwned>(
        &self,
        request: GraphqlRequest,
    ) -> impl Future<Output = Result<D, GraphqlError>> + 'static {
        let transport = self.transport.clone();
        async move {
            let response = transport.execute(request).await?;
            response.into_typed::<D>()
        }
    }
}
