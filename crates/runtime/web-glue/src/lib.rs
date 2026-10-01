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
//! | typed DOM handles, casts | [`dom`], [`JsCast`], [`js_class!`]     |
//! | workers on this module   | [`worker`]                             |
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
//! # Constructors (own mode)
//!
//! A wasm32 bin is linked by LLD as a *command* module, which wraps EVERY
//! export — `__glue_invoke`, `__glue_alloc`, … — in a call to
//! `__wasm_call_ctors`, so every JS → Rust call would re-run static
//! constructors (measured: a ctor-bumped counter read 1, 2, 4 across three
//! calls). `idealyst build --web` links the command module and its glue
//! pass then points every export but `main` past its wrapper
//! (`build_web::own_glue::extract_for_build`), so constructors run once,
//! inside `main`. Linking with `-C link-arg=--export=__wasm_call_ctors`
//! (`build_web::own_glue::link_args()`) makes a reactor instead; the
//! generated loader then runs `__wasm_call_ctors()` itself, once, before
//! `main`. Either way, never per call.

#[cfg(feature = "wasm-bindgen-bridge")]
pub mod bridge;
pub mod callback;
pub mod cast;
pub mod dom;
mod dom_api;
pub mod js;
pub mod error;
mod ffi;
mod macros;
#[cfg(not(target_arch = "wasm32"))]
mod mock;
pub mod record;
pub mod string;
pub mod task;
pub mod value;
pub mod worker;

pub use callback::Closure;
pub use cast::JsCast;
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
