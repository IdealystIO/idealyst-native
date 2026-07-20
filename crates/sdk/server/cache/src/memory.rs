//! In-process [`Cache`]: a map with lazy expiry. Right for tests and
//! single-instance dev; multi-instance deployments want `RedisCache`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::{Cache, CacheFuture};

/// Above this many live entries, expired ones are swept on insert (the
/// same lazy-eviction shape as server-kit's bucket store).
const SWEEP_THRESHOLD: usize = 10_000;

#[derive(Default)]
pub struct MemoryCache {
    entries: Mutex<HashMap<String, (Vec<u8>, Option<Instant>)>>,
}

impl MemoryCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn is_live(expiry: &Option<Instant>, now: Instant) -> bool {
        expiry.map(|at| at > now).unwrap_or(true)
    }
}

impl Cache for MemoryCache {
    fn get<'a>(&'a self, key: &'a str) -> CacheFuture<'a, Option<Vec<u8>>> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        let value = match entries.get(key) {
            Some((bytes, expiry)) if Self::is_live(expiry, now) => Some(bytes.clone()),
            Some(_) => {
                entries.remove(key); // expired — evict on read
                None
            }
            None => None,
        };
        Box::pin(async move { Ok(value) })
    }

    fn set<'a>(
        &'a self,
        key: &'a str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> CacheFuture<'a, ()> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap();
        if entries.len() > SWEEP_THRESHOLD {
            entries.retain(|_, (_, expiry)| Self::is_live(expiry, now));
        }
        entries.insert(key.to_string(), (value, ttl.map(|t| now + t)));
        Box::pin(async { Ok(()) })
    }

    fn delete<'a>(&'a self, key: &'a str) -> CacheFuture<'a, ()> {
        self.entries.lock().unwrap().remove(key);
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CacheExt;

    #[tokio::test]
    async fn set_get_delete_round_trip() {
        let c = MemoryCache::new();
        assert_eq!(c.get("k").await.unwrap(), None);
        c.set("k", b"v".to_vec(), None).await.unwrap();
        assert_eq!(c.get("k").await.unwrap(), Some(b"v".to_vec()));
        c.delete("k").await.unwrap();
        assert_eq!(c.get("k").await.unwrap(), None);
    }

    #[tokio::test]
    async fn expired_entries_read_as_miss() {
        let c = MemoryCache::new();
        // A zero TTL is already expired by read time.
        c.set("k", b"v".to_vec(), Some(Duration::ZERO)).await.unwrap();
        assert_eq!(c.get("k").await.unwrap(), None);
        // No TTL never expires.
        c.set("k2", b"v".to_vec(), None).await.unwrap();
        assert_eq!(c.get("k2").await.unwrap(), Some(b"v".to_vec()));
    }

    #[tokio::test]
    async fn json_helpers_round_trip_and_treat_bad_payload_as_miss() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Dash {
            count: u32,
        }
        let c = MemoryCache::new();
        c.set_json("d", &Dash { count: 7 }, None).await.unwrap();
        assert_eq!(c.get_json::<Dash>("d").await.unwrap(), Some(Dash { count: 7 }));
        // A schema change (undecodable payload) is a MISS, not an error.
        c.set("d", b"not json".to_vec(), None).await.unwrap();
        assert_eq!(c.get_json::<Dash>("d").await.unwrap(), None);
    }
}
