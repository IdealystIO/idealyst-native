//! Compile-checked usage **recipes** for the location SDK.
//!
//! Each `recipe!(Target, fn ...)` is a real, type-checked example of how to
//! use the SDK. Because the fn compiles against the live API, a signature
//! change that isn't reflected here is a compile error (whenever the catalog
//! is built), so these examples can't silently rot — and the MCP / docs
//! surface them as trustworthy "how do I use this?" context.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every
//! production build) these expand to nothing — the recipes, and the imports
//! inside them, don't compile at all. So there's no `#[cfg]` here and no cost
//! in shipped apps. Recipes are self-contained (imports live inside each fn)
//! so the captured source reads as a complete, copy-pasteable example.

use runtime_core::recipe;

recipe!(
    Position,
    /// Read the device's current position on a button press and render the
    /// latitude / longitude into a reactive text. `current()` requests the
    /// location permission (through the `permissions` SDK) and resolves one
    /// fix; the press handler spawns the async call and writes the result
    /// into a `Signal<String>` the `text` reads, so the label updates when
    /// the fix lands. Drop a `watch(..)` guard the same way for continuous
    /// updates.
    pub fn position_on_press() -> ::runtime_core::Element {
        use crate::{current, LocationError};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::{signal, text, ui, Signal};

        // The text the button updates: prompt → fix coordinates (or an error).
        let label: Signal<String> = signal!("Tap to locate".to_string());

        let on_press = move || {
            label.set("Locating…".to_string());
            // `current()` is async (it awaits the permission grant, then a
            // fix); spawn it off the press handler and write the result back.
            spawn_async(async move {
                match current().await {
                    Ok(pos) => label.set(format!(
                        "{:.5}, {:.5} (±{:.0} m)",
                        pos.latitude, pos.longitude, pos.accuracy_m
                    )),
                    Err(LocationError::NotAuthorized) => {
                        label.set("Location permission denied".to_string())
                    }
                    Err(e) => label.set(format!("No fix: {e}")),
                }
            });
        };

        ui! {
            view() {
                // Reactive: re-reads `label` whenever it changes.
                text(move || label.get())
                button(label = "Where am I?".to_string(), on_click = on_press)
            }
        }
    }
);
