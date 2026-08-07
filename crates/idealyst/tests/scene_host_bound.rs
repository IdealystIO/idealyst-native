//! The boot seam's capability bound must cover what real apps register.
//!
//! `idealyst::entry!` lifts an app's `register_scene_extensions` into a
//! `SceneExtensions` impl whose method is generic over `H: SceneHost`.
//! That bound is therefore the *only* thing an app's registration seam is
//! type-checked against — and a seam may ask for any capability its
//! handlers touch.
//!
//! `SceneHost` was originally `Host + StyleServices + InputOps`, which is
//! narrower than what every non-trivial app needs: the codeblock handler
//! wants `TextOps`, canvas/video want `ExternalOps`, a virtualizer wants
//! `VirtualizerOps`. Under the narrow bound those apps could not be
//! expressed at the seam at all — `idea-ui-docs`, `whiteboard-demo` and
//! the website all failed to build on web with "the trait bound
//! `H: TextOps` is not satisfied" pointing into the macro expansion.
//!
//! These are COMPILE-TIME tests: each fn below is a registration seam
//! shaped like a real app's, and the `SceneExtensions` impl instantiating
//! it is what fails to compile if `SceneHost` narrows again. The bodies
//! never run — building this file is the assertion.

use idealyst::{SceneExtensions, SceneHost};
use runtime_scene::Registry;

// ---------------------------------------------------------------------
// Seams shaped like the real ones in-tree.
// ---------------------------------------------------------------------

/// `websites/idea-ui-docs` — the codeblock handler measures text.
fn text_seam<H>(_registry: &mut Registry<H>)
where
    H: runtime_vocabulary::style_attach::StyleServices
        + runtime_vocabulary::caps::TextOps
        + runtime_vocabulary::caps::InputOps
        + 'static,
{
}

/// `examples/whiteboard-demo` — canvas + video mount External nodes.
fn external_seam<H>(_registry: &mut Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
}

/// A seam asking for a capability neither of the above names, so the
/// test keeps failing if `SceneHost` is trimmed back to "whatever the
/// current examples happen to use".
fn graphics_and_scroll_seam<H>(_registry: &mut Registry<H>)
where
    H: runtime_vocabulary::caps::GraphicsOps
        + runtime_vocabulary::caps::VirtualizerOps
        + runtime_vocabulary::caps::ScrollOps
        + 'static,
{
}

// ---------------------------------------------------------------------
// The seam `entry!` generates. If `SceneHost` doesn't imply the bounds
// above, these impls don't compile.
// ---------------------------------------------------------------------

struct TextExtensions;
impl SceneExtensions for TextExtensions {
    fn register<H>(registry: &mut Registry<H>)
    where
        H: SceneHost,
    {
        text_seam(registry);
    }
}

struct ExternalExtensions;
impl SceneExtensions for ExternalExtensions {
    fn register<H>(registry: &mut Registry<H>)
    where
        H: SceneHost,
    {
        external_seam(registry);
    }
}

struct GraphicsExtensions;
impl SceneExtensions for GraphicsExtensions {
    fn register<H>(registry: &mut Registry<H>)
    where
        H: SceneHost,
    {
        graphics_and_scroll_seam(registry);
    }
}

/// A no-op seam — the scaffold's shape — must still satisfy the trait.
/// Widening `SceneHost` must not have made the trivial case harder.
struct EmptyExtensions;
impl SceneExtensions for EmptyExtensions {
    fn register<H>(_registry: &mut Registry<H>)
    where
        H: SceneHost,
    {
    }
}

/// Nothing to execute — the impls above are the assertion. This exists so
/// `cargo test` reports the file rather than silently only compiling it.
#[test]
fn app_registration_seams_satisfy_the_boot_seams_capability_bound() {
    fn assert_impls<E: SceneExtensions>() {}
    assert_impls::<TextExtensions>();
    assert_impls::<ExternalExtensions>();
    assert_impls::<GraphicsExtensions>();
    assert_impls::<EmptyExtensions>();
}
