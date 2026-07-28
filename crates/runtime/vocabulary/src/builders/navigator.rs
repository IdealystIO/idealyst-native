//! Navigator builders: `swap_navigator()`, `stack_navigator()`,
//! `navigator_outlet()` — the new-core counterparts of the SDK
//! `SwapNavigator`/`StackNavigator` fluent builders and the old
//! `navigator_outlet()` free fn.
//!
//! Screens register with a typed [`Route<P>`] + a render closure
//! returning a scene `Element`; the author layout is a plain closure
//! (no context parameter — read [`SwapNav`](crate::prims::SwapNav) /
//! [`StackNav`](crate::prims::StackNav) via `runtime_world::inject`, and
//! splat [`navigator_outlet`] exactly once where the active screen
//! renders).

use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{Route, RouteParams};
use runtime_scene::{item, Element};

use crate::prims::{
    MountPolicy, NavConfig, NavHandle, NavScreenEntry, NavigatorOutletPrim, PrimCell,
    StackNavigatorPrim, StackRetention, SwapNavigatorPrim,
};
use crate::style_attach::IntoStyleProp;

/// Start a `swap_navigator` whose initial (selected) screen is `initial`.
pub fn swap_navigator(initial: &Route<()>) -> SwapNavigatorBuilder {
    SwapNavigatorBuilder {
        prim: SwapNavigatorPrim {
            config: NavConfig {
                initial: initial.name(),
                initial_path: initial.path(),
                screens: HashMap::new(),
            },
            layout: None,
            mount_policy: MountPolicy::default(),
            select_args: HashMap::new(),
            style: None,
            a11y: AccessibilityProps::default(),
            on_handle: None,
        },
    }
}

pub struct SwapNavigatorBuilder {
    prim: SwapNavigatorPrim,
}

impl SwapNavigatorBuilder {
    /// Register a screen: its route and the closure building it from
    /// typed params.
    pub fn screen<P, F>(mut self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        F: Fn(P) -> Element + 'static,
    {
        let (entry, select) = screen_entry(&route, render);
        self.prim.config.screens.insert(route.name(), entry);
        self.prim.select_args.insert(route.name(), select);
        self
    }

    /// Set the author layout — owns the chrome tree, splats
    /// [`navigator_outlet`] where the active screen renders, reads
    /// `inject::<SwapNav>()` for state + `on_select`.
    pub fn layout(mut self, f: impl Fn() -> Element + 'static) -> Self {
        self.prim.layout = Some(Rc::new(f));
        self
    }

    /// Screen mount lifecycle — see [`MountPolicy`].
    pub fn mount_policy(mut self, policy: MountPolicy) -> Self {
        self.prim.mount_policy = policy;
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Receive the imperative [`NavHandle`] at mount.
    pub fn on_handle(mut self, fill: impl FnOnce(NavHandle) + 'static) -> Self {
        self.prim.on_handle = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}

/// Start a `stack_navigator` whose root screen is `initial`.
pub fn stack_navigator(initial: &Route<()>) -> StackNavigatorBuilder {
    StackNavigatorBuilder {
        prim: StackNavigatorPrim {
            config: NavConfig {
                initial: initial.name(),
                initial_path: initial.path(),
                screens: HashMap::new(),
            },
            layout: None,
            retention: StackRetention::default(),
            style: None,
            a11y: AccessibilityProps::default(),
            on_handle: None,
        },
    }
}

pub struct StackNavigatorBuilder {
    prim: StackNavigatorPrim,
}

impl StackNavigatorBuilder {
    /// Register a screen: its route and the closure building it from
    /// typed params.
    pub fn screen<P, F>(mut self, route: Route<P>, render: F) -> Self
    where
        P: RouteParams + 'static,
        F: Fn(P) -> Element + 'static,
    {
        let (entry, _select) = screen_entry(&route, render);
        self.prim.config.screens.insert(route.name(), entry);
        self
    }

    /// Set the author layout (reads `inject::<StackNav>()`).
    pub fn layout(mut self, f: impl Fn() -> Element + 'static) -> Self {
        self.prim.layout = Some(Rc::new(f));
        self
    }

    /// Covered-screen lifecycle — see [`StackRetention`].
    pub fn retention(mut self, r: StackRetention) -> Self {
        self.prim.retention = r;
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Receive the imperative [`NavHandle`] at mount.
    pub fn on_handle(mut self, fill: impl FnOnce(NavHandle) + 'static) -> Self {
        self.prim.on_handle = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}

/// Start a `navigator_outlet` — the placeholder an author layout splats
/// (exactly once) to mark where the enclosing navigator's active screen
/// renders. Style-less outlets default to a bounded, fillable flex
/// region (`outlet_fill_rules`); a `.style(...)` replaces that default.
pub fn navigator_outlet() -> NavigatorOutletBuilder {
    NavigatorOutletBuilder {
        prim: NavigatorOutletPrim {
            style: None,
            a11y: AccessibilityProps::default(),
        },
    }
}

pub struct NavigatorOutletBuilder {
    prim: NavigatorOutletPrim,
}

impl NavigatorOutletBuilder {
    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}

/// Shared screen-registration lowering: the type-erased builder (params
/// downcast — mirrors the SDK builders' panic contract), the
/// segment→params reconstructor, and the bare-name select recipe (url
/// from the route pattern + params from an empty segment map — `Some`
/// only when `P` is constructible without path segments; the old
/// select-args fix).
fn screen_entry<P, F>(
    route: &Route<P>,
    render: F,
) -> (NavScreenEntry, crate::prims::SelectArgs)
where
    P: RouteParams + 'static,
    F: Fn(P) -> Element + 'static,
{
    let route_path = route.path();
    let build: Rc<dyn Fn(Box<dyn Any>) -> Element> = Rc::new(move |any_params: Box<dyn Any>| {
        let typed: Box<P> = any_params
            .downcast::<P>()
            .expect("navigator: route params type mismatch");
        render(*typed)
    });
    let from_segments = Rc::new(
        |segs: &HashMap<String, String>| -> Option<Box<dyn Any>> {
            P::from_segments(segs).map(|p| Box::new(p) as Box<dyn Any>)
        },
    );
    let select: crate::prims::SelectArgs = Rc::new(move || {
        P::from_segments(&HashMap::new())
            .map(|params| (params.to_path(route_path), Box::new(params) as Box<dyn Any>))
    });
    (
        NavScreenEntry { path: route_path, build, from_segments },
        select,
    )
}
