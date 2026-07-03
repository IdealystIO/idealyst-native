//! Serialization helpers shared by the persistent backends (Redis / Postgres /
//! SQS). Only compiled when one of those features is on.
//!
//! [`Backoff`] is a public `Duration`-based enum (ergonomic to construct) but
//! `Duration` isn't `serde`-serializable, so persistent backends store this
//! millisecond mirror instead.

use crate::Backoff;
use std::time::Duration;

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct BackoffRepr {
    /// 0 = none, 1 = fixed, 2 = exponential.
    kind: u8,
    base_ms: u64,
    factor: f64,
    max_ms: u64,
}

impl BackoffRepr {
    pub(crate) fn of(b: &Backoff) -> Self {
        match b {
            Backoff::None => Self {
                kind: 0,
                base_ms: 0,
                factor: 0.0,
                max_ms: 0,
            },
            Backoff::Fixed(d) => Self {
                kind: 1,
                base_ms: d.as_millis() as u64,
                factor: 0.0,
                max_ms: 0,
            },
            Backoff::Exponential { base, factor, max } => Self {
                kind: 2,
                base_ms: base.as_millis() as u64,
                factor: *factor,
                max_ms: max.as_millis() as u64,
            },
        }
    }

    fn into_backoff(self) -> Backoff {
        match self.kind {
            1 => Backoff::Fixed(Duration::from_millis(self.base_ms)),
            2 => Backoff::Exponential {
                base: Duration::from_millis(self.base_ms),
                factor: self.factor,
                max: Duration::from_millis(self.max_ms),
            },
            _ => Backoff::None,
        }
    }

    /// Serialize a backoff to a compact JSON string for storage.
    pub(crate) fn to_json(b: &Backoff) -> String {
        serde_json::to_string(&Self::of(b)).unwrap_or_else(|_| "{}".to_string())
    }

    /// Parse a stored backoff JSON string, falling back to the default schedule
    /// if it's missing or malformed.
    pub(crate) fn from_json(s: &str) -> Backoff {
        serde_json::from_str::<Self>(s)
            .map(Self::into_backoff)
            .unwrap_or_default()
    }
}
