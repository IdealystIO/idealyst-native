//! Compile-checked usage **recipes** for the permissions SDK.
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
    Permission,
    /// Request a permission on a button press and show the resulting grant
    /// state. `request` is `async` (the OS prompt resolves later), so the
    /// press handler spawns it with the framework's `spawn_async` and writes
    /// the outcome into a `signal` the `text` reads reactively. The same
    /// shape works for any [`Permission`] — swap the variant.
    pub fn request_notifications() -> ::runtime_core::Element {
        use crate::{request, Permission, PermissionStatus};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::{signal, text, ui};

        // Reactive label, seeded before the user has decided.
        let status = signal!("Notifications: not requested".to_string());

        let on_click = {
            let status = status.clone();
            move || {
                let status = status.clone();
                // The prompt resolves on a later turn; drive the future with
                // the framework's executor and push the result into the
                // signal, which re-renders the text.
                spawn_async(async move {
                    let text = match request(Permission::Notifications).await {
                        PermissionStatus::Granted => "Notifications: granted",
                        PermissionStatus::Denied => "Notifications: denied",
                        PermissionStatus::Restricted => "Notifications: restricted",
                        PermissionStatus::Undetermined => "Notifications: not decided",
                        PermissionStatus::Unsupported => "Notifications: not needed here",
                    };
                    status.set(text.to_string());
                });
            }
        };

        ui! {
            view {
                text(move || status.get())
                button(label = "Enable notifications".to_string(), on_click = on_click)
            }
        }
    }
);
