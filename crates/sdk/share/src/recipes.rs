//! Compile-checked usage **recipes** for the share SDK.
//!
//! Each `recipe!(Target, fn ...)` is a real, type-checked example of how to use
//! the SDK. Because the fn compiles against the live API, a signature change
//! that isn't reflected here is a compile error (whenever the catalog is
//! built), so these examples can't silently rot — and the MCP / docs surface
//! them as trustworthy "how do I use this?" context.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every production
//! build) these expand to nothing — the recipes, and the imports inside them,
//! don't compile at all. So there's no `#[cfg]` here and no cost in shipped
//! apps. Recipes are self-contained (imports live inside each fn) so the
//! captured source reads as a complete, copy-pasteable example.

use runtime_core::recipe;

recipe!(
    share,
    /// A "Share" button that hands a link to the system share sheet. Build a
    /// [`ShareContent`] (here some text plus a URL), then `share(...)` it from
    /// the press handler — `share` is async (the sheet is modal), so drive it
    /// with `spawn_async`. The OS shows its own sheet and the future resolves
    /// to [`ShareOutcome::Completed`] or [`ShareOutcome::Dismissed`]. On web,
    /// presenting from a button press satisfies the Web Share API's
    /// user-gesture requirement.
    pub fn share_button() -> ::runtime_core::Element {
        use crate::{share, ShareContent, ShareOutcome};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::ui;

        let on_click = || {
            spawn_async(async {
                let content =
                    ShareContent::text("Check out Idealyst").with_url("https://idealyst.dev");
                match share(content).await {
                    Ok(ShareOutcome::Completed) => { /* sent somewhere */ }
                    Ok(ShareOutcome::Dismissed) => { /* user cancelled */ }
                    Err(_e) => { /* not supported here, or a backend failure */ }
                }
            });
        };
        ui! {
            button(label = "Share", on_click = on_click)
        }
    }
);

recipe!(
    ShareContent,
    /// Build the content to share. Start from a constructor
    /// (`ShareContent::text` / `::url` / `::files`) and chain the `with_*`
    /// setters to add more — text, a URL, file references, and a title/subject
    /// (used as the email subject on targets that have one). Each platform's
    /// share sheet picks the fields it understands.
    pub fn share_content_builder() -> ::runtime_core::Element {
        use crate::{share, ShareContent};
        use ::runtime_core::driver::spawn_async;
        use ::runtime_core::ui;

        let on_click = || {
            let content = ShareContent::text("Trip photos from the weekend")
                .with_url("https://example.com/album/42")
                .with_title("Weekend trip")
                .with_file("/tmp/photo.jpg");
            spawn_async(async move {
                let _ = share(content).await;
            });
        };
        ui! {
            button(label = "Share trip", on_click = on_click)
        }
    }
);
