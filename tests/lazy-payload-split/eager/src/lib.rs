//! EAGER variant of the handler-registration split measurement. Its
//! `register_scene_extensions` installs the heavy SDK's mount handler at
//! the boot seam, so `main.wasm` statically reaches `mount_heavy` and
//! therefore carries its 512 KiB `HEAVY` payload.
//!
//! # What this pair measures
//!
//! **Where a third-party payload's HANDLER is registered.** This file and
//! the `lazy/` sibling are identical — same `app()`, same rendered tree,
//! same `#[component(lazy)]` chunk body — except for one line:
//!
//! - `eager/` (here): `heavy::register(registry)` at boot.
//! - `lazy/`: `registry.defer::<heavy::HeavyProps>()` at boot; the
//!   handler installs itself later, from inside the chunk, via
//!   `runtime_scene::defer_registration` → `Registry::register_deferred`.
//!
//! Both apps render `heavy::widget()` from `app()`. In the lazy variant
//! that item has no handler when realize first meets it, so the scene
//! parks it behind a placeholder and completes the mount in place when
//! the chunk's registration lands. The `prune-regression` runner diffs
//! the two `main.wasm` sizes and requires the lazy variant to be ≥ 400
//! KiB smaller.
//!
//! The gate therefore covers three mechanisms at once: `runtime-scene`'s
//! post-boot registration seam keeping a handler out of the boot module,
//! wasm-split placing a chunk-only symbol outside `main.wasm`, and
//! `--data-prune` evicting the now-unreachable static from main's data
//! segments.

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, Typography};
use lazy_payload_split_heavy as heavy;
use runtime_core::{component, ui, Element};

/// The chunk boundary. Byte-identical to the `lazy` sibling's, so the
/// chunk-side work is held constant across the pair.
///
/// `register_from_chunk()` is called here too, and is deliberately inert
/// in this variant: `Registry::register_deferred` returns 0 without
/// installing anything when a boot handler already won. That is what
/// lets one SDK ship both registration paths, and what lets these two
/// fixtures differ by exactly one line.
#[component(lazy)]
fn HeavyChunk() -> Element {
    heavy::register_from_chunk();
    ui! { Typography(content = "heavy handler chunk loaded".to_string()) }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // Rendered by BOTH variants, from `app()` — main's reachability of
    // the *payload constructor* is held constant, so the only variable
    // left is where the handler was registered. Bound out here and
    // splatted by bare identifier: the sanctioned form for an element
    // that arrives from a call (a braced `{ expr }` child directly after
    // a component call would parse as that component's children block).
    let heavy_widget = heavy::widget();

    ui! {
        view {
            Stack(gap = StackGap::Lg) {
                Typography(
                    content = "eager registration \u{2014} heavy handler anchored in main.wasm".to_string(),
                    kind = idea_ui::typography_kind::H2,
                )
                heavy_widget
                HeavyChunk(
                    loading = || ui! { Typography(content = "Loading heavy chunk\u{2026}".to_string()) },
                )
            }
        }
    }
}

/// SDK-handler registration seam, invoked by the CLI-generated wrapper
/// after `runtime_vocabulary::register_builtins`.
///
/// **THE line that differs from the `lazy` sibling.** Registering here
/// makes `mount_heavy` — and the 512 KiB static it reaches — statically
/// reachable from the boot entry, so wasm-split must keep it in
/// `main.wasm`.
pub fn register_scene_extensions<H: runtime_vocabulary::caps::ViewOps>(
    registry: &mut runtime_scene::Registry<H>,
) {
    heavy::register(registry);
}
