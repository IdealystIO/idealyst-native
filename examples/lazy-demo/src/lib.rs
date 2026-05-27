//! `lazy-demo` — minimal demo of `lazy! { … }`, the framework's
//! code-splitting primitive.
//!
//! Single crate. The `lazy! { … }` block compiles to a `#[wasm_split]`
//! async function which the post-build `wasm-split-cli` step extracts
//! into a separate wasm chunk on web. On native (terminal / iOS /
//! macOS / Android) the macro is transparent — the function is just
//! compiled into the same binary.
//!
//! What you'll see:
//!
//! - **Native (terminal etc.)**: the chunk's content renders inline,
//!   instantly. `on_state` fires `Rendered` synchronously.
//! - **Web**: a placeholder shows while the chunk wasm downloads;
//!   `on_state` walks `Loading` → `Rendered` as it lands. The main
//!   bundle is dramatically smaller than if the lazy block were
//!   compiled in.

use runtime_core::primitives::lazy::{lazy_split, LazyState};
use runtime_core::{lazy, signal, ui, IntoPrimitive, Primitive};

pub fn app() -> Primitive {
    let state = signal!(LazyState::Loading);
    let state_for_label = state.clone();
    let status = move || match state_for_label.get() {
        LazyState::Loading => "status: Loading chunk...".to_string(),
        LazyState::Loaded => "status: Loaded; mounting...".to_string(),
        LazyState::Rendered => "status: Rendered ✓".to_string(),
        LazyState::Error(e) => format!("status: Error — {e}"),
    };

    // The lazy! macro hoists this block into a #[wasm_split] async
    // fn. On web, wasm-split-cli extracts it post-build; on native
    // the macro is transparent and the block compiles inline.
    let chunk: Primitive = lazy_lifecycle_wrapper(
        lazy! {
            ui! {
                View {
                    Text { "[chunk says] Hello from the lazy chunk!" }
                    Text { "(rendered by lazy! { ... })" }
                }
            }
        },
        move |s| state.set(s),
    );

    let title = "Lazy Primitive Demo";
    let intro = "The status line below reflects the chunk's lifecycle:";

    ui! {
        View {
            Text { title }
            Text { intro }
            Text { status }
            chunk
        }
    }
}

// Tiny adapter that attaches the demo's lifecycle observer + a
// "loading..." placeholder to the macro-produced LazyBuilder. The
// macro itself returns a `LazyBuilder` so the author can chain
// `.on_state` / `.placeholder` / `.with_style`; this helper just
// keeps the call site tidy.
fn lazy_lifecycle_wrapper(
    builder: runtime_core::primitives::lazy::LazyBuilder,
    on_state: impl Fn(LazyState) + 'static,
) -> Primitive {
    builder
        .on_state(on_state)
        .placeholder(|| {
            ui! {
                View {
                    Text { "(loading chunk...)" }
                }
            }
            .into_primitive()
        })
        .into_primitive()
}

// Silence the unused import (lazy_split is the macro's expansion
// target — surfaced here for completeness even though author code
// almost never touches it directly).
#[allow(dead_code)]
fn _refer_lazy_split() {
    let _ = lazy_split::<fn() -> _>;
}

#[cfg(target_arch = "wasm32")]
pub fn register_extensions(_backend: &mut backend_web::WebBackend) {}

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_ios_mobile::IosBackend) {}

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub fn register_extensions(_backend: &mut backend_android_mobile::AndroidBackend) {}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn register_extensions(_backend: &mut backend_terminal::TerminalBackend) {}
