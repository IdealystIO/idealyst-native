//! Native WebSocket round-trip: the `net::WebSocket` client (sync
//! `tungstenite` on an I/O worker thread, no tokio) against a
//! tokio-tungstenite echo server. Proves connect / send / recv / close
//! and the futures-channel bridge from the worker thread into async.

#![cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;

/// Spawn an echo server; returns its `ws://` address.
async fn echo_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut ws = match tokio_tungstenite::accept_async(stream).await {
                    Ok(ws) => ws,
                    Err(_) => return,
                };
                while let Some(Ok(msg)) = ws.next().await {
                    if msg.is_text() || msg.is_binary() {
                        if ws.send(msg).await.is_err() {
                            break;
                        }
                    } else if msg.is_close() {
                        break;
                    }
                }
            });
        }
    });
    format!("ws://{addr}")
}

#[tokio::test]
async fn round_trips_text_and_binary() {
    let url = echo_server().await;
    let mut sock = net::WebSocket::connect(&url)
        .await
        .expect("connect should succeed");

    sock.send(net::WsMessage::Text("hello".into())).unwrap();
    let got = sock.recv().await.expect("a message").expect("not an error");
    assert_eq!(got, net::WsMessage::Text("hello".into()));

    sock.send(net::WsMessage::Binary(vec![1, 2, 3])).unwrap();
    let got = sock.recv().await.expect("a message").expect("not an error");
    assert_eq!(got, net::WsMessage::Binary(vec![1, 2, 3]));
}

#[tokio::test]
async fn many_messages_preserve_order() {
    let url = echo_server().await;
    let mut sock = net::WebSocket::connect(&url).await.unwrap();

    for i in 0..20u32 {
        sock.send(net::WsMessage::Text(format!("msg-{i}"))).unwrap();
    }
    for i in 0..20u32 {
        let got = sock.recv().await.expect("a message").expect("not an error");
        assert_eq!(got, net::WsMessage::Text(format!("msg-{i}")));
    }
}

#[tokio::test]
async fn recv_yields_none_after_close() {
    let url = echo_server().await;
    let mut sock = net::WebSocket::connect(&url).await.unwrap();

    sock.send(net::WsMessage::Text("ping".into())).unwrap();
    let _ = sock.recv().await;

    sock.close();
    // After close, recv must eventually report the connection is gone
    // (None), not hang. Bound it so a regression fails fast.
    let drained = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match sock.recv().await {
                None => break true,            // closed — the expected end state
                Some(Ok(_)) => continue,       // a straggler echo; keep draining
                Some(Err(_)) => break true,    // transport error also ends the stream
            }
        }
    })
    .await;
    assert_eq!(drained, Ok(true), "recv() must terminate after close()");
}

#[tokio::test]
async fn connect_error_surfaces() {
    // Nothing is listening on this port.
    let result = net::WebSocket::connect("ws://127.0.0.1:1").await;
    assert!(result.is_err(), "connecting to a dead port must error");
}

#[tokio::test]
async fn sender_sends_while_socket_recvs() {
    // A cloned WsSender can send while the WebSocket is parked in recv() —
    // the split the use_socket hook relies on (recv loop owns the socket,
    // the UI scope holds a sender).
    let url = echo_server().await;
    let mut sock = net::WebSocket::connect(&url).await.unwrap();
    let tx = sock.sender();

    // Drive recv concurrently with sends from the independent handle.
    let recv_task = tokio::spawn(async move {
        let mut got = Vec::new();
        for _ in 0..5 {
            match sock.recv().await {
                Some(Ok(net::WsMessage::Text(s))) => got.push(s),
                other => panic!("unexpected: {other:?}"),
            }
        }
        got
    });

    for i in 0..5u32 {
        tx.send(net::WsMessage::Text(format!("s{i}"))).unwrap();
    }

    let got = recv_task.await.unwrap();
    assert_eq!(got, vec!["s0", "s1", "s2", "s3", "s4"]);
}

