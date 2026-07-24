//! EAGER-registration variant. `register_extensions` installs the heavy
//! External handler at boot, so `main.wasm` statically reaches
//! `build_heavy` and carries its 512 KiB `HEAVY` payload.
//!
//! Identical to the `lazy` sibling except for this one thing — where the
//! handler is registered. The lazy component's body renders the exact
//! same `heavy::widget()`. Comparing the two `main.wasm` sizes isolates
//! the cost of the eager registration anchor. See `heavy/src/lib.rs`.

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, Typography};
use lazy_external_split_heavy as heavy;
use runtime_core::{component, ui, Element, IntoElement};

/// The chunk boundary. The handler is already registered (eagerly, at
/// boot) — the chunk only renders the external. No `register_lazy()`
/// call here.
#[component(lazy)]
fn HeavyChunk() -> Element {
    heavy::widget()
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    ui! {
        view {
            Stack(gap = StackGap::Lg) {
                Typography(
                    content = "eager registration \u{2014} heavy SDK anchored in main.wasm".to_string(),
                    kind = idea_ui::typography_kind::H2,
                )
                HeavyChunk(
                    loading = || ui! { Typography(content = "Loading heavy chunk\u{2026}".to_string()) },
                )
            }
        }
    }
}

/// Eager: install the heavy handler at boot. Generic so the web wrapper's
/// `register_extensions(&mut WebBackend)` monomorphizes cleanly and the
/// host build still compiles.
pub fn register_extensions<B: runtime_core::RegisterExternal>(backend: &mut B) {
    heavy::register(backend);
}
