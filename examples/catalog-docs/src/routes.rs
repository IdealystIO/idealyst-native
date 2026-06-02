//! Routing for the catalog docs.
//!
//! Unlike idea-ui-docs (one hand-written `Route` constant per page),
//! this app has TWO routes:
//!
//! - [`OVERVIEW_ROUTE`] — the catalog landing page (counts per kind).
//! - [`ENTRY_ROUTE`] — a single *parameterized* route whose
//!   [`EntryParams`] carry `kind` + `slug`. Every catalog entry (of
//!   which there are hundreds, and the set changes as the framework
//!   grows) is reached through this one route, so we never enumerate
//!   entries in the navigator wiring.
//!
//! The URL pattern is `/entry/:kind/:slug`; on web the
//! [`RouteParams`] impl round-trips params ⇄ URL so deep links work and
//! the active-path signal distinguishes entries (`route.name()` alone
//! can't — every entry shares the `"entry"` name).

use std::collections::HashMap;

use runtime_core::primitives::navigator::RouteParams;
use runtime_core::Route;

use crate::catalog::Kind;

pub const OVERVIEW_ROUTE: Route<()> = Route::<()>::new("catalog", "/");
pub const ENTRY_ROUTE: Route<EntryParams> = Route::<EntryParams>::new("entry", "/entry/:kind/:slug");

/// Typed params for [`ENTRY_ROUTE`] — the kind's path segment plus the
/// entry slug.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryParams {
    pub kind: Kind,
    pub slug: String,
}

impl EntryParams {
    pub fn new(kind: Kind, slug: impl Into<String>) -> Self {
        Self { kind, slug: slug.into() }
    }

    /// The concrete URL path for this entry — used for active-state
    /// matching against the slot's `active_path` signal.
    pub fn url(&self) -> String {
        format!("/entry/{}/{}", self.kind.path_segment(), self.slug)
    }
}

fn kind_from_segment(seg: &str) -> Option<Kind> {
    match seg {
        "components" => Some(Kind::Component),
        "primitives" => Some(Kind::Primitive),
        "utilities" => Some(Kind::Utility),
        "types" => Some(Kind::Type),
        "guides" => Some(Kind::Guide),
        _ => None,
    }
}

impl RouteParams for EntryParams {
    fn to_path(&self, _pattern: &str) -> String {
        // Fill `:kind` / `:slug` ourselves — the default impl panics on
        // placeholder patterns.
        self.url()
    }

    fn from_segments(segments: &HashMap<String, String>) -> Option<Self> {
        let kind = kind_from_segment(segments.get("kind")?.as_str())?;
        let slug = segments.get("slug")?.clone();
        Some(EntryParams { kind, slug })
    }
}

/// Convenience used by the screen render closure: pull `(kind, slug)`
/// out of the typed params. (Kept as a free fn so `lib.rs` doesn't have
/// to reach into the struct fields directly.)
pub fn decode_entry_route(params: &EntryParams) -> (Kind, String) {
    (params.kind, params.slug.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_params_url_round_trips_through_segments() {
        let p = EntryParams::new(Kind::Component, "button");
        let url = p.to_path(ENTRY_ROUTE.path());
        assert_eq!(url, "/entry/components/button");

        let mut segs = HashMap::new();
        segs.insert("kind".to_string(), "components".to_string());
        segs.insert("slug".to_string(), "button".to_string());
        let decoded = EntryParams::from_segments(&segs).expect("decodes");
        assert_eq!(decoded, p);
    }

    #[test]
    fn unknown_kind_segment_does_not_decode() {
        let mut segs = HashMap::new();
        segs.insert("kind".to_string(), "nonsense".to_string());
        segs.insert("slug".to_string(), "x".to_string());
        assert!(EntryParams::from_segments(&segs).is_none());
    }
}
