//! The dev-time overlay's build-time half.
//!
//! Two things, and they are two halves of one loop:
//!
//! - [`archive`] produces a build's descriptor set from SOURCE and
//!   writes it beside the build;
//! - [`decide`] reads it back on the next save and answers the only
//!   question that matters — patch this, or rebuild it?
//!
//! [`workspace`] asks the same question across every crate of the app's
//! cargo workspace: one archive per crate, and the extra rules a library
//! crate's hot patch needs (its dependents re-emitted with it, and no
//! edit to a body a dependent compiles from its metadata).
//!
//! Neither needs a compiler, a watcher or a transport, which is why
//! this is its own crate: both dev shapes need the decision and they
//! have nothing else in common. The web watcher (`dev-reload`) pulls the
//! whole bundler; the runtime-server host (`dev-server`) pulls the wire
//! protocol and a sidecar. Putting the decision in either would have
//! dragged that crate's dependencies into the other's binary.

pub mod archive;
pub mod decide;
pub mod workspace;

pub use archive::{
    crate_overlay_dir, overlay_dir, scan_crate, write_for, write_into, ArchivedSite,
    DescriptorSet, FileDigest, OVERLAY_VERSION,
};
pub use decide::{
    advance_archive, decide, load_archive, load_archive_from, wire_payload, ChangedFile,
    Decision, Reason, SitePatch,
};
pub use workspace::{HotPatchPlan, Route, SavedFile, Workspace, WorkspaceCrate, WorkspaceDecision};
