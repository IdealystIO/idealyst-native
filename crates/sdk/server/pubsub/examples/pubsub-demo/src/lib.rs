//! `pubsub-demo` — **decentralized WebSocket fan-out** on the [`pubsub`] SDK.
//!
//! - `#[server] say(text)` publishes a `ChatMsg` to the `"room"` topic.
//! - `#[subscription] room_feed()` bridges that topic to a WebSocket:
//!   `pubsub::subscribe(...)` returns a stream the subscription macro pumps to
//!   the socket. Every connected client sees every message — and with a shared
//!   backend (redis/postgres via `dev.toml`), that holds **across server
//!   instances**: publish on instance A, deliver to a client on instance B.
//!
//! All `pubsub` references live behind `#[cfg(feature = "server")]`, so the
//! wasm client never compiles the SDK.
//!
//! ## Run it
//!
//! ```text
//! idealyst dev --web crates/sdk/server/pubsub/examples/pubsub-demo
//! ```
//! Open two tabs, type in one → both feeds update.

use std::rc::Rc;

use idea_ui::{
    install_idea_theme, light_theme, tone, typography_kind, variant, Button, Card, CardPadding,
    Field, Stack, StackAlign, StackAxis, StackGap, StackPadding, Typography,
};
use runtime_core::{component, signal, ui, Element};
use serde::{Deserialize, Serialize};
use server::{server, ServerError};

/// A chat message — the topic's payload type. Compiles in both builds (it's on
/// the wire); `Clone` is needed by the client subscription hook.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ChatMsg {
    pub from: String,
    pub text: String,
}

/// The shared topic. Server-only (const-constructed), so `pubsub` stays out of
/// the client build.
#[cfg(feature = "server")]
const ROOM: pubsub::Topic<ChatMsg> = pubsub::Topic::new("room");

/// Publish a message to the room. Returns immediately; the fan-out is handled
/// by the pub/sub backend.
#[server]
pub async fn say(from: String, text: String) -> Result<(), ServerError> {
    #[cfg(feature = "server")]
    ROOM.publish(&ChatMsg { from, text })
        .await
        .map_err(|e| ServerError::failed(format!("publish failed: {e}")))?;
    Ok(())
}

/// Live feed of room messages over a WebSocket. The body returns the topic's
/// stream; the subscription macro pumps each message to the connected socket.
/// (The macro compiles this body only on the server build; the client gets a
/// `UseSocket` stub.)
#[server::subscription]
pub async fn room_feed() -> impl futures_util::Stream<Item = ChatMsg> {
    ROOM.subscribe()
}

fn configure_server() {
    #[cfg(target_arch = "wasm32")]
    {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
        server::configure(server::ClientConfig::new(origin));
    }
    #[cfg(all(not(feature = "server"), not(target_arch = "wasm32")))]
    {
        server::configure(server::ClientConfig::new("http://127.0.0.1:3000"));
    }
}

/// Root component: a live feed (client-only WS subscription) + a send box.
#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());
    configure_server();

    let name = signal("guest".to_string());
    let text = signal(String::new());

    let on_name: Rc<dyn Fn(String)> = {
        let name = name.clone();
        Rc::new(move |v: String| name.set(v))
    };
    let on_text: Rc<dyn Fn(String)> = {
        let text = text.clone();
        Rc::new(move |v: String| text.set(v))
    };

    let submit: Rc<dyn Fn()> = {
        let name = name.clone();
        let text = text.clone();
        Rc::new(move || {
            let body = text.get();
            if body.trim().is_empty() {
                return;
            }
            let from = name.get();
            text.set(String::new());
            runtime_core::driver::spawn_async(async move {
                let _ = say(from, body).await;
            });
        })
    };

    // Live feed: consume the WS subscription and accumulate messages. Client
    // only — `app()` is compiled but never called on the server build, so the
    // server arm is a placeholder (matches server-fn-demo's pattern).
    #[cfg(not(feature = "server"))]
    let feed: Element = {
        use runtime_core::{effect, IntoElement};
        let live = room_feed();
        let log = signal(Vec::<String>::new());
        {
            let log = log.clone();
            effect!({
                if let Some(m) = live.latest() {
                    log.update(|v| v.push(format!("{}: {}", m.from, m.text)));
                }
            });
        }
        runtime_core::text(move || {
            let lines = log.get();
            if lines.is_empty() {
                "(waiting for messages…)".to_string()
            } else {
                lines.join("\n")
            }
        })
        .into_element()
    };
    #[cfg(feature = "server")]
    let feed: Element =
        ui! { Typography(content = "(live feed on client builds)".to_string(), muted = true) };

    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Lg, align = StackAlign::Center) {
            Typography(content = "PubSub Room".to_string(), kind = typography_kind::H1)
            Typography(
                content = "Messages fan out to every connected client via the pubsub backend \
                           — across server instances when a shared broker is configured.".to_string(),
                muted = true,
            )
            Card(padding = CardPadding::Lg) {
                Stack(gap = StackGap::Sm) {
                    feed
                }
            }
            Card(padding = CardPadding::Md) {
                Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::End) {
                    Field(label = Some("Name".to_string()), value = name, on_change = on_name)
                    Field(
                        label = Some("Message".to_string()),
                        value = text,
                        on_change = on_text,
                        placeholder = Some("say something…".to_string()),
                    )
                    Button(
                        label = "Send".to_string(),
                        on_click = submit,
                        tone = tone::Primary,
                        variant = variant::Filled,
                    )
                }
            }
        }
    }
}

/// SDK-registration hook the platform wrappers call before mount.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
