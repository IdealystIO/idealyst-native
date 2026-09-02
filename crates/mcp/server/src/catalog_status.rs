//! Freshness of the served catalog.
//!
//! The catalog is produced by a subprocess that compiles the project, so
//! it is never instantaneous and can fail. Two facts follow, and both
//! used to be invisible to a client:
//!
//! 1. **A rebuild takes real time.** The first extraction on a cold cache
//!    is minutes; a warm incremental one is tens of seconds. During that
//!    window the previous catalog is still served (deliberately — see
//!    [`crate::watch`]), so answers are *old*, not wrong-shaped.
//! 2. **A rebuild can fail.** A project that doesn't compile produces no
//!    catalog, and the previous one keeps being served. That is the right
//!    behaviour and the dangerous one: without a signal, a client reports
//!    props for a component the user just changed, with full confidence.
//!
//! The failure this exists to prevent is *absence without cause*. An
//! empty `list_components` is indistinguishable between "this project has
//! no components", "the first build is still running", and "the project
//! root was misconfigured so nothing was ever built" — three very
//! different situations that used to look identical.
//!
//! Status is therefore reported two ways. [`CatalogStatus::marker`]
//! rides on the catalog tools' own responses, so a caller learns the data
//! is stale without having to think to ask; the `catalog_status` tool
//! answers deliberately, and can wait for an in-flight rebuild to land.
//! Notifications are NOT the mechanism: whether a client surfaces
//! `notifications/message` into a model's context is client-dependent,
//! so they can only ever be a nudge to re-query.

use std::time::{Duration, SystemTime};

/// What the catalog currently being served *is*.
///
/// Orthogonal to whether a rebuild is running: a rebuild can be in flight
/// over a perfectly good catalog, which is the common case on a save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogState {
    /// Built successfully and nothing has invalidated it.
    Current,
    /// No catalog has ever been built and the first extraction is
    /// running. Distinct from [`Self::Current`] with a rebuild in
    /// flight: there is nothing to serve yet, so an empty result means
    /// "not built", not "nothing found".
    ///
    /// The server binds stdio BEFORE this finishes — a first extraction
    /// compiles the project and can take minutes, far longer than an MCP
    /// client's connect timeout, so blocking on it made the server
    /// unreachable rather than merely empty.
    Initializing,
    /// A rebuild was attempted and failed. The catalog below is the last
    /// one that built — usable, but it predates whatever the user just
    /// changed.
    Stale,
    /// No catalog was ever produced, and the reason is known (a
    /// misconfigured project root, an extractor that never compiled).
    /// Distinct from an empty catalog that built fine.
    Unavailable,
}

impl CatalogState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Initializing => "initializing",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Freshness of the served catalog, plus what it cost to build.
#[derive(Debug, Clone)]
pub struct CatalogStatus {
    pub state: CatalogState,
    /// When the catalog being served finished building.
    pub built_at: Option<SystemTime>,
    /// How long that build took — the estimate a caller needs to decide
    /// whether waiting for a rebuild is worth it.
    pub build_duration: Option<Duration>,
    /// When a rebuild started, while one is running. `None` means idle.
    pub rebuild_started_at: Option<SystemTime>,
    /// When the last attempt (successful or not) was made.
    pub last_attempt_at: Option<SystemTime>,
    /// Why the last attempt failed, when it did. Extractor stderr,
    /// trimmed — usually a compiler error.
    pub last_error: Option<String>,
    /// Why no catalog exists at all, for [`CatalogState::Unavailable`].
    pub reason: Option<String>,
    /// Increments on every successful swap. A caller that remembers a
    /// generation can tell "rebuilt since I last looked" from "still the
    /// same data".
    pub generation: u64,
}

impl Default for CatalogStatus {
    fn default() -> Self {
        Self {
            // Nothing has been attempted yet. A server with no project
            // context legitimately sits here forever, serving whatever
            // the live-app bridge provides.
            state: CatalogState::Current,
            built_at: None,
            build_duration: None,
            rebuild_started_at: None,
            last_attempt_at: None,
            last_error: None,
            reason: None,
            generation: 0,
        }
    }
}

/// Seconds since `t`, saturating — clocks can step backwards.
fn secs_since(t: SystemTime) -> u64 {
    SystemTime::now()
        .duration_since(t)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

impl CatalogStatus {
    /// Is the served catalog both current and settled?
    pub fn is_settled_current(&self) -> bool {
        self.state == CatalogState::Current && self.rebuild_started_at.is_none()
    }

    /// A one-paragraph warning to prepend to catalog tool output, or
    /// `None` when the catalog is current and no rebuild is running.
    ///
    /// Returning `None` in the happy path is deliberate: a marker on
    /// every response would be noise, and noise gets skimmed past exactly
    /// when it finally matters.
    pub fn marker(&self) -> Option<String> {
        let cost = self
            .build_duration
            .map(|d| format!(" Last successful build took {}s.", d.as_secs().max(1)))
            .unwrap_or_default();

        match self.state {
            CatalogState::Unavailable => Some(format!(
                "[catalog: UNAVAILABLE — no catalog could be built{}. \
                 Results below are empty for that reason, NOT because the \
                 project has no components.]",
                self.reason
                    .as_deref()
                    .map(|r| format!(": {r}"))
                    .unwrap_or_default()
            )),
            CatalogState::Stale => {
                let ago = self
                    .last_attempt_at
                    .map(|t| format!(" {}s ago", secs_since(t)))
                    .unwrap_or_default();
                let built = self
                    .built_at
                    .map(|t| format!(" from the build {}s ago", secs_since(t)))
                    .unwrap_or_default();
                Some(format!(
                    "[catalog: STALE — the last rebuild failed{ago}, so results \
                     below are{built} and predate any change that broke it.{}{}]",
                    self.last_error
                        .as_deref()
                        .map(|e| format!(" Extractor said: {e}"))
                        .unwrap_or_default(),
                    if self.rebuild_started_at.is_some() {
                        " A rebuild is running now."
                    } else {
                        ""
                    },
                ))
            }
            CatalogState::Initializing => Some(format!(
                "[catalog: INITIALIZING — the first extraction is still running{}. \
                 It compiles the project, so this takes tens of seconds warm and \
                 minutes cold. The catalog is EMPTY until it lands: an empty result \
                 below means \"not built yet\", NOT \"no components\". Call \
                 catalog_status with wait_for_current to wait for it.{cost}]",
                self.rebuild_started_at
                    .map(|t| format!(" ({}s so far)", secs_since(t)))
                    .unwrap_or_default(),
            )),
            CatalogState::Current => {
                let started = self.rebuild_started_at?;
                Some(format!(
                    "[catalog: REBUILDING — started {}s ago. Results below are the \
                     previous build and do not include changes since.{cost}]",
                    secs_since(started)
                ))
            }
        }
    }

    /// Machine-readable form for the `catalog_status` tool.
    pub fn to_json(&self) -> serde_json::Value {
        let ago = |t: Option<SystemTime>| t.map(secs_since);
        serde_json::json!({
            "state": self.state.as_str(),
            "rebuild_in_flight": self.rebuild_started_at.is_some(),
            "rebuild_running_for_seconds": ago(self.rebuild_started_at),
            "built_seconds_ago": ago(self.built_at),
            "last_build_duration_seconds": self.build_duration.map(|d| d.as_secs()),
            "last_attempt_seconds_ago": ago(self.last_attempt_at),
            "last_error": self.last_error,
            "reason": self.reason,
            "generation": self.generation,
            "summary": self.marker().unwrap_or_else(||
                "[catalog: current — no rebuild running.]".to_string()),
        })
    }
}
