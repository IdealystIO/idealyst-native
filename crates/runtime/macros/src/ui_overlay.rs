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
//! # Where the node index comes from
//!
//! Not from here. `runtime_macros_parse::number` stamps every node of
//! the parsed tree in a preorder walk, before any emission happens, and
//! the stamp rides through the split pass's clone into the tree the
//! emission walks. [`tag`] is handed that number.
//!
//! The alternative — a counter incremented once per `emit_component` —
//! was what this module did first, and it is subtly wrong: emission
//! order is not source order for every primitive (`anchored_overlay`
//! splits its children, `presence` builds a thunk), so a counter numbers
//! some trees differently from a walk of the same tree. The build-time
//! descriptor producer has only the tree. Taking the number FROM the
//! tree is what makes the two agree by construction rather than by
//! coincidence, and `runtime_template::SPLIT_VERSION` is what lets a
//! differ refuse a binary numbered by a walk it does not know.
//!
//! What is left here is the site KEY, which does depend on the
//! expansion (it is read off the call span), and is therefore the only
//! thing this module keeps in a thread-local.

#[cfg(not(feature = "ui-overlay"))]
use proc_macro2::TokenStream as TokenStream2;

#[cfg(not(feature = "ui-overlay"))]
pub(crate) use inert::*;
#[cfg(feature = "ui-overlay")]
pub(crate) use live::*;

// ===========================================================================
// Feature OFF
// ===========================================================================

#[cfg(not(feature = "ui-overlay"))]
mod inert {
    use super::TokenStream2;

    #[inline(always)]
    pub(crate) fn begin_site() {}

    #[inline(always)]
    pub(crate) fn tag(body: TokenStream2, _node: u32) -> TokenStream2 {
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

    thread_local! {
        /// The site currently being expanded.
        static SITE: Cell<u64> = const { Cell::new(0) };
    }

    /// Start a site: compute its key from the invocation's location.
    pub(crate) fn begin_site() {
        SITE.with(|s| s.set(site_key()));
    }

    pub(crate) fn tag(body: TokenStream2, node: u32) -> TokenStream2 {
        let site = SITE.with(|s| s.get());
        quote! { ::runtime_core::__overlay::tag(#body, #site, #node) }
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
