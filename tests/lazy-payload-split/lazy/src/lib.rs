//! LAZY variant of the handler-registration split measurement. Its
//! `register_scene_extensions` registers NOTHING — it only declares the
//! heavy SDK's payload kind late-bound
//! (`Registry::defer::<heavy::HeavyProps>()`, which costs `main.wasm` a
//! compile-time `TypeId` and nothing else). The handler installs itself
//! from inside the `#[component(lazy)]` chunk, so nothing in `main.wasm`
//! statically reaches `mount_heavy`: wasm-split confines it to the chunk
//! and the release data-prune drops its 512 KiB `HEAVY` static from
//! main's data segments.
//!
//! # What this pair measures
//!
//! **Where a third-party payload's HANDLER is registered.** This file and
//! the `eager/` sibling are identical — same `app()`, same rendered tree,
//! same `#[component(lazy)]` chunk body — except for one line:
//!
//! - `eager/`: `heavy::register(registry)` at boot.
//! - `lazy/` (here): `registry.defer::<heavy::HeavyProps>()` at boot; the
//!   handler installs itself later, from inside the chunk, via
//!   `runtime_scene::defer_registration` → `Registry::register_deferred`.
//!
//! `app()` renders `heavy::widget()` in BOTH variants. Here that item has
//! no handler when realize first meets it: because the kind was declared
//! deferred, the scene PARKS it behind a layout-transparent placeholder
//! instead of panicking, and realizes it in place — same node, same
//! position, no remount — the moment the chunk's registration lands. A
//! payload kind that was NOT declared still panics at realize.
//!
//! The `prune-regression` runner diffs the two `main.wasm` sizes and
//! requires this variant to be ≥ 400 KiB smaller. The gate therefore
//! covers three mechanisms at once: `runtime-scene`'s post-boot
//! registration seam keeping a handler out of the boot module,
//! wasm-split placing a chunk-only symbol outside `main.wasm`, and
//! `--data-prune` evicting the now-unreachable static from main's data
//! segments.

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, Typography};
use lazy_payload_split_heavy as heavy;
use runtime_core::{component, ui, Element};

/// The chunk boundary. Byte-identical to the `eager` sibling's — and
/// here it is the ONLY thing that reaches `heavy::mount_heavy`, so
/// wasm-split confines the handler and its payload to the chunk.
///
/// `register_from_chunk()` queues the handler on the scene's
/// late-registration mailbox; the realization of THIS body's element is
/// what drains it, which is also what completes the item `app()` parked
/// in the main tree.
#[component(lazy)]
fn HeavyChunk() -> Element {
    heavy::register_from_chunk();
    ui! { Typography(content = "heavy handler chunk loaded".to_string()) }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // Rendered by BOTH variants, from `app()` — main's reachability of
    // the *payload constructor* is held constant, so the only variable
    // left is where the handler was registered. Here the item parks
    // until the chunk arrives. Bound out and splatted by bare
    // identifier: the sanctioned form for an element that arrives from a
    // call (a braced `{ expr }` child directly after a component call
    // would parse as that component's children block).
    let heavy_widget = heavy::widget();

    ui! {
        view {
            Stack(gap = StackGap::Lg) {
                Typography(
                    content = "lazy registration \u{2014} heavy handler confined to the chunk".to_string(),
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
/// **THE line that differs from the `eager` sibling.** No handler is
/// installed — only the declaration that one arrives late, which is what
/// licenses realize to park `heavy::widget()`'s item instead of
/// panicking on it. `main.wasm` never names `mount_heavy`.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    registry: &mut runtime_scene::Registry<H>,
) {
    registry.defer::<heavy::HeavyProps>();
}
