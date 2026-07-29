//! `NetworkState<T, E>` — the four-state enum projection shared by
//! [`Resource`](crate::Resource) and [`Mutation`](crate::Mutation).
//!
//! The backing state structs (`ResourceState` / `MutationState`) are
//! deliberately rich — they can simultaneously hold prior data, a new
//! error, and a `loading: true` flag, which is useful for refetch-
//! while-stale and optimistic-UI patterns. But the common UI case is
//! "show one of these four things":
//!
//! ```text
//! Idle      — never triggered (mutations only; resources always start Loading).
//! Loading   — a fetch is in flight.
//! Success(T) — the most recent fetch succeeded.
//! Error(E)  — the most recent fetch failed.
//! ```
//!
//! This module owns the enum + the projection rules.

use crate::mutation::MutationState;
use crate::resource::ResourceState;

/// Collapsed view of an async operation's state, suitable for direct
/// `match` against in UI code.
///
/// Constructed via [`Resource::network_state`](crate::Resource::network_state)
/// or [`Mutation::network_state`](crate::Mutation::network_state); also
/// `From<&ResourceState<T, E>>` / `From<&MutationState<T, E>>` for ad-hoc
/// conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkState<T, E> {
    /// No fetch has been triggered yet. Only reachable for
    /// `Mutation`; a `Resource` runs its fetcher eagerly so its
    /// initial state is `Loading`, never `Idle`.
    Idle,
    /// A fetch is currently in flight.
    Loading,
    /// The most recent fetch resolved with this payload.
    Success(T),
    /// The most recent fetch failed with this error.
    Error(E),
}

impl<T, E> NetworkState<T, E> {
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success(_))
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    pub fn data(&self) -> Option<&T> {
        match self {
            Self::Success(d) => Some(d),
            _ => None,
        }
    }
    pub fn error(&self) -> Option<&E> {
        match self {
            Self::Error(e) => Some(e),
            _ => None,
        }
    }
}

// =============================================================================
// Projection rules
//
// Precedence when collapsing a rich state struct to the enum:
//   Loading > Error > Success > Idle
//
// `Loading` wins because it represents the *current* operation, which
// is the most actionable for UI (show a spinner). After a completed
// op, Error wins over Success because a fresh failure shouldn't be
// hidden behind stale data — apps that want "show stale data on
// error" can read the underlying ResourceState/MutationState directly
// rather than going through this projection.
// =============================================================================

impl<T: Clone, E: Clone> From<&ResourceState<T, E>> for NetworkState<T, E> {
    fn from(s: &ResourceState<T, E>) -> Self {
        if s.loading {
            return Self::Loading;
        }
        if let Some(e) = &s.error {
            return Self::Error(e.clone());
        }
        if let Some(d) = &s.data {
            return Self::Success(d.clone());
        }
        // ResourceState is constructed with loading: true on creation
        // and only flips to loading: false once a fetch settles, so
        // the all-None/!loading branch should be unreachable in
        // practice — but `From` must be total. Treat it as Loading
        // (the state was never observed to settle).
        Self::Loading
    }
}

impl<T: Clone, E: Clone> From<&MutationState<T, E>> for NetworkState<T, E> {
    fn from(s: &MutationState<T, E>) -> Self {
        if s.loading {
            return Self::Loading;
        }
        if let Some(e) = &s.error {
            return Self::Error(e.clone());
        }
        if let Some(d) = &s.data {
            return Self::Success(d.clone());
        }
        Self::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_state_loading_projects_to_loading() {
        let s = ResourceState::<i32, &'static str> {
            data: None,
            error: None,
            loading: true,
        };
        assert_eq!(NetworkState::from(&s), NetworkState::Loading);
    }

    #[test]
    fn resource_state_settled_with_data_projects_to_success() {
        let s = ResourceState::<i32, &'static str> {
            data: Some(7),
            error: None,
            loading: false,
        };
        assert_eq!(NetworkState::from(&s), NetworkState::Success(7));
    }

    #[test]
    fn resource_state_settled_with_error_projects_to_error() {
        let s = ResourceState::<i32, &'static str> {
            data: None,
            error: Some("boom"),
            loading: false,
        };
        assert_eq!(NetworkState::from(&s), NetworkState::Error("boom"));
    }

    #[test]
    fn resource_state_refetching_projects_to_loading_even_with_prior_data() {
        // Refetch-while-stale: data present, error absent, loading true.
        // The enum collapses to Loading because the in-flight op is the
        // most actionable state for a UI.
        let s = ResourceState {
            data: Some(7),
            error: None,
            loading: true,
        };
        assert_eq!(NetworkState::<i32, &'static str>::from(&s), NetworkState::Loading);
    }

    #[test]
    fn resource_state_error_beats_stale_success() {
        // Both present, not loading → Error wins (precedence rule).
        let s = ResourceState {
            data: Some(7),
            error: Some("boom"),
            loading: false,
        };
        assert_eq!(NetworkState::from(&s), NetworkState::Error("boom"));
    }

    #[test]
    fn mutation_state_default_projects_to_idle() {
        let s = MutationState::<i32, &'static str>::default();
        assert_eq!(NetworkState::from(&s), NetworkState::Idle);
    }

    #[test]
    fn mutation_state_loading_projects_to_loading() {
        let s = MutationState {
            data: None,
            error: None,
            loading: true,
        };
        assert_eq!(NetworkState::<i32, &'static str>::from(&s), NetworkState::Loading);
    }

    #[test]
    fn mutation_state_settled_success_projects_to_success() {
        let s = MutationState::<i32, &'static str> {
            data: Some(42),
            error: None,
            loading: false,
        };
        assert_eq!(NetworkState::from(&s), NetworkState::Success(42));
    }
}
