//! Link primitive — declarative navigation.
//!
//! `Link(route, params) { children }` is the declarative counterpart
//! to `NavigatorHandle::push`. It wraps content in a tappable
//! container; activation dispatches a nav command (`Push` by
//! default; `.kind(NavKind::Replace | Reset)` switches semantics)
//! against the **ambient navigator** — the nearest enclosing
//! `Navigator` whose `mount_screen` is currently building this
//! screen subtree.
//!
//! # Why a primitive, not just `Button` + `nav.push`?
//!
//! - **Web semantics.** Backends are free to emit a real `<a href>`
//!   so the browser's link contract works without re-implementation:
//!   hover URL preview, right-click "copy link," middle-click and
//!   cmd-click for new tab/window, keyboard activation, screen-reader
//!   "link" role, search-engine crawlability.
//! - **Static introspection.** A primitive lets future tooling
//!   extract the declared link graph; imperative dispatch can't be
//!   inspected.
//! - **No prop drilling.** The ambient navigator wiring means
//!   authors don't have to thread a `Ref<NavigatorHandle>` through
//!   every component crossing a screen boundary.
//!
//! # Ambient navigator
//!
//! The framework's `Navigator` pushes its `Rc<NavigatorControl>`
//! onto a thread-local stack while running each `mount_screen`
//! call. `link(...)` reads the top of that stack at construction
//! time and captures it; on activation it dispatches through that
//! captured control plane.
//!
//! A link constructed outside any screen has no ambient navigator
//! and silently no-ops on activation (matches the
//! handle-before-build posture of the rest of the navigator
//! system). Nested navigators target correctly — each one pushes
//! its own control plane while building its screens, so a `Link`
//! inside a child navigator's screen drives the child by default.

use crate::primitives::navigator::{
    ambient_navigator, NavCommand, NavigatorControl, Route, RouteParams,
};
use crate::{Bound, Element, Ref, RefFill};
use std::any::Any;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// NavKind — which nav command the link dispatches on activation
// ---------------------------------------------------------------------------

/// How activation maps to a `NavCommand`.
///
/// `Default` defers to the SDK-installed link activator on the
/// ambient `NavigatorControl` — stack SDKs typically don't install
/// one and the activator falls through to `Push`; tab/drawer SDKs
/// install one that returns `Select`. Authors can override per-link
/// with an explicit kind.
///
/// `Pop` isn't a link kind — a hyperlink that navigates backward
/// isn't a hyperlink, it's a back button. Use a regular `Button` +
/// `nav.pop()` for that.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum NavKind {
    /// Use the SDK-installed link activator on the ambient navigator,
    /// or fall back to `Push` when none is installed.
    Default,
    Push,
    Replace,
    Reset,
    Select,
}

impl Default for NavKind {
    fn default() -> Self {
        NavKind::Default
    }
}

// ---------------------------------------------------------------------------
// LinkHandle — imperative API for refs
// ---------------------------------------------------------------------------

/// Handle exposed via `Ref<LinkHandle>`. Lets a parent fire a
/// link's nav command programmatically — useful for "press enter
/// on a focused row triggers its link" patterns where there's no
/// synthesizable click event.
#[derive(Clone)]
pub struct LinkHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn LinkOps,
}

impl LinkHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn LinkOps) -> Self {
        Self { node, ops }
    }

    /// Fire the link's nav command. Same effect as a user tap /
    /// click on the rendered widget.
    pub fn activate(&self) {
        self.ops.activate(&*self.node);
    }
}

pub trait LinkOps {
    fn activate(&self, node: &dyn Any);
}

// ---------------------------------------------------------------------------
// LinkConfig — what `Backend::create_link` receives
// ---------------------------------------------------------------------------

/// Bundle the framework hands to `Backend::create_link`. The
/// backend wires the platform-native interaction widget (a real
/// `<a href>` on web, an accessibility-Link-roled tappable
/// container on native) and calls `on_activate` when the user
/// activates it.
pub struct LinkConfig {
    /// Route name (matches `Route::name()`). Stable; passed through
    /// to backends that want to expose it in accessibility metadata
    /// (e.g. "Link to home"). Empty (`""`) for external links — they
    /// have no in-app route; use [`url`](Self::url) for the label.
    pub route: &'static str,
    /// Concrete URL. For in-app links: `params.to_path(route.path)`,
    /// used on web for the `<a href>` and right-click affordances,
    /// ignored on native. For external links: the off-app destination
    /// (`https://…`, `mailto:`, `tel:`) the backend opens directly.
    pub url: String,
    /// `true` ⇒ this link points *outside* the app. Backends route it
    /// to the platform's external handler rather than the in-app
    /// navigator: web emits `<a target="_blank" rel="noopener">` and
    /// lets the browser navigate (no SPA `preventDefault`); native
    /// fires `on_activate`, which calls
    /// [`open_url`](crate::open_url). `false` ⇒ in-app navigation
    /// (the historical behavior).
    pub external: bool,
    /// Fire when the user activates the link. For in-app links the
    /// framework wraps push/replace/reset dispatch in here; for
    /// external links it wraps [`open_url`](crate::open_url). Either
    /// way the backend just fires it on activation. (Web skips this
    /// for external links — the native `<a target="_blank">` already
    /// navigates.)
    pub on_activate: Rc<dyn Fn()>,
}

