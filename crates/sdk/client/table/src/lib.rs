//! `table` — third-party Table SDK, dual-core same-source.
//!
//! Web emits real HTML `<table>` / `<thead>` / `<tbody>` / `<tr>` /
//! `<th>` / `<td>` so the browser's native table-layout algorithm
//! handles cross-row column alignment for free. Native (iOS / Android /
//! macOS / terminal / gpu / SSR) builds a single **CSS-grid**: every
//! row is flattened to its cells and the cells are parented directly
//! under one grid node whose `N` column tracks span all rows, giving
//! the same cross-row alignment the browser gives a real `<table>`.
//!
//! # One authored surface, two cores
//!
//! The default build is the old-core implementation (`Element::External`
//! on web + the per-backend `register_external` registry), byte-moved
//! into [`mod oldcore`](self). The `new-core` feature swaps in
//! `newcore`, which re-expresses the SAME public names (`Table` /
//! `TableRow` / `TableCell`, `table()` / `table_row()` / `table_cell()`,
//! the `*Props` structs, `register`, `prelude`) over the idea-lite
//! stack: scene-`Registry` payload handlers on web
//! ([`register_handlers`](newcore::register_handlers) — the new core's
//! unified primitive==external contract), and the vocabulary glue's
//! `view` builder for the native grid. Mutually exclusive (same names),
//! mirroring the swap/stack navigator SDKs.
#![deny(missing_docs)]

// One authored surface, two cores (idea-lite migration P6): the default
// build is the old-core implementation, byte-moved into `oldcore`; the
// `new-core` feature swaps in `newcore`, which re-expresses the SAME
// public names over the scene Registry + vocabulary glue.
#[cfg(not(feature = "new-core"))]
mod oldcore;
#[cfg(not(feature = "new-core"))]
pub use oldcore::*;

#[cfg(feature = "new-core")]
mod newcore;
#[cfg(feature = "new-core")]
pub use newcore::*;
