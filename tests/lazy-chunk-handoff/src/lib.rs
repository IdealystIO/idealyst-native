//! Regression test for the wasm-split main↔chunk boundary under
//! release-mode data pruning.
//!
//! The `lazy! { … }` block is hoisted into a `#[wasm_split]` async fn
//! and post-processed by `wasm-split-cli` into a separate chunk. The
//! chunk doesn't carry its own copy of every dep — it imports shared
//! code and DATA from the main bundle (idea_ui component vtables,
//! `Signal<T>` static thread-local slots, panic-message strings, the
//! token registry). If the pruning heuristic in main zeroes a data
//! symbol the chunk still imports, the chunk traps on first use with
//! `RuntimeError: null function`, `panicked at :` (empty message), or
//! a torn-string display somewhere visible.
//!
//! This app is intentionally tiny: a heading in the main bundle, a
//! `lazy!` subtree carrying enough surface (button + reactive text +
//! Stack/Typography component vtables + a Signal<u32>) to force the
//! chunk to reach back into main for several different kinds of data.
//!
//! What to look for in the browser:
//! - The main heading paints immediately.
//! - A brief placeholder shows ("Loading chunk…").
//! - The chunk's subtree mounts with a working button: each click
//!   increments the displayed count. No console errors.

use idea_ui::{
    install_idea_theme, light_theme, tone, variant, Button, Stack, StackGap, Typography,
};
use runtime_core::{lazy, rx, signal, ui, Element, IntoElement, Signal};
use std::rc::Rc;

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // The lazy block. `lazy!` v1 doesn't support captures across the
    // boundary, so every signal / closure the chunk uses is created
    // inside the block. The chunk still depends on the main bundle for
    // the Button/Typography/Stack vtables, the `Signal::new` allocator
    // path, the panic infrastructure, and the reactive scheduler — all
    // the things data-pruning could plausibly damage.
    let chunk = lazy! {
        let count: Signal<u32> = signal!(0);
        let inc: Rc<dyn Fn()> = Rc::new(move || count.update(|n| *n += 1));
        ui! {
            view {
                Stack(gap = StackGap::Sm) {
                    Typography(
                        content = "Loaded from a separate wasm chunk".to_string(),
                        kind = idea_ui::typography_kind::H3,
                    )
                    Typography(content = rx!(format!("count = {}", count.get())))
                    Button(
                        label = "Increment (chunk handler)".to_string(),
                        tone = tone::Primary,
                        variant = variant::Soft,
                        on_click = inc,
                    )
                }
            }
        }
    }
    .placeholder(|| {
        ui! { Typography(content = "Loading chunk\u{2026}".to_string()) }
    })
    .into_element();

    ui! {
        view {
            Stack(gap = StackGap::Lg) {
                Typography(
                    content = "lazy-chunk-handoff regression test".to_string(),
                    kind = idea_ui::typography_kind::H2,
                )
                Typography(content = "Heading above is in the main bundle. Subtree below ships in a chunk.".to_string())
                chunk
            }
        }
    }
}

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