// ---------------------------------------------------------------------------
// Constructor
// ---------------------------------------------------------------------------

/// Build a Link.
///
/// `params: P` must match the route's `P` — the type system
/// enforces this the same way `NavigatorHandle::push` does.
///
/// `P: Clone` is required because the link's underlying primitive
/// may be activated multiple times (every click reproduces a fresh
/// boxed param payload for the dispatcher). For most apps `P` is
/// either `()` or a small `#[derive(Clone)]` struct, so the bound
/// is trivially met.
pub fn link<P: RouteParams + Clone>(
    route: &Route<P>,
    params: P,
    children: Vec<Element>,
) -> Bound<LinkHandle> {
    // Pre-compute the URL once. Web uses it for the `<a href>` and
    // right-click "copy link"; native backends ignore it but the
    // cost is one stringify, so unconditional is fine.
    let url = params.to_path(route.path());
    let route_name: &'static str = route.name();

    // Capture the ambient navigator at construction time. A link
    // built outside any screen captures `None` and no-ops on
    // activation. The actual dispatch shape (Push vs Select vs SDK-
    // specific) is resolved at activation time via the captured
    // control plane's link-activator (or `Push` as fallback).
    let ambient: Option<Rc<NavigatorControl>> = ambient_navigator();
    let kind = NavKind::Default;

    // Type-erased params source. Each activation needs a fresh
    // `Box<dyn Any>` because `NavCommand::Push`/etc. own their
    // params. `P: Clone` is what lets us reproduce on demand.
    let params_rc: Rc<P> = Rc::new(params);
    let make_params: Rc<dyn Fn() -> Box<dyn Any>> = {
        let params_rc = params_rc.clone();
        Rc::new(move || Box::new((*params_rc).clone()) as Box<dyn Any>)
    };

    Bound::new(Element::Link {
        children,
        route: route_name,
        url,
        make_params,
        kind,
        target: ambient,
        external: false,
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
    })
}

/// Build an **external** Link — one that leaves the app for an
/// off-app destination (`https://…`, `mailto:`, `tel:`).
///
/// Unlike [`link`], an external link has no `Route` and captures no
/// ambient navigator: on web the backend renders a real
/// `<a href target="_blank" rel="noopener">` and lets the browser
/// navigate (so it's never popup-blocked, unlike a programmatic
/// `window.open`); on native, activation calls
/// [`open_url`](crate::open_url), which hands the URL to the
/// platform's external handler (`UIApplication.open`, an
/// `ACTION_VIEW` intent, `NSWorkspace`).
///
/// Use this for GitHub links, docs, `mailto:` etc. For in-app
/// navigation between routes, use [`link`] so web stays single-page.
pub fn external_link(url: impl Into<String>, children: Vec<Element>) -> Bound<LinkHandle> {
    // External links carry no params; the dispatcher never reads this
    // (the walker builds an `open_url` closure instead of a
    // NavCommand), but the field is non-optional so supply a noop.
    let make_params: Rc<dyn Fn() -> Box<dyn Any>> =
        Rc::new(|| Box::new(()) as Box<dyn Any>);

    Bound::new(Element::Link {
        children,
        route: "",
        url: url.into(),
        make_params,
        kind: NavKind::Default,
        target: None,
        external: true,
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
    })
}

// ---------------------------------------------------------------------------
// Bound<LinkHandle> — builder methods
// ---------------------------------------------------------------------------

impl Bound<LinkHandle> {
    /// Switch dispatch shape. Default is `NavKind::Push`.
    pub fn kind(mut self, k: NavKind) -> Self {
        if let Element::Link { kind, .. } = &mut self.primitive {
            *kind = k;
        }
        self
    }

