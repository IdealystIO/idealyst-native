//! The framework-owned wasm ⇄ JS boundary.
//!
//! Phase 1 (proof of concept) of `docs/proposals/own-web-bindings.md`:
//! everything the framework takes from wasm-bindgen / web-sys / js-sys,
//! without them.
//!
//! | need                     | here                                   |
//! |--------------------------|----------------------------------------|
//! | JS object handles        | [`JsValue`] (RAII slab index)          |
//! | declaring JS functions   | [`import!`] (JS inline, per function)  |
//! | shipping a JS module     | [`js_module!`]                         |
//! | strings                  | [`string`] (borrow in, Rust buffer out)|
//! | closures handed to JS    | [`Closure`]                            |
//! | exceptions               | `#[catch]` imports → [`JsError`]       |
//! | promises / executor      | [`JsFuture`], [`spawn_local`]          |
//! | microtasks               | [`queue_microtask`]                    |
//!
//! # How the JS gets to the page
//!
//! There is no build-time code generator that knows about these types.
//! Every snippet rides in its wasm import's NAME, and the JS runtime plus
//! any crate's modules ride in the `__idealyst_glue` custom section (see
//! [`record`]). The build pass `build_web::own_glue` streams the linked
//! module once (import section + custom sections; function bodies are
//! copied as bytes, never parsed), strips both, and writes `pkg/<lib>.js`
//! with the same entry contract wasm-bindgen's `--target web` output has.
//!
//! # Link requirement (own mode)
//!
//! Link with `-C link-arg=--export=__wasm_call_ctors`. Without it, LLD
//! builds a *command* module and wraps EVERY export — `__glue_invoke`,
//! `__glue_alloc`, … — in a call to `__wasm_call_ctors`, so every JS → Rust
//! call re-runs static constructors (measured: a ctor-bumped counter read
//! 1, 2, 4 across three calls). Exporting it makes the module a reactor;
//! the generated loader then runs constructors exactly once, before `main`.
//! `build_web::own_glue::link_args()` supplies the flag.

pub mod callback;
pub mod error;
mod ffi;
mod macros;
#[cfg(not(target_arch = "wasm32"))]
mod mock;
pub mod record;
pub mod string;
pub mod task;
pub mod value;

pub use callback::Closure;
pub use error::JsError;
pub use task::{queue_microtask, spawn_local, JsFuture};
pub use value::{JsType, JsValue};

/// The wasm import module every glue import is declared under. Relative,
/// so that under wasm-bindgen (hybrid mode) its generated
/// `import * as … from "./__idealyst_glue.js"` resolves to the file the
/// build pass writes next to it.
pub const IMPORT_MODULE: &str = "./__idealyst_glue.js";

/// The custom section carrying [`record`]s.
pub const SECTION: &str = "__idealyst_glue";

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
