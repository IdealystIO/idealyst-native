//! Server-tier publish/subscribe SDK — cross-instance broadcast / fan-out. See
//! `Cargo.toml` for the pitch.
//!
//! The broadcast sibling of `jobs`: `jobs` delivers each message to one consumer
//! (a durable work queue); `pubsub` fans a message out to every current
//! subscriber, fire-and-forget. Its headline use is **decentralized WebSocket
//! notifications** — a `#[subscription]` body returns a topic's stream, anything
//! server-side publishes to the topic, and a shared backend (redis/postgres)
//! carries events across instances so a client connected to one instance
//! receives events produced on another.
//!
//! ```ignore
//! use futures_util::Stream;
//!
//! const ROOM: pubsub::Topic<Msg> = pubsub::Topic::new("room");
//!
//! #[subscription]
//! async fn feed() -> impl Stream<Item = Msg> {
//!     ROOM.subscribe()
//! }
//!
//! #[server]
//! async fn say(text: String) -> Result<(), ServerError> {
//!     ROOM.publish(&Msg { text }).await.map_err(ServerError::failed)?;
//!     Ok(())
//! }
//! ```

mod backend;
mod config;
mod error;
mod topic;

pub use backend::PubSubBackend;
pub use config::{configure, configure_from_env, configured_backend, publish, subscribe};
pub use error::PubSubError;
pub use topic::Topic;

#[cfg(feature = "memory")]
mod memory;
#[cfg(feature = "memory")]
pub use memory::MemoryBackend;

#[cfg(feature = "redis")]
mod redis_backend;
#[cfg(feature = "redis")]
pub use redis_backend::RedisBackend;

#[cfg(feature = "postgres")]
mod postgres_backend;
#[cfg(feature = "postgres")]
pub use postgres_backend::PostgresBackend;
