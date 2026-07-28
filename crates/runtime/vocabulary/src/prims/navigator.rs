//! Navigator payloads: `swap_navigator`, `stack_navigator`, and the
//! `navigator_outlet` — the outlet-model navigation primitives, ported
//! from the old `Element::Navigator` substrate + the backend-neutral
//! `swap-navigator` / `stack-navigator` SDK handlers.
//!
//! # What moved where (vs the old core)
//!
//! - The old split — framework substrate (`walker/navigator.rs` +
//!   `primitives::navigator::shared`) vs SDK handler crates — collapses:
//!   swap and stack are ordinary vocabulary primitives mounted through
//!   the registry, exactly like `view`. The routing *data* types that are
//!   Element-free and pure (`Route`, `RouteParams`, `NavCommand`,
//!   `join_path`/`match_prefix`/`match_pattern`, the nav-base and
//!   screen-state/route thread-local guards, the fill-rule constructors)
//!   are REUSED from `runtime_core::primitives::navigator` — the
//!   sanctioned transitional dependency; they migrate here at P7.
//! - `SwapContext`/`StackContext` become world-context values
//!   ([`SwapNav`] / [`StackNav`], `provide`d by the handler around the
//!   author layout build and read back with `inject`), and the one-shot
//!   `ctx.outlet` element is replaced by the plain
//!   [`navigator_outlet`](crate::builders::navigator_outlet) builder.
//! - `NavigatorControl` / `NavigatorHost` / `retain_scope` / the
//!   keepalive+release_navigator cycle-break machinery all disappear:
//!   nav-state signals and the dispatch driver effect are created inside
//!   the mount handler, so the enclosing `Realized` owns them — one drop
//!   tears the whole navigator down (design §4: "the retain_scope
//!   machinery simplifies").
//!
//! Screens are held as retained [`runtime_scene::Realized`] subtrees:
//! cached (persistent-policy) screens keep their nodes + reactive scope
//! while DETACHED from the tree, and are re-inserted on return without
//! re-running the route builder; dropping the `Realized` is the entire
//! screen teardown.

use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::navigator::{NavCommand, Route, RouteParams};
use runtime_scene::Element;
use runtime_world::Signal;

use crate::style_attach::StyleProp;

// ===========================================================================
// Routing config (the new-core `NavigatorConfig` / `RouteEntry`)
// ===========================================================================

/// Build typed params back from `:segment` captures — same alias as the
/// old `ParamsFromSegments`, re-declared here because it pairs with the
/// new-core screen builder.
pub type ParamsFromSegments = Rc<dyn Fn(&HashMap<String, String>) -> Option<Box<dyn Any>>>;

/// One registered route: its (navigator-relative) URL pattern, the
/// type-erased screen builder, and the segment→params reconstructor.
/// The old `RouteEntry`, with the builder returning a scene `Element`.
pub struct NavScreenEntry {
    pub path: &'static str,
    pub build: Rc<dyn Fn(Box<dyn Any>) -> Element>,
    pub from_segments: ParamsFromSegments,
}

/// The kind-agnostic routing config (old `NavigatorConfig`, minus
/// `defer_initial_mount` — web's deferred URL-driven initial mount is
/// P3-web work and rides the url_sync port, not this handler).
pub struct NavConfig {
    pub initial: &'static str,
    pub initial_path: &'static str,
    pub screens: HashMap<&'static str, NavScreenEntry>,
}

// ===========================================================================
// Value types (ported from the SDK crates — the vocabulary cannot depend
// on swap-navigator/stack-navigator, so the enums are re-declared)
// ===========================================================================

/// When a swap screen's subtree is materialized and whether it survives
/// a switch away (old `swap_navigator::MountPolicy`). NOTE the old
/// handler never implemented eager mounting — `EagerPersistent` behaves
/// exactly like `LazyPersistent` there, and this port preserves that
/// behavior rather than inventing it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum MountPolicy {
    /// Reserved: behaves like `LazyPersistent` (old-handler parity).
    EagerPersistent,
    /// Mount on first activation; keep mounted (detached `Realized`)
    /// across switches — the default.
    #[default]
    LazyPersistent,
    /// Mount on first activation; DROP the screen's `Realized` when
    /// switched away — re-mounts fresh on return.
    LazyDisposing,
}

/// What happens to a stack screen when a `push` covers it (old
/// `stack_navigator::StackRetention`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum StackRetention {
    /// Resolve at mount: `Rebuild` on `Platform::Web`, `Retain`
    /// everywhere else.
    #[default]
    PlatformDefault,
    /// Covered screens stay alive (retained `Realized`); pop reveals
    /// them with state intact. Native-stack semantics.
    Retain,
    /// Covered screens are disposed on push; pop re-mounts from the
    /// URL. Browser semantics.
    Rebuild,
}

