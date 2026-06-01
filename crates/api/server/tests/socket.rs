//! Typed `Socket<In, Out>` end-to-end, both feature modes:
//!
//! - server mode: a `server::accept` handler over axum, exercised by a
//!   raw `net::WebSocket` client speaking the JSON frames.
//! - client mode: the typed `Socket::connect` client against a raw
//!   `tokio-tungstenite` server.
//!
//! Both prove the same thing from opposite ends: the shared `ClientMsg`/
//! `ServerMsg` enums round-trip as the wire contract, with no protocol
//! beyond the types themselves.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum ClientMsg {
    Echo(String),
    Ping,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum ServerMsg {
    Echo(String),
    Pong,
}

fn reply_to(msg: ClientMsg) -> ServerMsg {
    match msg {
        ClientMsg::Echo(s) => ServerMsg::Echo(s),
        ClientMsg::Ping => ServerMsg::Pong,
    }
}

// A #[channel] endpoint. On the server build the macro generates the
// axum upgrade handler + auto-registers the route (server::router()
// mounts it at GET /_srv/_ws/echo_channel); on the client build it
// generates `fn echo_channel() -> UseSocket<ServerMsg, ClientMsg>`.
#[server::channel]
async fn echo_channel(
    mut ch: server::Socket<ClientMsg, ServerMsg>,
) -> Result<(), server::ServerError> {
    while let Some(Ok(msg)) = ch.recv().await {
        if ch.send(reply_to(msg)).await.is_err() {
            break;
        }
    }
    Ok(())
}

// A #[subscription] endpoint: server→client stream. The macro pumps the
// returned Stream into the socket (server build) and generates
// `fn counts() -> UseSocket<u32, ()>` (client build).
#[server::subscription]
async fn counts() -> impl futures_util::Stream<Item = u32> {
    futures_util::stream::iter(vec![1u32, 2, 3])
}

// Open (wire) args: a channel parameterized by a `prefix` sent in the
// connect URL. Client stub becomes `fn room_channel(prefix: String) -> …`.
#[server::channel]
async fn room_channel(
    mut ch: server::Socket<ClientMsg, ServerMsg>,
    prefix: String,
) -> Result<(), server::ServerError> {
    while let Some(Ok(msg)) = ch.recv().await {
        let reply = match reply_to(msg) {
            ServerMsg::Echo(s) => ServerMsg::Echo(format!("{prefix}:{s}")),
            other => other,
        };
        if ch.send(reply).await.is_err() {
            break;
        }
    }
    Ok(())
}

// A subscription parameterized by `start`.
#[server::subscription]
async fn count_from(start: u32) -> impl futures_util::Stream<Item = u32> {
    futures_util::stream::iter(vec![start, start + 1, start + 2])
}

// ===========================================================================
// Server mode: the typed `accept` handler, hit by a raw net::WebSocket.
// ===========================================================================

#[cfg(feature = "server")]
mod server_mode {
    use super::{reply_to, ClientMsg, ServerMsg};
    use axum::{extract::ws::WebSocketUpgrade, response::Response, routing::get, Router};
    use server::Socket;
    use tokio::net::TcpListener;

    async fn echo_ws(ws: WebSocketUpgrade) -> Response {
        server::accept(ws, |mut sock: Socket<ClientMsg, ServerMsg>| async move {
            while let Some(Ok(msg)) = sock.recv().await {
                if sock.send(reply_to(msg)).await.is_err() {
                    break;
                }
            }
        })
    }

    fn decode_text(msg: net::WsMessage) -> ServerMsg {
        let net::WsMessage::Text(json) = msg else {
            panic!("expected a text frame, got {msg:?}");
        };
        serde_json::from_str(&json).unwrap()
    }

    #[tokio::test]
    async fn typed_accept_handler_echoes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route("/_srv/_ws/echo", get(echo_ws));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut ws = net::WebSocket::connect(&format!("ws://{addr}/_srv/_ws/echo"))
            .await
            .expect("connect");

        ws.send(net::WsMessage::Text(
            serde_json::to_string(&ClientMsg::Echo("hi".into())).unwrap(),
        ))
        .unwrap();
        assert_eq!(
            decode_text(ws.recv().await.unwrap().unwrap()),
            ServerMsg::Echo("hi".into())
        );

