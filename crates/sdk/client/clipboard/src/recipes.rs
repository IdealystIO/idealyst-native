//! Compile-checked usage **recipes** for the `clipboard` SDK.
//!
//! Each `recipe!(target, fn ...)` is a real, type-checked example, compiled
//! against this crate's live API — so an API change that isn't reflected
//! here is a compile error (whenever the `catalog` feature is built). With
//! `catalog` off (every production build) `recipe!` expands to nothing, so
//! these examples and their imports vanish at zero cost.
//!
//! Recipes are self-contained — every `use` lives inside the fn — so the
//! captured source reads as a complete, copy-pasteable example.

use runtime_core::recipe;

recipe!(
    set_text,
    /// Copy a fixed string to the system clipboard, then read it back
    /// into a reactive `signal` and show it. The button's press handler
    /// spawns the async clipboard work (`set_text` / `text` are `async`)
    /// via `runtime_core::driver::spawn_async`, writing the read-back text
    /// into a `Signal<String>` that the `text` primitive displays.
    pub fn clipboard_copy_paste() -> ::runtime_core::Element {
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::{signal, text, ui};

        // The text shown after the copy + read-back round trip.
        let pasted = signal!(String::new());

        // The button's press handler. A bare `Fn()` closure is an
        // `IntoAction`, so it drops straight into `on_click`. `Signal` is
        // `Copy`, so the handler copies `pasted` into each spawned task.
        let on_click = move || {
            spawn_async(async move {
                // Copy a fixed string, then read the clipboard back.
                if crate::set_text("Hello from Idealyst").await.is_ok() {
                    if let Ok(Some(value)) = crate::text().await {
                        pasted.set(value);
                    }
                }
            });
        };

        ui! {
            view {
                button(label = "Copy + read back".to_string(), on_click = on_click)
                // Reactive display: the closure re-reads `pasted` whenever
                // it changes (a `Fn() -> String` is a text source).
                text(move || pasted.get())
            }
        }
    }
);