/// Builds a route's `(url, typed_params)` for a bare-name selection
/// (`SwapNav::on_select`). `None` when the route needs path params a
/// bare name can't supply — the old select-args fix: dispatching
/// placeholder `""`/`()` values corrupted the path mirror and panicked
/// on non-unit-params routes.
pub type SelectArgs = Rc<dyn Fn() -> Option<(String, Box<dyn Any>)>>;

// ===========================================================================
// World-context values (the old SwapContext/StackContext, minus the
// one-shot outlet element — authors splat `navigator_outlet()` instead)
// ===========================================================================

/// Swap-navigator context, `provide`d into the world context for the
/// duration of the author layout build (and left available to the
/// layout's own reactive regions). Read it with
/// `runtime_world::inject::<SwapNav>()`.
#[derive(Clone)]
pub struct SwapNav {
    /// Currently active route key — highlight the live tab/nav item.
    pub active_route: Signal<&'static str>,
    /// Full resolved path of the active screen.
    pub active_path: Signal<String>,
    /// Switch to a sibling screen by route name (`Select`). Handler-safe:
    /// stages a command + tick, the driver effect commits on flush.
    pub on_select: Rc<dyn Fn(&'static str)>,
}

/// Stack-navigator context (`inject::<StackNav>()`).
#[derive(Clone)]
pub struct StackNav {
    /// The active (top) screen's route name.
    pub active_route: Signal<&'static str>,
    /// The active screen's full path.
    pub active_path: Signal<String>,
    /// Stack depth (1 at the root).
    pub depth: Signal<usize>,
    /// Whether a pop is possible (depth > 1).
    pub can_go_back: Signal<bool>,
    /// Pop the top screen (no-op at the root). Handler-safe.
    pub pop: Rc<dyn Fn()>,
}

// ===========================================================================
// NavHandle — the imperative handle (`.on_handle(...)` on the builders)
// ===========================================================================

/// Typed imperative handle to a live navigator — the new-core
/// counterpart of the SDK `SwapHandle`/`StackHandle` pair, unified
/// (the command vocabulary distinguishes the kinds; a swap ignores
/// stack verbs and vice versa, same as the old dispatchers). Cheap to
/// clone; safe to call from event handlers (dispatch only stages).
#[derive(Clone)]
pub struct NavHandle {
    dispatch: Rc<dyn Fn(NavCommand)>,
}

impl NavHandle {
    pub(crate) fn new(dispatch: Rc<dyn Fn(NavCommand)>) -> Self {
        NavHandle { dispatch }
    }

    /// Dispatch a raw command.
    pub fn dispatch(&self, cmd: NavCommand) {
        (self.dispatch)(cmd)
    }

    /// Switch to `route` (swap navigators). Selecting the already-active
    /// URL is a no-op at the driver.
    pub fn select<P: RouteParams>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.dispatch(NavCommand::Select {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    /// Push `route` onto the stack.
    pub fn push<P: RouteParams>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.dispatch(NavCommand::Push {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    /// Pop the top screen (no-op at the root).
    pub fn pop(&self) {
        self.dispatch(NavCommand::Pop);
    }

    /// Replace the top screen with `route`.
    pub fn replace<P: RouteParams>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.dispatch(NavCommand::Replace {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }

    /// Reset the whole stack to a single `route`.
    pub fn reset<P: RouteParams>(&self, route: &Route<P>, params: P) {
        let url = params.to_path(route.path());
        self.dispatch(NavCommand::Reset {
            name: route.name(),
            url,
            params: Box::new(params),
            state: None,
        });
    }
}

// ===========================================================================
// Prim payloads
// ===========================================================================

/// The `swap_navigator` primitive — a flat set of co-equal screens
/// switched via `Select`, chrome as author layout around the outlet.
pub struct SwapNavigatorPrim {
    pub config: NavConfig,
    /// The author layout closure (`None` ⇒ a bare outlet fills the
    /// navigator). Reads [`SwapNav`] via `inject`; splats
    /// `navigator_outlet()` exactly once.
    pub layout: Option<Rc<dyn Fn() -> Element>>,
    pub mount_policy: MountPolicy,
    /// Per-route bare-name select recipes (see [`SelectArgs`]).
    pub select_args: HashMap<&'static str, SelectArgs>,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub on_handle: Option<Box<dyn FnOnce(NavHandle)>>,
}

/// The `stack_navigator` primitive — push/pop over a back-stack, chrome
/// as author layout around the outlet.
pub struct StackNavigatorPrim {
    pub config: NavConfig,
    /// The author layout closure (reads [`StackNav`] via `inject`).
    pub layout: Option<Rc<dyn Fn() -> Element>>,
    pub retention: StackRetention,
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
    pub on_handle: Option<Box<dyn FnOnce(NavHandle)>>,
}

/// The `navigator_outlet` primitive — the single hole an author layout
/// splats to mark where the active screen renders. Style-less outlets
/// get the fill default (`outlet_fill_rules`); an author style replaces
/// it entirely (old `dispatch_navigator_outlet` contract).
pub struct NavigatorOutletPrim {
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
}