        ws.send(net::WsMessage::Text(
            serde_json::to_string(&ClientMsg::Ping).unwrap(),
        ))
        .unwrap();
        assert_eq!(
            decode_text(ws.recv().await.unwrap().unwrap()),
            ServerMsg::Pong
        );
    }

    #[tokio::test]
    async fn channel_macro_mounts_route_and_echoes() {
        // server::router() folds the #[channel]'s WsEntry → mounts
        // GET /_srv/_ws/echo_channel. A raw client round-trips through it.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = server::router();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut ws = net::WebSocket::connect(&format!("ws://{addr}/_srv/_ws/echo_channel"))
            .await
            .expect("connect");

        ws.send(net::WsMessage::Text(
            serde_json::to_string(&ClientMsg::Echo("yo".into())).unwrap(),
        ))
        .unwrap();
        assert_eq!(
            decode_text(ws.recv().await.unwrap().unwrap()),
            ServerMsg::Echo("yo".into())
        );
    }

    #[tokio::test]
    async fn subscription_macro_streams_items_then_closes() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = server::router();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut ws = net::WebSocket::connect(&format!("ws://{addr}/_srv/_ws/counts"))
            .await
            .expect("connect");

        // The stream's three items arrive as messages, in order.
        let mut got = Vec::new();
        for _ in 0..3 {
            let net::WsMessage::Text(json) = ws.recv().await.unwrap().unwrap() else {
                panic!("expected text frame");
            };
            got.push(serde_json::from_str::<u32>(&json).unwrap());
        }
        assert_eq!(got, vec![1, 2, 3]);

        // Stream exhausted → the server drops the socket → recv ends.
        let end = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match ws.recv().await {
                    None => break true,
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => break true,
                }
            }
        })
        .await;
        assert_eq!(end, Ok(true), "stream end must close the subscription");
    }

    /// Hex-encode like the client stub does, so a raw client can pass
    /// open args in the connect URL.
    fn hex_args<T: serde::Serialize>(args: &T) -> String {
        serde_json::to_vec(args)
            .unwrap()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    async fn boot_router() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = server::router();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        addr
    }

    #[tokio::test]
    async fn channel_open_arg_reaches_handler() {
        let addr = boot_router().await;
        let args = hex_args(&("r1".to_string(),));
        let mut ws =
            net::WebSocket::connect(&format!("ws://{addr}/_srv/_ws/room_channel?args={args}"))
                .await
                .unwrap();
        ws.send(net::WsMessage::Text(
            serde_json::to_string(&ClientMsg::Echo("hi".into())).unwrap(),
        ))
        .unwrap();
        // The handler prefixed the echo with the open arg.
        assert_eq!(
            decode_text(ws.recv().await.unwrap().unwrap()),
            ServerMsg::Echo("r1:hi".into())
        );
    }

    #[tokio::test]
    async fn subscription_open_arg_parameterizes_stream() {
        let addr = boot_router().await;
        let args = hex_args(&(10u32,));
        let mut ws =
            net::WebSocket::connect(&format!("ws://{addr}/_srv/_ws/count_from?args={args}"))
                .await
                .unwrap();
        let mut got = Vec::new();
        for _ in 0..3 {
            let net::WsMessage::Text(json) = ws.recv().await.unwrap().unwrap() else {
                panic!("expected text");
            };
            got.push(serde_json::from_str::<u32>(&json).unwrap());
        }
        assert_eq!(got, vec![10, 11, 12]);
    }
}

// ===========================================================================
// Client mode: the typed Socket::connect, against a raw ws server.
// ===========================================================================

#[cfg(not(feature = "server"))]
mod client_mode {
    use super::{reply_to, ClientMsg, ServerMsg};
    use futures_util::{SinkExt, StreamExt};
    use server::Socket;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    /// A raw server that decodes `ClientMsg` and replies with `ServerMsg`.
    async fn typed_echo_server() -> String {
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
                        if let Message::Text(s) = msg {
                            let cm: ClientMsg = serde_json::from_str(&s).unwrap();
                            let reply = serde_json::to_string(&reply_to(cm)).unwrap();
                            if ws.send(Message::Text(reply)).await.is_err() {
                                break;
                            }
                        }
                    }
                });
            }
        });
        format!("ws://{addr}")
    }

    #[tokio::test]
    async fn typed_socket_round_trip() {
        let url = typed_echo_server().await;
        // Client receives ServerMsg, sends ClientMsg — mirror of the server.
        let mut sock = Socket::<ServerMsg, ClientMsg>::connect(&url)
            .await
            .expect("connect");

        sock.send(ClientMsg::Echo("hi".into())).await.unwrap();
        assert_eq!(
            sock.recv().await.unwrap().unwrap(),
            ServerMsg::Echo("hi".into())
        );

        sock.send(ClientMsg::Ping).await.unwrap();
        assert_eq!(sock.recv().await.unwrap().unwrap(), ServerMsg::Pong);
    }

    #[tokio::test]
    async fn recv_none_when_server_drops() {
        let url = typed_echo_server().await;
        let mut sock = Socket::<ServerMsg, ClientMsg>::connect(&url)
            .await
            .unwrap();
        // Round-trip once so the connection is established, then the test
        // server task stays up; closing our side ends recv promptly.
        sock.send(ClientMsg::Ping).await.unwrap();
        let _ = sock.recv().await;
        drop(sock); // Drop closes the connection — no hang, no panic.
    }
}
