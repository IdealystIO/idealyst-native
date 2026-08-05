//! Compatibility aliases for the wire replay client.
//!
//! [`WireBackend`](crate::WireBackend) is the wire replayer: it maps each
//! incoming [`wire::Command`] to one capability call on the platform
//! backend.
//!
//! ```text
//! Commands → WireBackend<B> → caps::*Ops on B → native UI
//! ```
//!
//! Historically the replayer was bounded on the `Backend` mega-trait and
//! reached the capability surface through a `CapsReplay<B>` adapter, so
//! the runtime-v2 embedding spelled itself
//! `WireBackend<CapsReplay<B>>` (aliased `NewCoreReplayClient<B>`) and
//! constructed itself with `WireBackend::new_newcore`. With the
//! mega-trait deleted, [`WireBackend`](crate::WireBackend) is bounded
//! DIRECTLY on `runtime_vocabulary::caps::AllCaps` and the adapter has
//! dissolved — but the two names survive here as aliases because the
//! generated web wrapper (`build-runtime-server` / `build-web`) spells
//! `dev_client::newcore::NewCoreReplayClient<WebBackend>` verbatim.

use runtime_vocabulary::caps;

use crate::{OutboundSender, WireBackend};

/// The replay client. Now just [`WireBackend`] — kept as an alias
/// because generated client wrappers name this path.
pub type NewCoreReplayClient<B> = WireBackend<B>;

impl<B: caps::AllCaps + 'static> WireBackend<B> {
    /// Alias of [`WireBackend::new`], kept for generated wrappers that
    /// still spell the old constructor.
    pub fn new_newcore(backend: B, outbound: impl Into<OutboundSender>) -> Self {
        WireBackend::new(backend, outbound)
    }
}
