//! Lightweight, Rust-native i18n — runtime half.
//!
//! Author translations inline with the [`i18n!`] macro:
//!
//! ```ignore
//! mod t {
//!     i18n::i18n! {
//!         locales: { En = "en" (default), Fr = "fr", Ja = "ja" (lazy) }
//!         greeting(name) { En: "Hello, {name}", Fr: "Bonjour, {name}" }
//!         items(count)   { En: "{count} items", Fr: "{count} articles" }
//!     }
//! }
//! ```
//!
//! The macro emits a strongly-typed `Locale` enum and one function per
//! message returning [`Reactive<String>`](runtime_core::Reactive), so the
//! result is *live*: calling [`set_locale_code`] (or the generated typed
//! `set_locale`) re-renders affected `text()` in place. A bundled locale
//! that's missing a message — or a `{placeholder}` that doesn't match a
//! declared argument — is a **compile error**.
//!
//! ## Bundled vs. opt-in locales
//!
//! - **Bundled** locales (the default, and any unmarked locale) have their
//!   strings compiled into the binary as `match` arms — zero network, work
//!   offline on every backend.
//! - **Opt-in** (`lazy`) locales carry *no bytes in the binary*. Their
//!   strings live in a JSON pack (`{ "greeting": "こんにちは、{name}", … }`)
//!   fetched on demand (or inlined by SSR/SSG) via [`install_pack`] /
//!   [`set_pack_loader`]. Until a pack arrives, messages fall back to the
//!   default locale's text; when it arrives the same reactive text upgrades
//!   in place.
//!
//! ## Formatting
//!
//! Built-in formatting is interpolation only — `{name}` substitution with
//! `{{`/`}}` escapes. For plurals / gender / ICU, install a custom
//! formatter via [`install_formatter`]; it receives the same
//! `(template, args)` and can do anything.

mod format;
mod locale;
mod packs;

pub use format::{clear_formatter, format, install_formatter};
pub use locale::{current_locale_code, set_locale_code};
pub use packs::{
    ensure_pack_loaded, has_pack, install_pack, install_pack_json, opt_in_template, set_pack_loader,
};

#[cfg(feature = "lazy-fetch")]
pub use packs::net_pack_loader;

/// Re-exported so generated message functions (and call sites) need only
/// the `i18n` crate in scope, not `runtime-core` directly.
pub use runtime_core::Reactive;

/// The `i18n! { … }` translation macro. See the crate-level docs.
pub use i18n_macros::i18n;
