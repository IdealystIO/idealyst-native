//! LAZY-registration variant. `register_extensions` is empty; the heavy
//! External handler registers itself from *inside* the lazy component's
//! body via `runtime_core::defer_external_registration`. Because nothing
//! in `main.wasm` statically reaches `build_heavy`, the release
//! data-prune drops its 512 KiB `HEAVY` payload from main — it ships in
//! the chunk instead, loaded on demand.
//!
//! Identical to the `eager` sibling except for where the handler is
//! registered (here, the first line of the lazy component's body).
//! Comparing the two `main.wasm` sizes is the measurement. See
//! `heavy/src/lib.rs`.

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, Typography};
use lazy_external_split_heavy as heavy;
use runtime_core::{component, ui, Element, IntoElement};

/// The chunk boundary. Register-then-render, both inside the chunk:
/// `register_lazy()` queues the handler as the chunk loads;
/// `create_external` drains the queue before dispatching `widget()`'s
/// external. main.wasm never sees `build_heavy`.
#[component(lazy)]
fn HeavyChunk() -> Element {
    heavy::register_lazy();
    heavy::widget()
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    ui! {
        view {
            Stack(gap = StackGap::Lg) {
                Typography(
                    content = "lazy registration \u{2014} heavy SDK confined to the chunk".to_string(),
                    kind = idea_ui::typography_kind::H2,
                )
                HeavyChunk(
                    loading = || ui! { Typography(content = "Loading heavy chunk\u{2026}".to_string()) },
                )
            }
        }
    }
}

/// Lazy: nothing registered at boot. The heavy handler installs itself
/// from the chunk. Generic no-op body keeps the web wrapper + host build
/// happy.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
