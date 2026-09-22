//! The `ui!` front end: parse, split, number, describe.
//!
//! Everything here is a pure function from tokens to data. No proc-macro
//! context is required, and none is available — `proc_macro2` is used
//! throughout precisely because it works outside one.
//!
//! ```text
//!   ui! { … } tokens
//!        │
//!        ├─ ast      parse   → Ui / UiNode / Prop
//!        ├─ split    classify→ Scope { slots, rewritten nodes }
//!        ├─ number   walk    → one u32 per element node
//!        └─ describe fold    → runtime_template::Descriptor
//! ```
//!
//! # Why this is a library and not part of the proc macro
//!
//! Two callers need the same answer:
//!
//! - `runtime-macros`, expanding `ui!` inside rustc, tags each node it
//!   builds with the node's number; and
//! - the CLI, reading a crate's sources at build time, produces the
//!   descriptor a dev server diffs — whose node indices must mean
//!   exactly what the tags mean.
//!
//! A proc-macro crate cannot be used as a library, so while the parser
//! lived inside one the second caller had to reimplement it. A
//! reimplementation that disagreed by a single node would mis-address
//! every patch after that node, silently. One parser, one split, one
//! numbering, two callers is the only arrangement where that class of
//! bug does not exist.
//!
//! # The numbering is positional, not emission-ordered
//!
//! [`number::number_elements`] assigns each element node its index from
//! a PREORDER walk of the parsed tree. Both callers use it: the macro
//! looks a node up as it emits, rather than counting emissions.
//!
//! That is deliberate and it matters. Some primitives emit their
//! children in an order that is not source order — `anchored_overlay`
//! splits its children into anchor and overlay, `presence` builds a
//! thunk — so a counter incremented during emission would number those
//! trees differently from a walk of the same tree. Taking the number
//! from the tree POSITION removes the question: emission order is then
//! free to be whatever each primitive needs.

#![forbid(unsafe_code)]

pub mod ast;
pub mod describe;
pub mod number;
pub mod primitives;
pub mod reactive_shape;
pub mod recovery;
pub mod scan;
pub mod split;

pub use ast::{is_a11y_attr, MatchArm, Prop, Ui, UiNode};
pub use describe::describe;
pub use number::{number_elements, NodeNumbering, StampMismatch};
pub use reactive_shape::is_reactive_call_shape;
pub use scan::{file_skeleton, sites_in_file, skeleton_of, Site};
