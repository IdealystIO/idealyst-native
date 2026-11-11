//! `server::mock`: a `#[server]` call answered without a server.
//!
//! Client mode only (`cargo test -p server --test mock`): with the
//! `server` feature a `#[server]` fn runs in-process and never reaches
//! the client stub a mock stands in for.
//!
//! No base URL is configured and nothing listens anywhere, so a call that
//! got past its mock would come back as an error rather than the canned
//! value. The registry is process-wide and tests run in parallel, so each
//! test mocks functions of its own.

#![cfg(not(feature = "server"))]

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use serde::{Deserialize, Serialize};
use server::{server, ServerError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: u32,
    pub name: String,
}

#[server]
pub async fn get_user(id: u32) -> Result<User, ServerError> {
    unreachable!("the client stub never runs the body: {id}")
}

#[server]
pub async fn whoami() -> Result<User, ServerError> {
    unreachable!()
}

#[server]
pub async fn rename_user(id: u32, name: String) -> Result<User, ServerError<String>> {
    unreachable!("{id} {name}")
}

#[server]
pub async fn slow_report() -> Result<u32, ServerError> {
    unreachable!()
}

#[server(path = "accounts/balance")]
pub async fn balance(account: u32) -> Result<i64, ServerError> {
    unreachable!("{account}")
}

#[server]
pub async fn mistyped(id: u32) -> Result<User, ServerError> {
    unreachable!("{id}")
}

#[server]
pub async fn unmocked_then_removed() -> Result<u32, ServerError> {
    unreachable!()
}

#[tokio::test]
async fn a_mocked_call_resolves_with_the_handlers_value() {
    server::mock::on("get_user", |(id,): (u32,)| -> Result<User, ServerError> {
        Ok(User { id, name: "Ada".into() })
    });
    assert_eq!(get_user(7).await.unwrap(), User { id: 7, name: "Ada".into() });
}

#[tokio::test]
async fn a_zero_argument_fn_takes_the_unit_tuple() {
    server::mock::on("whoami", |()| -> Result<User, ServerError> { Ok(User { id: 1, name: "Me".into() }) });
    assert_eq!(whoami().await.unwrap().name, "Me");
}

/// The error branch of whatever awaits the call is shown by returning the
/// error from the handler: it crosses as the function's own return type.
#[tokio::test]
async fn a_handler_error_arrives_as_the_fns_error() {
    server::mock::on("rename_user", |(_, name): (u32, String)| -> Result<User, ServerError<String>> {
        Err(ServerError::Failed(format!("`{name}` is taken")))
    });
    match rename_user(1, "ada".into()).await {
        Err(ServerError::Failed(message)) => assert_eq!(message, "`ada` is taken"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_pending_mock_never_resolves() {
    server::mock::pending("slow_report");
    let mut call = pin!(slow_report());
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..3 {
        assert!(matches!(call.as_mut().poll(&mut cx), Poll::Pending));
    }
}

#[tokio::test]
async fn a_mock_is_keyed_by_the_wire_path() {
    server::mock::on("accounts/balance", |(account,): (u32,)| -> Result<i64, ServerError> { Ok(account as i64 * 100) });
    assert_eq!(balance(3).await.unwrap(), 300);
}

/// Handler types are checked when a call arrives. A handler written for
/// the wrong arguments resolves the call with a codec error that names
/// the mocked path, rather than panicking or going to the network.
#[tokio::test]
async fn a_mistyped_handler_is_a_codec_error_naming_the_path() {
    server::mock::on("mistyped", |(name,): (String,)| -> Result<User, ServerError> { Ok(User { id: 0, name }) });
    match mistyped(4).await {
        Err(ServerError::Codec(message)) => {
            assert!(message.contains("mock for `mistyped`: arguments:"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

/// Once removed, the call goes to the network again, which here has no
/// server behind it.
#[tokio::test]
async fn a_removed_mock_no_longer_answers() {
    server::mock::on("unmocked_then_removed", |()| -> Result<u32, ServerError> { Ok(5) });
    assert_eq!(unmocked_then_removed().await.unwrap(), 5);
    server::mock::remove("unmocked_then_removed");
    assert!(unmocked_then_removed().await.is_err());
}
