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
    /// Subscribe to inbound deep links and render the most recent one.
    ///
    /// `on_link` registers a handler inside an `Effect` and returns an RAII
    /// `LinkSubscription`; keeping it alive in a session signal keeps the
    /// subscription alive for the component's lifetime. Each link updates a
    /// reactive `signal`, and the `text` reads it so the view re-renders on
    /// every link. `feed_link` is what the platform host calls when the OS
    /// delivers a URL — here we call it ourselves to demonstrate the flow.
    pub fn deep_link_listen() -> ::runtime_core::Element {
        use crate::{feed_link, on_link, DeepLink};
        use ::runtime_core::session::signal;
        use ::runtime_core::{effect, text, ui};

        // The latest inbound link, rendered reactively below.
        let latest = signal::<Option<DeepLink>>("deep_link.latest", None);

        // Subscribe once, inside a scope-owned effect, and park the RAII
        // guard in a session signal so it lives as long as this scope.
        // Dropping the guard unsubscribes — so we must NOT drop it at the
        // end of setup.
        let sub_slot = signal::<::std::rc::Rc<::std::cell::RefCell<Option<crate::LinkSubscription>>>>(
            "deep_link.sub",
            ::std::rc::Rc::new(::std::cell::RefCell::new(None)),
        );
        effect!({
            let sink = latest;
            let sub = on_link(move |link| sink.set(Some(link)));
            *sub_slot.get().borrow_mut() = Some(sub);
        });

        // Demonstrate the host ingress: feed a link as the OS would.
        feed_link("myapp://demo/path?x=1");

        ui! {
            text(move || match latest.get() {
                Some(link) => format!("{}://{}{}", link.scheme, link.host.unwrap_or_default(), link.path),
                None => "waiting for a link…".to_string(),
            })
        }
    }
);
