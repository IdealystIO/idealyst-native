//! Compile-checked usage **recipes** for the connectivity SDK.
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
    Connectivity,
    /// Show live online/offline status. Seed a `signal` from a
    /// [`current`](connectivity::current) snapshot, then [`watch`] for
    /// changes — each change writes the new snapshot into the signal, which
    /// the `text` reads reactively.
    ///
    /// The [`watch`] guard must stay alive for as long as you want updates;
    /// here it's anchored to the reactive scope by moving it into a persisted
    /// `Effect` (the framework lifecycle owns it — no `mem::forget`). In a
    /// real app you'd typically hold the guard in your component/app state
    /// instead.
    pub fn connectivity_status() -> ::runtime_core::Element {
        use crate::{current, watch, Connectivity};
        use ::runtime_core::{effect, signal, text, ui};

        // A small label helper so the example reads at a glance.
        fn label(net: Connectivity) -> String {
            if net.online {
                format!("Online via {:?}", net.transport)
            } else {
                "Offline".to_string()
            }
        }

        // Seed from the synchronous snapshot, then keep it fresh.
        let status = signal!(label(current()));

        // Subscribe; on every change, write the fresh snapshot into the
        // signal. Anchor the returned guard to the component scope by moving
        // it into a scope-owned effect — it lives as long as the scope and
        // unregisters on teardown.
        let sub = watch(move |net| status.set(label(net)));
        effect!({
            // Hold the guard so the scope owns it; the body otherwise does
            // nothing (the callback already drives the signal).
            let _keep = &sub;
        });

        ui! {
            view {
                text(move || status.get())
            }
        }
    }
);
