//! A client crate compiled in the same cargo invocation as the server sees
//! the API crate WITH `server` on (Cargo unifies features across a
//! workspace). Its call sites must still compile against the wire
//! signature.
//!
//! ## The bug this pins
//!
//! The macros emitted the server body under the author's name with
//! `feature = "server"`, and the client stub only without it. A `#[server]`
//! fn with an extractor (`Auth<_>`, `State<_>`) then had a different
//! signature in a unified build, so every client call was one argument short
//! (`cargo check --workspace`: 57 × E0061 in Idea Chat), and a `#[channel]` /
//! `#[subscription]` / `#[sse]` client hook did not exist at all — its name
//! was the server body.
//!
//! In client mode (`cargo test -p server`) this file is trivially true; the
//! regression is caught in server mode (`--features server`), where it fails
//! to COMPILE if a client-shaped call stops resolving. The runtime check
//! covers the in-process call `#[server]` now makes on the server.

use server::{channel, server, sse, subscription, ServerError, Socket, State};

#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub struct Motd(pub String);
#[cfg(feature = "server")]
#[derive(Debug, Clone)]
pub struct NeverInstalled;

#[server]
pub async fn motd_for(who: String, motd: State<Motd>) -> Result<String, ServerError> {
    let Motd(m) = &*motd;
    Ok(format!("{m}, {who}"))
}

#[server]
pub async fn needs_absent(_s: State<NeverInstalled>) -> Result<u32, ServerError> {
    Ok(1)
}

#[channel]
pub async fn echo(mut ch: Socket<String, String>, _motd: State<Motd>) {
    while let Some(Ok(m)) = ch.recv().await {
        let _ = ch.send(m).await;
    }
}

#[subscription]
pub async fn counts(_motd: State<Motd>) -> impl futures_util::Stream<Item = u32> {
    futures_util::stream::iter([1u32, 2, 3])
}

#[sse]
pub async fn feed(_motd: State<Motd>) -> impl futures_util::Stream<Item = u32> {
    futures_util::stream::iter([1u32])
}

/// "Client crate" code: written against the wire signatures only. Never
/// run — it exists to be type-checked in both modes.
#[allow(dead_code)]
async fn client_call_sites() {
    let _: Result<String, ServerError> = motd_for("ada".to_string()).await;
    let _: Result<u32, ServerError> = needs_absent().await;
    let _: fn() -> server::UseSocket<String, String> = echo;
    let _: fn() -> server::UseSocket<u32, ()> = counts;
    let _: fn() -> String = feed;
}

/// On the server, `#ident(wire args)` runs in-process: extractors resolve
/// from the ambient context, and one that can't is reported like a remote
/// failure, not a panic.
#[cfg(feature = "server")]
#[tokio::test]
async fn regression_server_fn_keeps_its_wire_signature_under_the_server_feature() {
    server::install_state(Motd("hello".to_string()));
    assert_eq!(motd_for("ada".to_string()).await.unwrap(), "hello, ada");

    match needs_absent().await {
        Err(ServerError::Server { status: 500, .. }) => {}
        other => panic!("an unresolvable extractor must fail the call as a 500, got {other:?}"),
    }
}
