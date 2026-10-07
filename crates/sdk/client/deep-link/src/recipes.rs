//! Compile-checked usage **recipes** for the deep-link SDK.
//!
//! Each `recipe!(Target, fn ...)` is a real, type-checked example of how to
//! use the SDK. Because the fn compiles against the live API, a signature
//! change that isn't reflected here is a compile error (whenever the
//! catalog is built), so these examples can't silently rot — and the MCP /
//! docs surface them as trustworthy "how do I use this?" context.
//!
//! `recipe!` self-gates on the `catalog` feature: with it off (every
//! production build) these expand to nothing — the recipes, and the imports
//! inside them, don't compile at all. So there's no `#[cfg]` here and no
//! cost in shipped apps. Recipes are self-contained (imports live inside
//! each fn) so the captured source reads as a complete, copy-pasteable
//! example.

use runtime_core::recipe;

recipe!(
    DeepLink,
    /// Show the most recent inbound deep link.
    ///
    /// The framework already routes every link to its screen; `on_link` is
    /// for app code that wants to see links too (analytics, a banner).
    /// `on_link` returns an RAII `LinkSubscription` — dropping it
    /// unsubscribes. Register it inside an effect and RETURN the guard as the
    /// effect's cleanup: the surrounding component scope owns the effect, so
    /// the subscription lives exactly as long as the component. Each link
    /// writes a `signal` the `text` reads, so the view follows every link.
    pub fn deep_link_listen() -> ::runtime_core::Element {
        use crate::{on_link, DeepLink};
        use ::runtime_core::{effect, signal, text, ui};

        let latest = signal::<Option<DeepLink>>(None);

        let _ = effect(move || {
            let sub = on_link(move |link| latest.set(Some(link)));
            // Returned cleanup: runs before each re-run and at teardown.
            move || drop(sub)
        });

        ui! {
            text(move || match latest.get() {
                Some(link) => format!("opened {}", link.route_path()),
                None => "waiting for a link…".to_string(),
            })
        }
    }
);

recipe!(
    DeepLink,
    /// Hold deep links until the user signs in, then open the held one.
    ///
    /// `intercept` claims a link before the framework routes it (return
    /// `true`). The held link is replayed with `route_link` once signed in.
    /// The cold-start link needs no gate: navigators mounted only after
    /// sign-in still open it, because the launch path waits until the root
    /// navigator mounts.
    pub fn deep_link_auth_gate(signed_in: ::runtime_core::Signal<bool>) -> ::runtime_core::Element {
        use crate::{intercept, route_link, DeepLink};
        use ::runtime_core::{effect, signal, text, ui};

        let held = signal::<Option<DeepLink>>(None);

        let _ = effect(move || {
            let gate = intercept(move |link| {
                if signed_in.peek() {
                    return false; // signed in: let it route
                }
                held.set(Some(link.clone()));
                true
            });
            move || drop(gate)
        });

        // Replay the held link the moment the user signs in.
        let _ = effect(move || {
            if signed_in.get() {
                if let Some(link) = held.peek() {
                    held.set(None);
                    route_link(&link);
                }
            }
        });

        ui! { text("sign in to continue") }
    }
);