#[tokio::test]
async fn sender_close_ends_recv_loop() {
    // The exact teardown `use_socket` performs on unmount: a held sender
    // closes the connection, which makes the recv loop's recv() return
    // None and the loop (and thus the spawned task) end.
    let url = echo_server().await;
    let mut sock = net::WebSocket::connect(&url).await.unwrap();
    let tx = sock.sender();

    let recv_loop = tokio::spawn(async move {
        loop {
            match sock.recv().await {
                None => break true,         // closed — the loop ends here
                Some(Ok(_)) => continue,    // straggler echo; keep draining
                Some(Err(_)) => break true, // error also ends the loop
            }
        }
    });

    tx.close(); // what on_cleanup calls via the CloseCoord

    let ended = tokio::time::timeout(Duration::from_secs(2), recv_loop).await;
    assert!(
        matches!(ended, Ok(Ok(true))),
        "sender.close() must end the recv loop"
    );
}

/// A server that answers the first message by closing with `code` +
/// `reason` (or, with `code == None`, by dropping the TCP connection
/// without any close frame).
async fn closing_server(code: Option<u16>, reason: &'static str) -> String {
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    use tokio_tungstenite::tungstenite::Message;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = ws.next().await;
        match code {
            Some(code) => {
                let _ = ws
                    .send(Message::Close(Some(CloseFrame {
                        code: CloseCode::from(code),
                        reason: reason.into(),
                    })))
                    .await;
                // Let the client's echo arrive before the socket drops.
                let _ = tokio::time::timeout(Duration::from_secs(1), ws.next()).await;
            }
            None => drop(ws), // TCP gone, no close frame
        }
    });
    format!("ws://{addr}")
}

/// Read until the stream ends, bounded so a regression fails fast.
async fn drain(sock: &mut net::WebSocket) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(msg) = sock.recv().await {
            if msg.is_err() {
                // A transport error precedes the end; keep draining.
                continue;
            }
        }
    })
    .await
    .expect("recv() must end once the peer closes");
}

/// The peer's close code and reason are what `close_status` reports —
/// the voice-transcription client tells a finished session (1000) from a
/// failure by exactly this, and reading it off the event was the one
/// thing that kept it on raw `web_sys::WebSocket`.
#[tokio::test]
async fn close_status_reports_the_peers_code_and_reason() {
    let url = closing_server(Some(4001), "ticket expired").await;
    let mut sock = net::WebSocket::connect(&url).await.unwrap();
    assert_eq!(sock.close_status(), None, "open socket has no close status");

    sock.send(net::WsMessage::Text("hi".into())).unwrap();
    drain(&mut sock).await;

    assert_eq!(
        sock.close_status(),
        Some(net::WsClose {
            code: 4001,
            reason: "ticket expired".into()
        })
    );
    assert!(!sock.close_status().unwrap().is_normal());
}

#[tokio::test]
async fn close_status_normal_closure_is_normal() {
    let url = closing_server(Some(net::WsClose::NORMAL), "").await;
    let mut sock = net::WebSocket::connect(&url).await.unwrap();
    sock.send(net::WsMessage::Text("hi".into())).unwrap();
    drain(&mut sock).await;
    assert!(sock.close_status().expect("closed").is_normal());
}

/// No close frame at all — what a browser reports as 1006. The native
/// arm synthesizes the same code so an app reads one number everywhere.
#[tokio::test]
async fn close_status_without_a_close_frame_is_abnormal() {
    let url = closing_server(None, "").await;
    let mut sock = net::WebSocket::connect(&url).await.unwrap();
    sock.send(net::WsMessage::Text("hi".into())).unwrap();
    drain(&mut sock).await;
    assert_eq!(
        sock.close_status().map(|c| c.code),
        Some(net::WsClose::ABNORMAL)
    );
}

/// A close WE started ends as 1005 (a close frame with no status), the
/// code a browser reports for `WebSocket.close()` with no arguments.
#[tokio::test]
async fn close_status_after_local_close_is_no_status() {
    let url = echo_server().await;
    let mut sock = net::WebSocket::connect(&url).await.unwrap();
    sock.close();
    drain(&mut sock).await;
    assert_eq!(
        sock.close_status().map(|c| c.code),
        Some(net::WsClose::NO_STATUS)
    );
}
