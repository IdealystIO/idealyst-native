//! Alternate names for the wire replay client.
//!
//! [`WireBackend`](crate::WireBackend) is the wire replayer: it maps each
//! incoming [`wire::Command`] to one capability call on the platform
//! backend.
//!
//! ```text
//! Commands → WireBackend<B> → caps::*Ops on B → native UI
//! ```
//!
//! [`WireBackend`](crate::WireBackend) is bounded directly on
//! `runtime_vocabulary::caps::AllCaps`. `NewCoreReplayClient<B>` and
//! `WireBackend::new_newcore` are plain aliases of it, still named by
//! `idealyst::boot::web` (`dev_client::newcore::NewCoreReplayClient<WebBackend>`).

use runtime_vocabulary::caps;

use crate::{OutboundSender, WireBackend};

/// The replay client — an alias of [`WireBackend`].
pub type NewCoreReplayClient<B> = WireBackend<B>;

impl<B: caps::AllCaps + 'static> WireBackend<B> {
    /// Alias of [`WireBackend::new`].
    pub fn new_newcore(backend: B, outbound: impl Into<OutboundSender>) -> Self {
        WireBackend::new(backend, outbound)
    }
}