    /// Bind to a `Ref<LinkHandle>` for imperative `activate()`.
    pub fn bind(mut self, r: Ref<LinkHandle>) -> Self {
        if let Element::Link { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Link(Box::new(move |h| r.fill(h))));
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Internals shared with the walker
// ---------------------------------------------------------------------------

/// Build the activation closure for a link primitive. The walker
/// hands this to the backend as `LinkConfig::on_activate`.
///
/// `target` is the ambient navigator the link captured at
/// construction. `None` ⇒ no-op (no nav was active when the link
/// was built; activation is silently dropped).
pub(crate) fn make_on_activate(
    target: Option<Rc<NavigatorControl>>,
    route: &'static str,
    url: String,
    kind: NavKind,
    make_params: Rc<dyn Fn() -> Box<dyn Any>>,
) -> Rc<dyn Fn()> {
    Rc::new(move || {
        let Some(control) = target.as_ref() else { return };
        let url = url.clone();
        let params = make_params();
        let cmd = match kind {
            NavKind::Default => control.build_link_command(route, url, params),
            NavKind::Push => NavCommand::Push { name: route, url, params, state: None },
            NavKind::Replace => NavCommand::Replace { name: route, url, params, state: None },
            NavKind::Reset => NavCommand::Reset { name: route, url, params, state: None },
            NavKind::Select => NavCommand::Select { name: route, url, params, state: None },
        };
        control.dispatch(cmd);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::navigator::{AmbientNavGuard, NavCommand};
    use crate::style::StyleRules;
    use std::cell::RefCell;

    /// Test backend that wires `Link` activation so a test can OBSERVE
    /// whether a link dispatched. The default `make_link_handle` returns
    /// a no-op `LinkOps`, so we override `create_link` + `make_link_handle`
    /// to keep each node's `on_activate` closure in a side table and fire
    /// it from `LinkHandle::activate()`.
    struct LinkTestBackend {
        next_id: RefCell<u32>,
        // node id -> the link's on_activate closure (built by the walker
        // from the captured ambient navigator).
        activations: Rc<RefCell<std::collections::HashMap<u32, Rc<dyn Fn()>>>>,
    }

    impl LinkTestBackend {
        fn new() -> Self {
            Self {
                next_id: RefCell::new(0),
                activations: Rc::new(RefCell::new(std::collections::HashMap::new())),
            }
        }
        fn mint(&self) -> u32 {
            let id = *self.next_id.borrow();
            *self.next_id.borrow_mut() = id + 1;
            id
        }
    }

    /// `LinkOps` that fires the closure stored for the link's node id.
    struct TableLinkOps {
        activations: Rc<RefCell<std::collections::HashMap<u32, Rc<dyn Fn()>>>>,
    }
    impl LinkOps for TableLinkOps {
        fn activate(&self, node: &dyn Any) {
            let id = *node.downcast_ref::<u32>().expect("link node is u32");
            let cb = self.activations.borrow().get(&id).cloned();
            if let Some(cb) = cb {
                cb();
            }
        }
    }

    impl crate::Backend for LinkTestBackend {
        type Node = u32;

        fn create_view(
            &mut self,
            _a11y: &crate::accessibility::AccessibilityProps,
        ) -> Self::Node {
            self.mint()
        }
        fn create_text(
            &mut self,
            _content: &str,
            _a11y: &crate::accessibility::AccessibilityProps,
        ) -> Self::Node {
            self.mint()
        }
        fn create_button(
            &mut self,
            _label: &str,
            _on_click: &crate::derive::Action,
            _leading: Option<&crate::primitives::icon::IconData>,
            _trailing: Option<&crate::primitives::icon::IconData>,
            _a11y: &crate::accessibility::AccessibilityProps,
        ) -> Self::Node {
            self.mint()
        }
        fn create_link(
            &mut self,
            config: LinkConfig,
            _a11y: &crate::accessibility::AccessibilityProps,
        ) -> Self::Node {
            let id = self.mint();
            self.activations.borrow_mut().insert(id, config.on_activate);
            id
        }
        fn make_link_handle(&self, node: &Self::Node) -> LinkHandle {
            // The ops needs the side table; leak a per-backend ops object
            // so it can be a `&'static dyn LinkOps` (test-only — one per
            // backend instance, lives for the process).
            let ops: &'static TableLinkOps = Box::leak(Box::new(TableLinkOps {
                activations: self.activations.clone(),
            }));
            LinkHandle::new(Rc::new(*node), ops)
        }
        fn insert(&mut self, _parent: &mut Self::Node, _child: Self::Node) {}
        fn update_text(&mut self, _node: &Self::Node, _content: &str) {}
        fn clear_children(&mut self, _node: &Self::Node) {}
        fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {}
        fn finish(&mut self, _root: Self::Node) {}
    }

    /// Regression: a `link` rebuilt by a reactive `when` region (its
    /// `then` branch re-running inside the region's Effect, AFTER the
    /// screen build returned and the screen's `AmbientNavGuard` dropped)
    /// must keep navigating. The ambient navigator is captured at
    /// link CONSTRUCTION time; the rebuild happens with no guard on the
    /// stack, so without the ambient-context snapshot the rebuilt link
    /// captures `None` and silently no-ops.
    #[test]
    fn regression_reactively_remounted_link_keeps_navigator() {
        use crate::reactive::Signal;

        // The navigator + an installed link activator that records every
        // dispatched route name. Recording is how we OBSERVE the link
        // actually fired (vs. silently capturing `None`).
        let dispatched: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let control = Rc::new(NavigatorControl::new());
        {
            let dispatched = dispatched.clone();
            control.install_link_activator(Rc::new(
                move |name: &'static str, url: String, _params: Box<dyn Any>| {
                    dispatched.borrow_mut().push(name.to_string());
                    // Return a harmless command; the recording above is
                    // the observable side effect we assert on. No SDK
                    // dispatcher is installed, so `dispatch` won't forward.
                    NavCommand::Push { name, url, params: Box::new(()), state: None }
                },
            ));
        }

        static ROUTE: Route<()> = Route::new("detail", "/detail");

        let backend = Rc::new(RefCell::new(LinkTestBackend::new()));
        let link_ref: Ref<LinkHandle> = Ref::new();
        // Condition: when `cond` is true the `then` branch builds the
        // link. Toggling cond re-runs the region's Effect; toggling it
        // back to true rebuilds `then`, refilling `link_ref` with a
        // freshly-built link.
        let cond = Signal::new(true);

        // Mount with the navigator ambient ON the stack, exactly like
        // `mount_screen` does for the screen build.
        let _ambient = AmbientNavGuard::push(control.clone());
        let _owner = crate::render(backend.clone(), {
            let link_ref = link_ref;
            crate::element::Element::When {
                cond: {
                    let cond = cond;
                    crate::derive::IntoDerived::into_derived(move || cond.get())
                },
                then: Box::new(move || {
                    link(&ROUTE, (), Vec::new()).bind(link_ref).into()
                }),
                otherwise: Box::new(|| {
                    crate::builder::view(Vec::new()).into()
                }),
                style: None,
            }
        });

        // FIRST build: link captured the ambient navigator. Activating
        // it must dispatch once.
        link_ref
            .with(|h| h.activate())
            .expect("link should be mounted after first build");
        assert_eq!(
            *dispatched.borrow(),
            vec!["detail".to_string()],
            "link must navigate on first build",
        );

        // Drop the ambient guard BEFORE triggering the rebuild — this is
        // the real-world situation: the reactive Effect re-fires long
        // after the screen build returned and its guard was dropped.
        drop(_ambient);
        assert!(
            crate::primitives::navigator::ambient_navigator().is_none(),
            "ambient nav must be empty when the rebuild fires",
        );

        // TRIGGER REBUILD: flip false then back to true so the `then`
        // branch is rebuilt and `link_ref` is refilled with a brand-new
        // link element built inside the region's Effect.
        cond.set(false);
        cond.set(true);

        // The REBUILT link must STILL navigate. Against the buggy code
        // the rebuilt link captured `None` (no ambient guard on the
        // stack during the Effect rebuild) and `activate()` no-ops, so
        // the activator is never called again.
        link_ref
            .with(|h| h.activate())
            .expect("link should be remounted after rebuild");
        assert_eq!(
            *dispatched.borrow(),
            vec!["detail".to_string(), "detail".to_string()],
            "reactively-remounted link must keep its navigator and dispatch again",
        );
    }

    /// `external_link` must build a `Element::Link` flagged
    /// external, carrying the raw URL, with no route and no captured
    /// navigator — the walker keys off `external` to route activation
    /// to `open_url` and the web backend to emit `<a target="_blank">`.
    #[test]
    fn external_link_builds_external_primitive() {
        let bound = external_link("https://example.com/docs", Vec::new());
        match &bound.primitive {
            Element::Link { external, url, route, target, .. } => {
                assert!(*external, "external_link must set external = true");
                assert_eq!(url, "https://example.com/docs");
                assert_eq!(*route, "", "external links carry no in-app route");
                assert!(target.is_none(), "external links capture no navigator");
            }
            _ => panic!("external_link must build a Element::Link"),
        }
    }
}
