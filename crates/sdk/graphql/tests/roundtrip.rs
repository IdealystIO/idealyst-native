//! End-to-end client→transport→schema→typed-response tests.
//!
//! These build a real `async-graphql` schema and drive `graphql_client`-
//! derived operations through a `GraphqlClient` over an in-process mock
//! `Transport` (no network). They exercise the full typed path the SDK
//! promises, deterministically. Requires `--features server` for the engine.
#![cfg(feature = "server")]

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use graphql::async_graphql::{Context, EmptySubscription, Object, SimpleObject, ID};
use graphql::{
    GraphqlClient, GraphqlError, GraphqlRequest, GraphqlResponse, Schema, Transport, TransportFuture,
};
use graphql_client::GraphQLQuery;

// --- The server schema (async-graphql) -------------------------------------

#[derive(Clone, SimpleObject)]
struct Book {
    id: ID,
    title: String,
    author: String,
}

type Db = Arc<Mutex<Vec<Book>>>;

struct Query;

#[Object]
impl Query {
    async fn books(&self, ctx: &Context<'_>) -> Vec<Book> {
        ctx.data_unchecked::<Db>().lock().unwrap().clone()
    }

    async fn book(&self, ctx: &Context<'_>, id: ID) -> Option<Book> {
        ctx.data_unchecked::<Db>()
            .lock()
            .unwrap()
            .iter()
            .find(|b| b.id == id)
            .cloned()
    }

    async fn fail(&self) -> graphql::async_graphql::Result<String> {
        Err(graphql::async_graphql::Error::new("intentional boom"))
    }
}

struct Mutation;

#[Object]
impl Mutation {
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

type BookSchema = Schema<Query, Mutation, EmptySubscription>;

fn build_schema() -> BookSchema {
    let db: Db = Arc::new(Mutex::new(vec![Book {
        id: ID("1".into()),
        title: "The Rust Programming Language".into(),
        author: "Klabnik & Nichols".into(),
    }]));
    Schema::build(Query, Mutation, EmptySubscription)
        .data(db)
        .finish()
}

// --- The mock transport: runs requests straight against the schema ---------

struct MockTransport {
    schema: BookSchema,
}

impl Transport for MockTransport {
    fn execute(&self, request: GraphqlRequest) -> TransportFuture<'_, GraphqlResponse> {
        Box::pin(async move { Ok(graphql::execute_request(&self.schema, request).await) })
    }
}

fn client() -> GraphqlClient {
    GraphqlClient::new(Rc::new(MockTransport {
        schema: build_schema(),
    }))
}

// --- The typed operations (graphql_client codegen) -------------------------

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "tests/schema.graphql",
    query_path = "tests/operations.graphql",
    response_derives = "Debug, Clone, PartialEq"
)]
struct GetBooks;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "tests/schema.graphql",
    query_path = "tests/operations.graphql",
    response_derives = "Debug, Clone, PartialEq"
)]
struct GetBook;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "tests/schema.graphql",
    query_path = "tests/operations.graphql",
    response_derives = "Debug, Clone, PartialEq"
)]
struct GetFail;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "tests/schema.graphql",
    query_path = "tests/operations.graphql",
    response_derives = "Debug, Clone, PartialEq"
)]
struct AddBook;

// --- Tests -----------------------------------------------------------------

#[tokio::test]
async fn query_roundtrip() {
    let client = client();
    let data = client
        .query::<GetBooks>(get_books::Variables)
        .await
        .expect("query should succeed");
    assert_eq!(data.books.len(), 1);
    assert_eq!(data.books[0].title, "The Rust Programming Language");
}

#[tokio::test]
async fn mutation_roundtrip() {
    let client = client();
    let added = client
        .mutate::<AddBook>(add_book::Variables {
            title: "Programming Rust".into(),
            author: "Blandy, Orendorff & Tindall".into(),
        })
        .await
        .expect("mutation should succeed");
    assert_eq!(added.add_book.title, "Programming Rust");

    // The new book is visible to a subsequent query on the same schema.
    let data = client
        .query::<GetBooks>(get_books::Variables)
        .await
        .expect("query should succeed");
    assert_eq!(data.books.len(), 2);
    assert!(data.books.iter().any(|b| b.title == "Programming Rust"));
}

#[tokio::test]
async fn graphql_errors_surface() {
    let client = client();
    let err = client
        .query::<GetFail>(get_fail::Variables)
        .await
        .expect_err("a resolver error must surface, not be swallowed");
    match err {
        GraphqlError::Query(errors) => {
            assert_eq!(errors.len(), 1);
            assert!(errors[0].message.contains("intentional boom"));
        }
        other => panic!("expected GraphqlError::Query, got {other:?}"),
    }
}

#[tokio::test]
async fn variables_and_operation_name_select_the_right_op() {
    let client = client();
    // The document declares four operations; `operationName` must select
    // `GetBook` and the `$id` variable must bind.
    let data = client
        .query::<GetBook>(get_book::Variables { id: "1".into() })
        .await
        .expect("query should succeed");
    let book = data.book.expect("book 1 exists");
    assert_eq!(book.id, "1");
    assert_eq!(book.author, "Klabnik & Nichols");

    // A missing id resolves to null without erroring.
    let missing = client
        .query::<GetBook>(get_book::Variables { id: "999".into() })
        .await
        .expect("query should succeed");
    assert!(missing.book.is_none());
}

#[test]
fn request_serializes_to_canonical_graphql_body() {
    // The wire body must be exactly `{ query, variables, operationName }`
    // — the shape any spec-compliant GraphQL endpoint expects.
    let request = GraphqlRequest::from_query::<GetBook>(get_book::Variables { id: "42".into() })
        .expect("request builds");
    let json = serde_json::to_value(&request).expect("serializes");

    assert_eq!(json["operationName"], "GetBook");
    assert_eq!(json["variables"]["id"], "42");
    assert!(
        json["query"].as_str().unwrap().contains("query GetBook"),
        "query text should carry the operation, got: {}",
        json["query"]
    );
}
