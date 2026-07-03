//! Compile-checked usage **recipes** for the haptics SDK.
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
    impact,
    /// A row of buttons, each firing a different haptic. `impact(style)` is a
    /// physical-impact tap (the five `ImpactStyle`s vary its weight),
    /// `notify(feedback)` plays a success/warning/error pattern, and
    /// `selection()` is a light value-changed tick. All three are
    /// fire-and-forget, synchronous, and best-effort — just call them from a
    /// press handler; no async, no error to handle, a silent no-op where the
    /// device can't.
    pub fn haptics_button_row() -> ::runtime_core::Element {
        use crate::{impact, notify, selection, ImpactStyle, NotificationFeedback};
        use ::runtime_core::ui;

        ui! {
            view {
                button(
                    label = "Light tap",
                    on_click = || impact(ImpactStyle::Light),
                )
                button(
                    label = "Heavy tap",
                    on_click = || impact(ImpactStyle::Heavy),
                )
                button(
                    label = "Success",
                    on_click = || notify(NotificationFeedback::Success),
                )
                button(
                    label = "Error",
                    on_click = || notify(NotificationFeedback::Error),
                )
                button(
                    label = "Selection tick",
                    on_click = || selection(),
                )
            }
        }
    }
);
