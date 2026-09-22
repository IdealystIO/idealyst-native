//! `ui-overlay` emission: the origin tag each node a `ui!` site builds
//! carries.
//!
//! Entirely behind the `ui-overlay` cargo feature. With the feature OFF
//! this module is a handful of `#[inline(always)]` no-ops and the
//! emission is byte-identical to a build without it —
//! `crates/dev/ui-lowering-parity` runs its whole corpus in BOTH feature
//! states against the SAME frozen goldens, which is the strongest form
//! of "identical" available: same scene, same op sequence, same driven
//! deltas.
//!
//! # What ON adds — and only this
//!
//! Per NODE, wrapping that node's finished expression:
//!
//! ```ignore
//! runtime_core::__overlay::tag(<expr>, 0x1234_5678_9abc_def0u64, 7u32)
//! ```
//!
//! Two integer literals and a call. No `static`, no descriptor, no
//! prelude, nothing per SITE at all.
//!
//! # Why the descriptor is not here any more
//!
//! It used to be: this module emitted a `static Descriptor` per site
//! describing every node's props and literals, and registered it at
//! build time. Measured on CrewForge (~256 CGUs), that cost **+1.4 s on
//! every one-edit rebuild** — 5.0 s to 6.5 s, a 27% tax on exactly the
//! loop the feature exists to shorten — with the time in
//! `macro_expand_crate` (1.65 s to 2.52 s) and `serialize_dep_graph`,
//! and none of it in codegen. Compiling the descriptor out and keeping
//! only the tags measured within noise of the feature being off:
//!
//! | one-edit rebuild | off | tags only | tags + descriptor |
//! |---|---|---|---|
//! | literal in a screen | 5.04–5.30 s | 5.63–5.64 s | 6.83–6.97 s |
//! | trailing comment | 4.96–5.02 s | 4.85–5.02 s | 6.47–6.64 s |
//!
//! So the descriptor is produced from SOURCE at build time instead, by
//! the same split pass, and written beside the build. The numbers a
//! patch needs — which site, which node — are the only things that
//! cannot be recovered from source, and they are what stays.
//!
//! # Why the index comes from the emission walk
//!
//! A tag's only job is to map a descriptor node back to the `Element`
//! that node produced. That mapping is trustworthy exactly when both
//! come from the SAME walk. So the counter is threaded through the
//! emission itself: `emit_component` calls [`open_node`] as it emits and
//! tags with the index it gets back, and the build-time producer runs
//! the same split pass to get the same numbering. A test over the parity
//! corpus asserts the two agree; `runtime_template::SPLIT_VERSION`
//! exists so a differ can refuse a binary numbered by a walk it does not
//! know.
//!
//! One counter per expansion, in a thread-local — a proc-macro expansion
//! is single-threaded and never interleaved, the same reason
//! `ui_split`'s slot counter is one. Note that nothing held across the
//! expansion boundary is a `TokenStream` any more, which also retires a
//! sharp edge: a `proc_macro2` token wraps a bridge handle valid only
//! inside its own expansion, and the descriptor accumulator used to hold
//! them.

#[cfg(not(feature = "ui-overlay"))]
use proc_macro2::TokenStream as TokenStream2;

#[cfg(not(feature = "ui-overlay"))]
pub(crate) use inert::*;
#[cfg(feature = "ui-overlay")]
pub(crate) use live::*;

/// A node's identity within its site, handed back by [`open_node`] and
/// spliced into its tag.
///
/// `Option`-shaped so the feature-off path has a zero-sized "no index"
/// to return without every caller branching on a `cfg`.
pub(crate) type NodeIndex = Option<u32>;

// ===========================================================================
// Feature OFF
// ===========================================================================

#[cfg(not(feature = "ui-overlay"))]
mod inert {
    use super::{NodeIndex, TokenStream2};

    #[inline(always)]
    pub(crate) fn begin_site() {}

    #[inline(always)]
    pub(crate) fn open_node() -> NodeIndex {
        None
    }

    #[inline(always)]
    pub(crate) fn tag(body: TokenStream2, _index: NodeIndex) -> TokenStream2 {
        body
    }
}

// ===========================================================================
// Feature ON
// ===========================================================================

#[cfg(feature = "ui-overlay")]
mod live {
    use std::cell::Cell;

    use proc_macro2::TokenStream as TokenStream2;
    use quote::quote;

    use super::NodeIndex;

    thread_local! {
        /// The current site's key, and how many nodes it has opened.
        static SITE: Cell<(u64, u32)> = const { Cell::new((0, 0)) };
    }

    /// Start a site: compute its key from the invocation's location and
    /// restart node numbering at 0.
    pub(crate) fn begin_site() {
        SITE.with(|s| s.set((site_key(), 0)));
    }

    /// Take this node's index. Reserved BEFORE its children are emitted,
    /// so a parent always precedes its children — the same order the
    /// split pass numbers them in.
    pub(crate) fn open_node() -> NodeIndex {
        SITE.with(|s| {
            let (key, next) = s.get();
            s.set((key, next + 1));
            Some(next)
        })
    }

    pub(crate) fn tag(body: TokenStream2, index: NodeIndex) -> TokenStream2 {
        let Some(index) = index else { return body };
        let site = SITE.with(|s| s.get().0);
        quote! { ::runtime_core::__overlay::tag(#body, #site, #index) }
    }

    /// The key for the `ui!` invocation being expanded.
    ///
    /// Reads the call span's file and position and the compiling crate's
    /// own `CARGO_*` environment, then folds them with
    /// `runtime_template::site_key` — the SAME function the build-time
    /// producer calls, so a descriptor addresses the binary by
    /// construction rather than by convention.
    ///
    /// Outside a real proc-macro context (this crate's own unit tests,
    /// an IDE proc-macro server with degenerate spans) there is no
    /// location to read; the key is then the fold of empty parts. That
    /// makes tags in a unit test deterministic and useless for
    /// addressing, which is correct — a test expansion is not a build.
    fn site_key() -> u64 {
        let Some((file, line, col)) = call_location() else {
            return runtime_template::site_key("", "", 0, 0);
        };
        let package = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
        runtime_template::site_key(&package, &file, line, col)
    }

    /// `(file relative to the package root, line, column)` of the
    /// invocation, or `None` when not expanding inside rustc.
    fn call_location() -> Option<(String, u32, u32)> {
        if !proc_macro::is_available() {
            return None;
        }
        let span = proc_macro2::Span::call_site().unwrap();
        Some((relative_to_manifest(&span.file()), span.line() as u32, span.column() as u32))
    }

    /// Strip the compiling package's root from a source path and
    /// normalize separators.
    ///
    /// rustc reports whatever path it was invoked with: cargo passes a
    /// package-relative one for a workspace member (`src/app.rs`) and an
    /// absolute one for a registry dependency. Stripping
    /// `CARGO_MANIFEST_DIR` makes both `src/app.rs`, so the key does not
    /// depend on where the crate happened to be checked out — which is
    /// what lets a descriptor produced on one machine address a binary
    /// built on another.
    fn relative_to_manifest(file: &str) -> String {
        let file = file.replace('\\', "/");
        let Ok(root) = std::env::var("CARGO_MANIFEST_DIR") else {
            return file;
        };
        let root = root.replace('\\', "/");
        match file.strip_prefix(&root).and_then(|rest| rest.strip_prefix('/')) {
            Some(rest) => rest.to_string(),
            None => file,
        }
    }
}
