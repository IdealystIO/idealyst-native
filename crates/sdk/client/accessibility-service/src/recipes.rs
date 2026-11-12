//! Compile-checked usage **recipes** for the accessibility-service SDK.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every
//! production build) these expand to nothing. Recipes are self-contained
//! (imports live inside each fn) so the captured source reads as a complete,
//! copy-pasteable example.

use runtime_core::recipe;

recipe!(
    remote_view,
    /// A remote-support status panel: whether the service is switched on
    /// (kept current by `on_enabled_change`), a button that opens Settings to
    /// switch it on, and a button that takes a JPEG screenshot and presses
    /// Back. A real agent sends the frame up its own connection and replays
    /// the console's clicks with `tap` / `swipe`, in the screenshot's pixels.
    pub fn remote_view_panel() -> ::runtime_core::Element {
        use crate::{
            global_action, is_enabled, on_enabled_change, open_settings, screenshot, GlobalAction,
            ImageFormat,
        };
        use ::runtime_core::driver::spawn_detached;
        use ::runtime_core::{on_scope_drop, signal, text, ui, Signal};

        let enabled: Signal<bool> = signal(is_enabled());
        let status: Signal<String> = signal(String::new());
        let subscription = on_enabled_change(move |on| enabled.set(on));
        on_scope_drop(move || drop(subscription));

        let capture = move || {
            spawn_detached(async move {
                status.set(match screenshot(ImageFormat::Jpeg { quality: 70 }).await {
                    Ok(shot) => format!("{} × {}: {} bytes", shot.width, shot.height, shot.data.len()),
                    Err(e) => format!("Screenshot failed: {e}"),
                });
                let _ = global_action(GlobalAction::Back);
            });
        };

        ui! {
            view() {
                text(move || if enabled.get() { "Remote view: on" } else { "Remote view: off" })
                button(label = "Open accessibility settings".to_string(), on_click = move || {
                    let _ = open_settings();
                })
                button(label = "Screenshot, then Back".to_string(), on_click = capture)
                text(move || status.get())
            }
        }
    }
);
