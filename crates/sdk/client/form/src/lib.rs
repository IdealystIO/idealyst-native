//! Third-party `Form` SDK for the idealyst framework.
//!
//! Provides a `Form` container: a real `<form>` on web (submit-on-enter,
//! autofill grouping, `preventDefault()`-guarded `on_submit`), a plain
//! passthrough container on native (submission is the author's submit
//! button calling `on_submit`).
//!
//! # One authored surface, two cores
//!
//! The default build is the old-core implementation (`Element::External`
//! payload + per-backend `ExternalRegistry`), byte-moved into
//! [`oldcore`]; the `new-core` feature swaps in [`newcore`], which
//! re-expresses the SAME public names (`FormProps`, `form(props)`, the
//! PascalCase `Form` `ui!` tag, `FormHandle`/`FormBuilder::bind`, a
//! bootstrap `register`) over the scene registry — the new core's
//! unified primitive==external contract. Mutually exclusive (same
//! names), mirroring the svg/codeblock/table SDK precedents. The
//! new-core web leg reproduces the old web handler's `<form>` DOM
//! call-for-call; on non-web new-core hosts the SDK registers the
//! External-placeholder path with the children realized into it (the
//! same passthrough-container behavior the old iOS/Android handlers
//! provided — those native modules stay old-core-only until the
//! new-core native boots grow an external story).
//!
//! # Usage (old core)
//!
//! ```ignore
//! use form::prelude::*;       // brings in `Form`, `form`, `FormProps`
//! use idea_ui::prelude::*;    // Button, TextInput, …
//!
//! // App bootstrap (one line per third-party SDK):
//! let mut backend = WebBackend::new("#app");
//! form::register(&mut backend);
//!
//! // The submit action is a plain closure that reads your field
//! // signals — it is NOT fed by the DOM's FormData. Build it once and
//! // share the `Rc`: hand it to the form (web Enter-to-submit) AND to
//! // your submit button (the universal trigger).
//! let name = signal(String::new());
//! let on_submit: std::rc::Rc<dyn Fn()> = {
//!     let name = name.clone();
//!     std::rc::Rc::new(move || log::info!("submit: {}", name.get()))
//! };
//!
//! ui! {
//!     Form(on_submit = Some(on_submit.clone())) {
//!         TextInput(value = name.clone())
//!         Button(label = "Save", on_click = on_submit.clone())
//!     }
//! }
//! ```
//!
//! On the new core the same author code compiles unchanged; only the
//! bootstrap seam moves — pass [`register`] to the boot entry
//! (`backend_web::newcore::start_in("#app", form::register, app)`).
//!
//! # Why this is an SDK and not a core primitive
//!
//! A form has no convergent cross-platform behavior to put behind the
//! Backend trait: on web `<form>` is a real element (submit-on-enter,
//! autofill grouping, FormData), while iOS/Android have NO form
//! construct — their form affordances (autofill, return-key submit)
//! live per-field on the inputs, not on a container. So `Form` is an
//! opinionated SDK on the external-element contract (with children):
//!   * web    → a real `<form>` wrapping the inputs as DOM descendants,
//!              with the native `submit` event wired to `on_submit`
//!              after `preventDefault()`.
//!   * native → a plain passthrough container; submission is triggered
//!              by the author's submit button calling `on_submit`.
//!
//! # Why `on_submit` translates across platforms
//!
//! It's a triggered *action* (uniform closure), separated from its
//! *trigger* (platform-idiomatic) and its *data* (uniform signals):
//!
//! - **Web** — the handler wires the real `<form>`'s `submit` event,
//!   calls `preventDefault()` (idealyst apps don't POST form-encoded
//!   data — the browser must not navigate/reload), then invokes
//!   `on_submit`. Free Enter-to-submit, and autofill works because the
//!   inputs are real DOM descendants of the `<form>`.
//! - **Native** — there is no form `submit` event, so the handler is a
//!   passthrough container and submission is fired by the author's
//!   submit button calling `on_submit` directly. (Keyboard return /
//!   IME-action submit is a *field-level* affordance and belongs on the
//!   input.)
#![deny(missing_docs)]

// Shared wasm32 helpers (pure DOM ops on the mounted `<form>`, no core
// types) used by BOTH cores' web legs.
#[cfg(target_arch = "wasm32")]
pub(crate) mod web_util;

#[cfg(all(target_arch = "wasm32", not(feature = "new-core")))]
mod web;

#[cfg(all(
    target_os = "android",
    not(target_arch = "wasm32"),
    not(feature = "new-core")
))]
mod android;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32"), not(feature = "new-core")))]
mod ios;

// One authored surface, two cores: the default build is the old-core
// implementation, byte-moved into `oldcore`; the `new-core` feature
// swaps in `newcore`, which re-expresses the SAME public names over the
// scene registry. Mutually exclusive (same names).
#[cfg(not(feature = "new-core"))]
mod oldcore;
#[cfg(not(feature = "new-core"))]
pub use oldcore::*;

#[cfg(feature = "new-core")]
mod newcore;
#[cfg(feature = "new-core")]
pub use newcore::*;
