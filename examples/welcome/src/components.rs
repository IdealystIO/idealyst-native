//! Visual elements that make up the welcome scene. Each module
//! exports a `#[component]`-annotated function the `ui!` tree in
//! [`crate::app`] invokes by its PascalCase tag. `#[macro_use]`
//! lifts each component's generated invocation macro into the
//! parent scope so the chain reaches [`crate::app`].
//!
//! Declaration order matters: `#[macro_use]` only brings macros
//! into scope *after* the declaration, so leaves with no internal
//! component references can sit anywhere, but `content_layer`
//! (which embeds `WelcomePhrase` + `Subtitle`) must come after
//! those declarations.

pub mod page;
#[macro_use]
pub mod planet;
#[macro_use]
pub mod subtitle;
#[macro_use]
pub mod sun_glare;
#[macro_use]
pub mod vignette;
#[macro_use]
pub mod welcome_phrase;
#[macro_use]
pub mod content_layer;
