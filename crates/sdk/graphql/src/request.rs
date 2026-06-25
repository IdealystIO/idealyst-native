//! The GraphQL wire envelope — request and response shapes shared by the
//! client and server halves.
//!
//! These types are intentionally codec-agnostic: a [`GraphqlRequest`]
//! serializes to the canonical `{ "query", "variables", "operationName" }`
//! HTTP-GraphQL body, and a [`GraphqlResponse`] deserializes the canonical
//! `{ "data", "errors", "extensions" }` envelope. They carry untyped
//! `serde_json::Value` payloads; the [`crate::GraphqlClient`] layers the
//! `graphql_client`-derived types on top.

use serde::{Deserialize, Serialize};

/// A GraphQL operation to execute: the document text, its variables, and an
/// optional operation name (required only when the document declares more
/// than one operation).
///
/// Serializes to the canonical HTTP-GraphQL POST body
/// (`{ "query", "variables", "operationName" }`), so it interops with any
/// spec-compliant GraphQL endpoint, not just this SDK's server side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphqlRequest {
    /// The GraphQL document (one or more operations).
    pub query: String,
    /// Operation variables. `None` omits the field entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<serde_json::Value>,
    /// Which operation to run, when the document declares several.
    #[serde(
        rename = "operationName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub operation_name: Option<String>,
}

impl GraphqlRequest {
    /// Build a request from a raw document string and JSON variables.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            variables: None,
            operation_name: None,
        }
    }

    /// Attach variables (any `Serialize` value).
    pub fn with_variables<V: Serialize>(mut self, vars: &V) -> Result<Self, GraphqlError> {
        self.variables =
            Some(serde_json::to_value(vars).map_err(|e| GraphqlError::Encode(e.to_string()))?);
        Ok(self)
    }

    /// Set the operation name.
    pub fn with_operation_name(mut self, name: impl Into<String>) -> Self {
        self.operation_name = Some(name.into());
        self
    }

    /// Build a request from a `graphql_client`-derived query type and its
    /// variables. Serializes the generated [`graphql_client::QueryBody`]
    /// into this canonical envelope.
    pub fn from_query<Q: graphql_client::GraphQLQuery>(
        variables: Q::Variables,
    ) -> Result<Self, GraphqlError> {
        let body = Q::build_query(variables);
        Ok(Self {
            query: body.query.to_string(),
            variables: Some(
                serde_json::to_value(&body.variables)
                    .map_err(|e| GraphqlError::Encode(e.to_string()))?,
            ),
            operation_name: Some(body.operation_name.to_string()),
        })
    }
}

/// The GraphQL response envelope: a `data` payload and/or a list of
/// `errors`, plus optional `extensions`.
///
/// Per the GraphQL spec a response may carry *both* partial `data` and
/// `errors`. [`into_typed`](GraphqlResponse::into_typed) takes the strict
/// stance — any `errors` present surfaces as [`GraphqlError::Query`] rather
/// than silently returning partial data (see the SDK's crash-loud posture).
/// Callers that want partial data can read the fields directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphqlResponse {
    /// The operation result, if execution produced one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Field/validation/execution errors, in spec shape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<GqlError>,
    /// Implementation-defined extensions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Value>,
}

impl GraphqlResponse {
    /// Strictly decode the `data` payload into `D`. Returns
    /// [`GraphqlError::Query`] if the response carries any `errors`, and
    /// [`GraphqlError::Decode`] if `data` is absent or shaped wrong.
    pub fn into_typed<D: serde::de::DeserializeOwned>(self) -> Result<D, GraphqlError> {
        if !self.errors.is_empty() {
            return Err(GraphqlError::Query(self.errors));
        }
        match self.data {
            Some(value) => {
                serde_json::from_value(value).map_err(|e| GraphqlError::Decode(e.to_string()))
            }
            None => Err(GraphqlError::Decode("response contained no data".into())),
        }
    }
}

/// A single GraphQL error in the spec's shape (`message`, `path`,
/// `locations`, `extensions`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GqlError {
    /// Human-readable description of the error.
    pub message: String,
    /// The response-key path to the field that errored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<serde_json::Value>,
    /// Source locations in the query document.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<GqlLocation>,
    /// Implementation-defined error extensions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Value>,
}

/// A line/column source location inside a GraphQL document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GqlLocation {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number.
    pub column: u32,
}

/// Everything that can go wrong executing a GraphQL operation through the
/// SDK. `Clone` so it can flow through `resource`/`mutation` (whose error
/// type must be `Clone`).
#[derive(Debug, Clone, thiserror::Error)]
pub enum GraphqlError {
    /// The transport failed to deliver the request or read the response
    /// (network error, non-2xx HTTP, server-fn error, …).
    #[error("graphql transport error: {0}")]
    Transport(String),
    /// The server returned GraphQL `errors` (validation or resolver
    /// failures). Carries them verbatim.
    #[error("graphql returned {} error(s): {}", .0.len(), first_message(.0))]
    Query(Vec<GqlError>),
    /// The variables could not be serialized into the request.
    #[error("graphql request encode error: {0}")]
    Encode(String),
    /// The `data` payload could not be decoded into the expected type.
    #[error("graphql response decode error: {0}")]
    Decode(String),
}

fn first_message(errors: &[GqlError]) -> &str {
    errors.first().map(|e| e.message.as_str()).unwrap_or("")
}
