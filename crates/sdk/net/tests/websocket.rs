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
