//! `use_query` / `use_mutation` — the reactive hooks, same source on
//! BOTH cores.
//!
//! Old core (default): reactive state is ambient thread-local; the
//! hooks run bare and `spawn_async`'s native pollster fallback settles
//! the canned transport synchronously.
//!
//! New core (`--features new-core`): the crate-root alias in this test
//! file mirrors the lib's (`extern crate runtime_facade as
//! runtime_core;`), so the SAME assertions drive the glue
//! `resource`/`mutation` mirrors. Reactive state then needs a `World`:
//! the per-core shim below enters a fresh test world
//! (`__with_fresh_world`) and commits staged writes (`__flush_test_world`)
//! before asserting — the standard aliased-SDK dual-core test pattern
//! (idea-theme precedent).
//!
//! No `server` feature: a canned in-process `Transport` returns spec-
//! shaped JSON, so the suite exercises exactly the hook layer (request
//! build → transport → typed decode → reactive state transitions).

#[cfg(feature = "new-core")]
extern crate runtime_facade as runtime_core;

use std::cell::RefCell;
use std::rc::Rc;

use graphql::{
    use_mutation, use_query, GraphqlClient, GraphqlError, GraphqlRequest, GraphqlResponse,
    Transport, TransportFuture,
};
use graphql_client::GraphQLQuery;
use serde_json::json;

// --- Per-core reactive-scope shim ------------------------------------------

/// Run `f` where hook creation is legal on the active core. Old core:
/// identity (state is ambient). New core: inside a fresh entered
/// `World`, flushed before it drops.
fn with_reactive_scope<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(feature = "new-core")]
    {
        runtime_core::__with_fresh_world(f)
    }
    #[cfg(not(feature = "new-core"))]
    {
        f()
    }
}

/// Commit pending reactive work before asserting. Old core applies
/// writes synchronously → no-op. New core: flush the ambient test
/// world (twice is deliberate — a flush-triggered effect re-run spawns
/// a fetch whose pollster-synchronous completion stages writes that the
/// second flush commits).
fn settle() {
    #[cfg(feature = "new-core")]
    {
        assert!(runtime_core::__flush_test_world(), "test world must be live");
        let _ = runtime_core::__flush_test_world();
    }
}

// --- Typed operations (same schema/operations as tests/roundtrip.rs) -------

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
struct AddBook;

// --- Canned transport -------------------------------------------------------

/// Pops one canned result per request (front-first) and logs each
/// request's query string so tests can count fetches.
struct CannedTransport {
    responses: RefCell<Vec<Result<serde_json::Value, GraphqlError>>>,
    log: Rc<RefCell<Vec<String>>>,
}

impl Transport for CannedTransport {
    fn execute(&self, request: GraphqlRequest) -> TransportFuture<'_, GraphqlResponse> {
        self.log.borrow_mut().push(request.query.clone());
        let next = if self.responses.borrow().is_empty() {
            Err(GraphqlError::Transport("canned transport exhausted".into()))
        } else {
            self.responses.borrow_mut().remove(0)
        };
        Box::pin(async move {
            next.map(|data| GraphqlResponse {
                data: Some(data),
                errors: Vec::new(),
                extensions: None,
            })
        })
    }
}

fn canned(
    responses: Vec<Result<serde_json::Value, GraphqlError>>,
) -> (GraphqlClient, Rc<RefCell<Vec<String>>>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let client = GraphqlClient::new(Rc::new(CannedTransport {
        responses: RefCell::new(responses),
        log: log.clone(),
    }));
    (client, log)
}

fn book(id: &str, title: &str) -> serde_json::Value {
    json!({ "id": id, "title": title, "author": "A. Author" })
}

// --- Tests ------------------------------------------------------------------

/// `use_query` fetches eagerly on creation and exposes the decoded
/// `ResponseData`; `refetch()` re-runs the same operation.
#[test]
fn use_query_fetches_on_mount_and_refetches() {
    with_reactive_scope(|| {
        let (client, log) = canned(vec![
            Ok(json!({ "books": [book("1", "First")] })),
            Ok(json!({ "books": [book("1", "First"), book("2", "Second")] })),
        ]);
        let books = use_query::<GetBooks>(&client, get_books::Variables);
        settle();
        assert!(!books.loading(), "canned transport settles synchronously");
        assert!(books.error().is_none());
        let data = books.data().expect("decoded ResponseData");
        assert_eq!(data.books.len(), 1);
        assert_eq!(data.books[0].title, "First");
        assert_eq!(log.borrow().len(), 1);

        books.refetch();
        settle();
        let data = books.data().expect("refetched ResponseData");
        assert_eq!(data.books.len(), 2, "refetch re-ran the operation");
        assert_eq!(log.borrow().len(), 2, "exactly one more request issued");
    });
}

/// A failing operation surfaces through `error()` while `data` stays
/// whatever the last success produced (here: none).
#[test]
fn use_query_surfaces_transport_errors() {
    with_reactive_scope(|| {
        let (client, _log) = canned(vec![Err(GraphqlError::Transport("offline".into()))]);
        let books = use_query::<GetBooks>(&client, get_books::Variables);
        settle();
        assert!(!books.loading());
        assert_eq!(books.data(), None);
        match books.error() {
            Some(GraphqlError::Transport(msg)) => assert_eq!(msg, "offline"),
            other => panic!("expected transport error, got {other:?}"),
        }
    });
}

/// `use_mutation` runs nothing until triggered, then decodes the typed
/// response into `data()`.
#[test]
fn use_mutation_triggers_and_decodes() {
    with_reactive_scope(|| {
        let (client, log) = canned(vec![Ok(json!({ "addBook": book("2", "New Book") }))]);
        let add = use_mutation::<AddBook>(&client);
        settle();
        assert!(!add.loading(), "fresh mutation is idle");
        assert_eq!(add.data(), None);
        assert_eq!(log.borrow().len(), 0, "nothing runs before trigger");

        add.trigger(add_book::Variables {
            title: "New Book".into(),
            author: "A. Author".into(),
        });
        settle();
        assert_eq!(log.borrow().len(), 1);
        let data = add.data().expect("decoded mutation ResponseData");
        assert_eq!(data.add_book.title, "New Book");
        assert!(add.error().is_none());
        assert!(!add.loading());
    });
}
