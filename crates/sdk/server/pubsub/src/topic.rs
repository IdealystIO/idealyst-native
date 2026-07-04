//! `Topic<T>` — a typed, named pub/sub channel.
//!
//! Pub/sub needs no macro (unlike `jobs`, whose `#[job]` exists so the worker
//! can *discover* handlers via `inventory`): every subscriber is an explicit
//! call site. A typed handle is the whole surface.
//!
//! ```ignore
//! const ROOM: pubsub::Topic<Msg> = pubsub::Topic::new("room");
//! ROOM.publish(&msg).await?;
//! let stream = ROOM.subscribe();           // impl Stream<Item = Msg>
//! ```

use crate::PubSubError;
use futures_util::Stream;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::borrow::Cow;
use std::marker::PhantomData;

/// A typed handle to a named topic. `const`-constructible for a static topic,
/// or [`Topic::named`] for a runtime-computed name (`format!("user:{id}")`).
///
/// `PhantomData<fn() -> T>` keeps `Topic<T>: Send + Sync + 'static` regardless
/// of `T` — the type parameter only constrains the publish/subscribe methods.
pub struct Topic<T> {
    name: Cow<'static, str>,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Topic<T> {
    /// A topic with a static name — usable in a `const`.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name: Cow::Borrowed(name),
            _marker: PhantomData,
        }
    }

    /// A topic with a runtime-computed name.
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: Cow::Owned(name.into()),
            _marker: PhantomData,
        }
    }

    /// The topic's name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl<T> Topic<T>
where
    T: Serialize + DeserializeOwned + 'static,
{
    /// Broadcast a message to every current subscriber.
    pub async fn publish(&self, msg: &T) -> Result<(), PubSubError> {
        crate::config::publish(&self.name, msg).await
    }

    /// Subscribe — yields each subsequently published `T`. Drops straight into a
    /// `#[subscription]` body (see the crate docs).
    pub fn subscribe(&self) -> impl Stream<Item = T> {
        crate::config::subscribe::<T>(self.name.to_string())
    }
}

impl<T> Clone for Topic<T> {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            _marker: PhantomData,
        }
    }
}
