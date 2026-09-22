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
//! and, per `#[component]` call site, around the props struct literal:
//!
//! ```ignore
//! {
//!     let mut __p = Badge { .. };
//!     for (n, v) in runtime_core::__overlay::staged_props(SITE, NODE) {
//!         __p.__apply_literal(&n, &v);
//!     }
//!     runtime_core::__overlay::register_ctor("Badge", |props, children| { .. });
//!     __p
//! }
//! ```
//!
//! Two integer literals and a call per node; a lookup and a registration
//! per component. No `static`, no descriptor, no prelude, nothing per
//! SITE at all.
//!
//! # Why a component is patched at its PROPS
//!
//! Everything else is patched on the built `Element`, inside
//! `__overlay::tag`. A component cannot be: by the time its `Element`
//! exists its props have been consumed and its body has run. The props
//! struct literal is the last moment they are reachable.
//!
//! And the loop has to be EMITTED, not called: `__apply_literal` is an
//! inherent method on the generated props type with a blanket-trait
//! fallback, and inherent-before-trait resolution only happens where the
//! concrete type is known. A generic helper in the vocabulary would bind
//! the fallback and silently apply nothing — the worst possible failure
//! here, since it looks exactly like "no patch staged".
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

    #[inline(always)]
    pub(crate) fn component_props(
        body: TokenStream2,
        _name: &proc_macro2::Ident,
        _node: u32,
    ) -> TokenStream2 {
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

    /// Wrap a `#[component]`'s props struct literal so a staged patch
    /// reaches it, and teach the overlay how to build this component.
    ///
    /// The constructor is a non-capturing closure, so it coerces to a
    /// plain `fn` pointer; it exists here rather than in a link-time
    /// registry because only this call site knows the props TYPE, which
    /// is what makes `__apply_literal` / `__apply_children` resolve
    /// inherently.
    pub(crate) fn component_props(
        body: TokenStream2,
        name: &proc_macro2::Ident,
        node: u32,
    ) -> TokenStream2 {
        let site = SITE.with(|s| s.get());
        let name_str = name.to_string();
        quote! {
            {
                #[allow(unused_imports)]
                use ::runtime_core::__template::ApplyLiteralFallback as _;
                ::runtime_core::__overlay::register_ctor(
                    #name_str,
                    |__props, __children| {
                        #[allow(unused_imports)]
                        use ::runtime_core::__template::ApplyLiteralFallback as _;
                        let mut __p = <#name as ::runtime_core::BuildElement>::defaults();
                        for (__n, __v) in __props {
                            __p.__apply_literal(__n, __v);
                        }
                        if !__children.is_empty() && !__p.__apply_children(__children) {
                            return ::core::option::Option::None;
                        }
                        ::core::option::Option::Some(
                            ::runtime_core::BuildElement::build(__p),
                        )
                    },
                );
                let mut __p = #body;
                for (__n, __v) in ::runtime_core::__overlay::staged_props(#site, #node) {
                    __p.__apply_literal(&__n, &__v);
                }
                __p
            }
        }
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

    /// Make a source path package-relative and `/`-separated.
    ///
    /// rustc reports whatever path it was invoked with, and that varies:
    /// cargo passes a WORKSPACE-ROOT-relative path for a workspace
    /// member (`crates/app/src/main.rs`) and an absolute one for a
    /// registry dependency. Neither is what a descriptor should be keyed
    /// by — one depends on where in the workspace the crate sits, the
    /// other on where the machine keeps its cargo registry.
    ///
    /// So: absolutize against rustc's working directory (which is the
    /// workspace root, and is what the relative form is relative to),
    /// then strip `CARGO_MANIFEST_DIR`. Both spellings become
    /// `src/main.rs`, and a descriptor produced on one machine addresses
    /// a binary built on another.
    ///
    /// A path that is not under the manifest dir — an `include!` of a
    /// generated file under `target/`, say — keeps its absolute form.
    /// It is still stable within a machine, which is the most that can
    /// be said for a file the package does not own.
    fn relative_to_manifest(file: &str) -> String {
        let path = std::path::Path::new(file);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            match std::env::current_dir() {
                Ok(cwd) => cwd.join(path),
                Err(_) => path.to_path_buf(),
            }
        };
        let Ok(root) = std::env::var("CARGO_MANIFEST_DIR") else {
            return absolute.to_string_lossy().replace('\\', "/");
        };
        match absolute.strip_prefix(&root) {
            Ok(rest) => rest.to_string_lossy().replace('\\', "/"),
            Err(_) => absolute.to_string_lossy().replace('\\', "/"),
        }
    }
}
